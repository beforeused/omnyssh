//! Backend-managed state (tech-gui.md §3.4). Holds the host cache, the
//! metrics/status poller, the PTY manager (with the raw-byte tap, §3.6), the
//! per-session terminal channels, and the session registry that maps public ids to
//! the core's inner handles. The shared engine channel feeds the bridge.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use omnyssh_core::event::{CoreEvent, SessionId};
use omnyssh_core::ssh::client::Host;
use omnyssh_core::ssh::pool::PollManager;
use omnyssh_core::ssh::pty::PtyManager;
use omnyssh_core::ssh::session::SshSession;
use omnyssh_core::ssh::sftp::{SftpCommand, SftpManager};
use omnyssh_core::ssh::transfer::{TransferEngine, DEFAULT_STREAMS, MAX_STREAMS};
use tauri::ipc::Channel;
use tokio::sync::mpsc;

use crate::dto::{HostDto, TerminalBytes};
use crate::edit::EditWatcher;

/// Metric poll cadence. Mirrors the TUI's fixed interval; a configurable refresh
/// interval lands with settings in Stage 4.3 (tech-gui.md §4.3).
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Maps frontend-facing **public** session ids to the core's **inner** handles, and
/// back for terminals (the bridge labels `terminal-exited` by inner PTY id, §3.4).
/// A single monotonic id space keeps terminal — and, from Stage 3.2, SFTP — ids from
/// ever colliding in the frontend.
#[derive(Default)]
pub struct SessionRegistry {
    next: SessionId,
    pty_by_public: HashMap<SessionId, SessionId>,
    public_by_pty: HashMap<SessionId, SessionId>,
}

impl SessionRegistry {
    /// Allocate a fresh public id from the shared monotonic space (§3.4). SFTP
    /// sessions use this directly — they are keyed by their public id, with no inner
    /// mapping — so terminal and SFTP ids can never collide in the frontend.
    pub fn allocate(&mut self) -> SessionId {
        self.next += 1;
        self.next
    }

    /// Allocate a fresh public id for a terminal backed by PTY inner id `inner`.
    pub fn register_terminal(&mut self, inner: SessionId) -> SessionId {
        let public = self.allocate();
        self.pty_by_public.insert(public, inner);
        self.public_by_pty.insert(inner, public);
        public
    }

    /// The PTY inner id for a public id (command → core), if the session is live.
    pub fn pty_inner(&self, public: SessionId) -> Option<SessionId> {
        self.pty_by_public.get(&public).copied()
    }

    /// Drop a terminal by public id (user-initiated close), returning its inner id.
    pub fn remove_by_public(&mut self, public: SessionId) -> Option<SessionId> {
        let inner = self.pty_by_public.remove(&public)?;
        self.public_by_pty.remove(&inner);
        Some(inner)
    }

    /// Drop a terminal by inner id (remote exit), returning its public id.
    pub fn remove_by_pty(&mut self, inner: SessionId) -> Option<SessionId> {
        let public = self.public_by_pty.remove(&inner)?;
        self.pty_by_public.remove(&public);
        Some(public)
    }
}

pub struct GuiState {
    /// Cached host list, mapped to `HostDto` on demand for `list_hosts`.
    hosts: RwLock<Vec<Host>>,
    /// Metrics/status/discovery poller; replaced on reload, dropped on exit.
    poll: Mutex<Option<PollManager>>,
    /// All terminal sessions; constructed with the raw-byte tap (§3.6).
    pty: Mutex<PtyManager>,
    /// Terminal output routing: PTY inner id -> that tab's frontend channel.
    term_channels: Mutex<HashMap<SessionId, Channel<TerminalBytes>>>,
    /// One SFTP manager per tab, keyed by its public session id (§3.4).
    sftp: Mutex<HashMap<SessionId, SftpManager>>,
    /// One transfer engine per SFTP tab, keyed like `sftp`. Its lanes connect on
    /// the first transfer and disconnect when idle.
    transfers: Mutex<HashMap<SessionId, Arc<TransferEngine>>>,
    /// The live "parallel streams" setting, shared with every engine.
    transfer_streams: Arc<AtomicUsize>,
    /// Remote files open in an external editor, per SFTP tab.
    edits: Mutex<HashMap<SessionId, Arc<EditWatcher>>>,
    /// One SSH connection per host for the Docker panel, reused across refreshes.
    docker_sessions: tokio::sync::Mutex<HashMap<String, SshSession>>,
    /// One-shot latch so the startup update check fires once, on the frontend's first
    /// `reload_hosts` — i.e. only after its event bridge is listening (§3.4).
    update_check_started: AtomicBool,
    /// The host with a key-setup in flight, if any. One run at a time, mirroring the
    /// TUI's single-popup model — a second run would race a second `hosts.toml` write
    /// and clobber the progress panel (§4.2).
    key_setup: Mutex<Option<String>>,
    /// Public id <-> inner handle mapping for all sessions.
    sessions: Mutex<SessionRegistry>,
    /// Shared engine channel the bridge drains; cloned to `PollManager`/`PtyManager`.
    engine_tx: mpsc::Sender<CoreEvent>,
}

impl GuiState {
    pub fn new(engine_tx: mpsc::Sender<CoreEvent>, pty: PtyManager) -> Self {
        Self {
            hosts: RwLock::new(Vec::new()),
            poll: Mutex::new(None),
            pty: Mutex::new(pty),
            term_channels: Mutex::new(HashMap::new()),
            sftp: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            transfer_streams: Arc::new(AtomicUsize::new(DEFAULT_STREAMS)),
            edits: Mutex::new(HashMap::new()),
            docker_sessions: tokio::sync::Mutex::new(HashMap::new()),
            update_check_started: AtomicBool::new(false),
            key_setup: Mutex::new(None),
            sessions: Mutex::new(SessionRegistry::default()),
            engine_tx,
        }
    }

    /// Replace the host cache with a freshly loaded list.
    pub fn set_hosts(&self, hosts: Vec<Host>) {
        *self.hosts.write().expect("hosts lock poisoned") = hosts;
    }

    /// Clone the shared engine sender for a command that drives the core directly and
    /// reports via `CoreEvent` on the bridge — key setup (§4.2) and the startup update
    /// check (§4.3). Same channel the pollers and PTY sessions use (§3.4).
    pub fn engine_sender(&self) -> mpsc::Sender<CoreEvent> {
        self.engine_tx.clone()
    }

    /// Snapshot the cached hosts as wire DTOs (secret fields dropped by the map).
    pub fn host_dtos(&self) -> Vec<HostDto> {
        self.hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .map(HostDto::from)
            .collect()
    }

    /// Resolve `names` to their full `Host` records for a backend-only op (snippet
    /// execute, key setup). Secret material rides along here and never crosses the
    /// IPC boundary. Unknown names are skipped; order follows `names`.
    pub fn hosts_by_name(&self, names: &[String]) -> Vec<Host> {
        let hosts = self.hosts.read().expect("hosts lock poisoned");
        names
            .iter()
            .filter_map(|name| hosts.iter().find(|h| &h.name == name).cloned())
            .collect()
    }

    /// Clone one host record for a backend-only connect (SFTP `connect` needs the full
    /// `Host`, secrets included; they stay backend-side, §3.4).
    pub fn host_by_name(&self, name: &str) -> Option<Host> {
        self.hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .find(|h| h.name == name)
            .cloned()
    }

    /// Private-key paths explicitly referenced by configured hosts. Used only to
    /// include keys kept outside `~/.ssh` on the local Keys screen.
    pub fn configured_identity_files(&self) -> Vec<String> {
        self.hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .filter_map(|host| host.identity_file.clone())
            .collect()
    }

    /// Trigger an immediate metric poll of every host (tech-gui.md §4.2). A no-op if
    /// the pollers have not started yet. Non-blocking — it only nudges the poller tasks.
    pub fn refresh_metrics(&self) {
        if let Some(poll) = self.poll.lock().expect("poll lock poisoned").as_ref() {
            poll.refresh_all();
        }
    }

    /// Reserve the single key-setup slot for `host`. `Ok` starts the run; `Err` names the
    /// host already running one, so a concurrent start is rejected instead of racing a
    /// second `hosts.toml` write and clobbering the progress panel (§4.2). Paired with
    /// `end_key_setup`, called on every terminal outcome of the run.
    pub fn try_begin_key_setup(&self, host: &str) -> Result<(), String> {
        let mut slot = self.key_setup.lock().expect("key_setup lock poisoned");
        if let Some(active) = slot.as_ref() {
            return Err(format!("Key setup is already running for '{active}'"));
        }
        *slot = Some(host.to_string());
        Ok(())
    }

    /// Release the key-setup slot when a run reaches any terminal outcome.
    pub fn end_key_setup(&self) {
        *self.key_setup.lock().expect("key_setup lock poisoned") = None;
    }

    /// Claim the one-shot startup update check: `true` exactly once, on the first call.
    /// Driven from `reload_hosts` so the check runs only after the frontend's event
    /// bridge is listening, never dropping `update-available` on a listener race (§3.4).
    pub fn claim_update_check(&self) -> bool {
        self.update_check_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Start (or restart) the pollers for the cached hosts. Must run inside the
    /// Tauri async runtime — `PollManager::start` spawns tokio tasks (§3.4).
    pub fn restart_pollers(&self) {
        let hosts = self.hosts.read().expect("hosts lock poisoned").clone();
        let mut poll = self.poll.lock().expect("poll lock poisoned");
        if let Some(old) = poll.take() {
            old.shutdown();
        }
        *poll = Some(PollManager::start(
            hosts,
            self.engine_tx.clone(),
            POLL_INTERVAL,
        ));
    }

    /// Open a terminal for `host_name`, wiring its raw-output `channel`, and return
    /// the public session id. The session task starts here but cannot emit output
    /// until it connects, so registering the channel right after `open` beats the
    /// first byte (§3.4). Locks are taken un-nested to stay deadlock-free.
    pub fn open_terminal(
        &self,
        host_name: &str,
        cols: u16,
        rows: u16,
        channel: Channel<TerminalBytes>,
    ) -> Result<SessionId, String> {
        let host = self
            .hosts
            .read()
            .expect("hosts lock poisoned")
            .iter()
            .find(|h| h.name == host_name)
            .cloned()
            .ok_or_else(|| format!("unknown host '{host_name}'"))?;
        let inner = self
            .pty
            .lock()
            .expect("pty lock poisoned")
            .open(&host, cols, rows, self.engine_tx.clone())
            .map_err(|e| e.to_string())?;
        self.term_channels
            .lock()
            .expect("term_channels lock poisoned")
            .insert(inner, channel);
        Ok(self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .register_terminal(inner))
    }

    /// Resolve a public id to its live PTY inner id and run `f` on the manager. The
    /// un-nested lock order (sessions dropped before pty is taken) lives here alone so
    /// write/resize can't drift apart. Unknown/closed ids are a no-op.
    fn on_pty_inner(&self, public: SessionId, f: impl FnOnce(&mut PtyManager, SessionId)) {
        let inner = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .pty_inner(public);
        if let Some(inner) = inner {
            let mut pty = self.pty.lock().expect("pty lock poisoned");
            f(&mut pty, inner);
        }
    }

    /// Forward keystrokes to a terminal. Unknown/closed ids are a no-op.
    pub fn write_terminal(&self, public: SessionId, data: &[u8]) {
        self.on_pty_inner(public, |pty, inner| {
            let _ = pty.write(inner, data);
        });
    }

    /// Relay a resize (window_change) to a terminal. Unknown/closed ids are a no-op.
    pub fn resize_terminal(&self, public: SessionId, cols: u16, rows: u16) {
        self.on_pty_inner(public, |pty, inner| {
            let _ = pty.resize(inner, cols, rows);
        });
    }

    /// User-initiated close: tear down the core session and drop its routing state,
    /// so the task's later `PtyExited` finds no mapping and emits no `terminal-exited`
    /// (the frontend already tore the tab down, §3.4).
    pub fn close_terminal(&self, public: SessionId) {
        let inner = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .remove_by_public(public);
        if let Some(inner) = inner {
            self.pty.lock().expect("pty lock poisoned").close(inner);
            self.term_channels
                .lock()
                .expect("term_channels lock poisoned")
                .remove(&inner);
        }
    }

    /// Route a raw PTY chunk (keyed by inner id) into its tab's channel. Called by
    /// the raw-output forwarder; unknown/closed ids are dropped (§3.6).
    pub fn send_terminal_output(&self, inner: SessionId, bytes: Vec<u8>) {
        let channel = self
            .term_channels
            .lock()
            .expect("term_channels lock poisoned")
            .get(&inner)
            .cloned();
        if let Some(channel) = channel {
            let _ = channel.send(TerminalBytes(bytes));
        }
    }

    /// Remote-side exit: map the inner id to its public id and drop all routing
    /// state. Returns the public id to emit `terminal-exited` for, or `None` if the
    /// user already closed it (§3.4). The ended task never prunes its own
    /// `PtyManager` slot, so — like the TUI's `PtyExited` handler — we `close` it
    /// here (a no-op control send to a dead task) to avoid leaking the session Vec
    /// entry on every remote exit or failed connect.
    pub fn terminal_exited(&self, inner: SessionId) -> Option<SessionId> {
        let public = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .remove_by_pty(inner);
        self.pty.lock().expect("pty lock poisoned").close(inner);
        self.term_channels
            .lock()
            .expect("term_channels lock poisoned")
            .remove(&inner);
        public
    }

    /// Store a freshly connected SFTP manager and its transfer engine under a new
    /// public session id (§3.4). The id comes from the shared registry so it never
    /// collides with a terminal id.
    pub fn register_sftp(&self, manager: SftpManager, engine: TransferEngine) -> SessionId {
        let id = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .allocate();
        self.sftp
            .lock()
            .expect("sftp lock poisoned")
            .insert(id, manager);
        self.transfers
            .lock()
            .expect("transfers lock poisoned")
            .insert(id, Arc::new(engine));
        id
    }

    /// Enqueue a command to a live SFTP session. Unknown/closed ids are a no-op
    /// (fire-and-forget, mirroring the core's own model, §3.4).
    pub fn send_sftp(&self, session_id: SessionId, cmd: SftpCommand) {
        if let Some(manager) = self
            .sftp
            .lock()
            .expect("sftp lock poisoned")
            .get(&session_id)
        {
            manager.send(cmd);
        }
    }

    /// The browsing SSH connection of a live SFTP tab (for exec-based file ops).
    pub fn sftp_ssh(&self, session_id: SessionId) -> Option<SshSession> {
        self.sftp
            .lock()
            .expect("sftp lock poisoned")
            .get(&session_id)
            .map(SftpManager::ssh_session)
    }

    /// A live SSH connection to `host` for the Docker panel: the cached one while
    /// it is up, else a fresh one.
    pub async fn docker_session(&self, host: &Host) -> Result<SshSession, String> {
        let mut cache = self.docker_sessions.lock().await;
        if let Some(s) = cache.get(&host.name) {
            if !s.is_closed() {
                return Ok(s.clone());
            }
        }
        let session = SshSession::connect(host)
            .await
            .map_err(|e| format!("{e:#}"))?;
        cache.insert(host.name.clone(), session.clone());
        Ok(session)
    }

    /// The transfer engine of a live SFTP tab.
    pub fn transfer_engine(&self, session_id: SessionId) -> Option<Arc<TransferEngine>> {
        self.transfers
            .lock()
            .expect("transfers lock poisoned")
            .get(&session_id)
            .cloned()
    }

    /// The shared "parallel streams" setting handed to each new engine.
    pub fn transfer_streams(&self) -> Arc<AtomicUsize> {
        self.transfer_streams.clone()
    }

    /// Update the "parallel streams" setting; lanes spawned from now on follow it.
    pub fn set_transfer_streams(&self, streams: usize) {
        self.transfer_streams
            .store(streams.clamp(1, MAX_STREAMS), Ordering::Relaxed);
    }

    /// The editor-sync watcher of an SFTP tab, created on first use by `make`.
    pub fn edit_watcher(
        &self,
        session_id: SessionId,
        make: impl FnOnce() -> Arc<EditWatcher>,
    ) -> Arc<EditWatcher> {
        self.edits
            .lock()
            .expect("edits lock poisoned")
            .entry(session_id)
            .or_insert_with(make)
            .clone()
    }

    /// The editor-sync watcher of an SFTP tab, if one was ever started.
    pub fn existing_edit_watcher(&self, session_id: SessionId) -> Option<Arc<EditWatcher>> {
        self.edits
            .lock()
            .expect("edits lock poisoned")
            .get(&session_id)
            .cloned()
    }

    /// User-initiated SFTP close: drop the manager (a graceful `Disconnect` to its
    /// task), cancel the tab's transfers and stop syncing its edited files (§3.4).
    pub fn close_sftp(&self, session_id: SessionId) {
        if let Some(manager) = self
            .sftp
            .lock()
            .expect("sftp lock poisoned")
            .remove(&session_id)
        {
            manager.disconnect();
        }
        if let Some(engine) = self
            .transfers
            .lock()
            .expect("transfers lock poisoned")
            .remove(&session_id)
        {
            engine.shutdown();
        }
        if let Some(watcher) = self
            .edits
            .lock()
            .expect("edits lock poisoned")
            .remove(&session_id)
        {
            watcher.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ids_are_unique_and_monotonic() {
        let mut reg = SessionRegistry::default();
        let a = reg.register_terminal(1);
        let b = reg.register_terminal(2);
        let c = reg.register_terminal(3);
        assert_eq!((a, b, c), (1, 2, 3));
    }

    #[test]
    fn distinct_inner_ids_get_distinct_public_ids() {
        // Two PTY sessions that (hypothetically) reused an inner id space still map
        // to unique public ids — the frontend never sees a collision (§3.4).
        let mut reg = SessionRegistry::default();
        let first = reg.register_terminal(1);
        let second = reg.register_terminal(1);
        assert_ne!(first, second);
    }

    #[test]
    fn maps_public_to_inner_both_ways() {
        let mut reg = SessionRegistry::default();
        let public = reg.register_terminal(7);
        assert_eq!(reg.pty_inner(public), Some(7));
    }

    #[test]
    fn remove_by_public_clears_both_directions() {
        let mut reg = SessionRegistry::default();
        let public = reg.register_terminal(7);
        assert_eq!(reg.remove_by_public(public), Some(7));
        assert_eq!(reg.pty_inner(public), None);
        // The reverse mapping is gone too: a late exit finds nothing to emit.
        assert_eq!(reg.remove_by_pty(7), None);
    }

    #[test]
    fn remove_by_pty_returns_public_for_the_exit_event() {
        let mut reg = SessionRegistry::default();
        let public = reg.register_terminal(9);
        assert_eq!(reg.remove_by_pty(9), Some(public));
        assert_eq!(reg.pty_inner(public), None);
    }

    #[test]
    fn removing_one_session_leaves_others_intact() {
        let mut reg = SessionRegistry::default();
        let a = reg.register_terminal(10);
        let b = reg.register_terminal(20);
        reg.remove_by_public(a);
        assert_eq!(reg.pty_inner(b), Some(20));
    }

    #[test]
    fn unknown_ids_resolve_to_none() {
        let reg = SessionRegistry::default();
        assert_eq!(reg.pty_inner(999), None);
    }

    #[test]
    fn sftp_and_terminal_ids_share_one_monotonic_space() {
        // An SFTP `allocate` and a terminal `register` draw from the same counter, so
        // the frontend never sees a terminal id collide with an SFTP id (§3.4).
        let mut reg = SessionRegistry::default();
        let term = reg.register_terminal(1);
        let sftp = reg.allocate();
        let term2 = reg.register_terminal(1);
        assert_eq!((term, sftp, term2), (1, 2, 3));
        // The SFTP id has no inner PTY mapping — it is keyed by its public id directly.
        assert_eq!(reg.pty_inner(sftp), None);
    }

    #[test]
    fn transfer_streams_setting_is_clamped_and_shared() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());
        let shared = state.transfer_streams();
        assert_eq!(shared.load(Ordering::Relaxed), DEFAULT_STREAMS);
        state.set_transfer_streams(0);
        assert_eq!(shared.load(Ordering::Relaxed), 1);
        state.set_transfer_streams(99);
        assert_eq!(shared.load(Ordering::Relaxed), MAX_STREAMS);
        // Closing an unknown session is a no-op.
        state.close_sftp(42);
        assert!(state.transfer_engine(42).is_none());
    }

    // A remote exit must prune the core PtyManager slot (the ended task never removes
    // its own entry). Without `pty.close` in `terminal_exited` this leaks per exit.
    #[tokio::test]
    async fn remote_exit_prunes_the_pty_session() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let (raw_tx, _raw_rx) = mpsc::channel(8);
        let state = GuiState::new(engine_tx, PtyManager::with_raw_output(raw_tx));
        state.set_hosts(vec![Host {
            name: "h".to_string(),
            ..Host::default()
        }]);

        // A no-op output channel is enough — this asserts lifecycle, not bytes.
        let channel = Channel::new(|_| Ok(()));
        let public = state.open_terminal("h", 80, 24, channel).unwrap();
        let inner = state
            .sessions
            .lock()
            .unwrap()
            .pty_inner(public)
            .expect("registered");

        // Live session: `parser_for` is Some only while the manager holds the slot.
        assert!(state.pty.lock().unwrap().parser_for(inner).is_some());

        assert_eq!(state.terminal_exited(inner), Some(public));
        // Pruned — no leaked PtyManager.sessions entry.
        assert!(state.pty.lock().unwrap().parser_for(inner).is_none());
    }

    #[test]
    fn key_setup_runs_one_at_a_time() {
        // A second start while one is in flight is rejected (no racing hosts.toml write /
        // panel clobber); the slot frees on the terminal outcome so a retry succeeds (§4.2).
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());

        assert!(state.try_begin_key_setup("web-1").is_ok());
        let busy = state.try_begin_key_setup("web-2").unwrap_err();
        assert!(
            busy.contains("web-1"),
            "rejection names the busy host: {busy}"
        );
        // The same host cannot double-start either.
        assert!(state.try_begin_key_setup("web-1").is_err());

        state.end_key_setup();
        assert!(state.try_begin_key_setup("web-2").is_ok());
    }

    #[test]
    fn update_check_is_claimed_exactly_once() {
        // The startup update check must fire once (the first reload), never again — so a
        // later `reload_hosts` (after a host edit) does not re-hit GitHub (§3.4).
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());
        assert!(state.claim_update_check());
        assert!(!state.claim_update_check());
        assert!(!state.claim_update_check());
    }

    #[test]
    fn open_terminal_rejects_an_unknown_host() {
        let (engine_tx, _engine_rx) = mpsc::channel::<CoreEvent>(8);
        let state = GuiState::new(engine_tx, PtyManager::new());
        let channel = Channel::new(|_| Ok(()));
        assert!(state.open_terminal("nope", 80, 24, channel).is_err());
    }
}
