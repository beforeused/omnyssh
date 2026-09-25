//! Parallel, pipelined SFTP transfer engine (desktop GUI).
//!
//! [`TransferEngine`] moves files between the local filesystem and one host over a
//! pool of *lanes*. Each lane owns its own SSH connection and SFTP channel, so lanes
//! never share a TCP stream, a flow-control window, or an encryption task. A lane
//! runs one large unit (a file, or a segment of a big file) alongside several small
//! files, and every unit keeps many read/write requests in flight instead of waiting
//! a full round trip per chunk — on a 50 ms link that alone is the difference
//! between ~1 MB/s and saturating the line.
//!
//! Planning (expanding folders, spotting name conflicts) and folder creation run on
//! a separate "meta" channel over the browsing connection, so browsing never waits
//! behind bulk data and planning never waits for a lane to connect. Progress is
//! batched and reported a few times a second.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context};
use futures_util::stream::{FuturesOrdered, FuturesUnordered, StreamExt};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::rawsession::Limits;
use russh_sftp::client::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, oneshot, Notify, OnceCell};

use crate::event::TransferId;
use crate::ssh::client::Host;
use crate::ssh::session::SshSession;

/// Default number of lanes (parallel SSH connections) per host.
pub const DEFAULT_STREAMS: usize = 4;
/// Upper bound on lanes per host — past this, servers start rate-limiting
/// unauthenticated connections (`MaxStartups`) and the gain flattens out.
pub const MAX_STREAMS: usize = 8;

/// Files at or below this size are latency-bound: a lane runs several at once.
const SMALL_FILE: u64 = 2 * 1024 * 1024;
/// Files at or above this size are split into segments that separate lanes move
/// concurrently.
const SEGMENT_THRESHOLD: u64 = 16 * 1024 * 1024;
/// A segment is never smaller than this, so splitting never costs more round
/// trips than it saves.
const MIN_SEGMENT: u64 = 8 * 1024 * 1024;
/// Small files a lane runs at once next to its large units.
const SMALL_PER_LANE: usize = 16;
/// SFTP channels per lane connection. The server grants each channel its own
/// flow-control window (2 MiB on OpenSSH), which caps a single upload channel at
/// window / RTT; two channels per connection double that without another login.
const CHANNELS_PER_LANE: usize = 2;
/// Bytes kept in flight per unit. The SSH channel window caps what actually
/// travels; this just makes sure the window is never the idle party.
const INFLIGHT_BYTES: u64 = 8 * 1024 * 1024;
/// Request size when the server does not advertise `limits@openssh.com` — the
/// SFTP draft's guaranteed minimum.
const FALLBACK_CHUNK: u64 = 32 * 1024;
/// Largest request we ever send, even if the server advertises more.
const MAX_CHUNK: u64 = 256 * 1024;
/// Per-request response timeout. Generous: with megabytes in flight on a slow
/// link, a request legitimately queues behind the ones before it.
const REQUEST_TIMEOUT_SECS: u64 = 120;
/// A lane with nothing to do for this long closes its connection.
const LANE_IDLE: Duration = Duration::from_secs(60);
/// How often batched progress goes out.
const REPORT_EVERY: Duration = Duration::from_millis(150);
/// Local buffered I/O size for reading upload sources / writing downloads.
const LOCAL_BUF: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Which way a transfer moves bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

/// One file to move. Built by [`TransferEngine::commit`] from a [`Plan`], or
/// directly via [`TransferEngine::enqueue_file`].
#[derive(Debug, Clone)]
pub struct TransferSpec {
    pub id: TransferId,
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: String,
    /// Expected size in bytes (the source's size at planning time).
    pub size: u64,
    /// Permission bits (`0o777` mask) to give a newly created destination.
    pub mode: Option<u32>,
    /// Upload only: write over the existing remote file in place, keeping its
    /// owner, mode and links. Otherwise the data lands in a hidden temp file that
    /// is renamed into place once complete (small files are written directly and
    /// deleted on failure), so a broken transfer never leaves a truncated file
    /// under the real name.
    pub in_place: bool,
}

/// Lifecycle of one transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Running,
    Done,
    Failed(String),
    Cancelled,
}

impl TransferState {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed(_) | Self::Cancelled)
    }
}

/// A progress/state snapshot for one transfer, batched into
/// [`TransferEngine`]'s update channel.
#[derive(Debug, Clone)]
pub struct TransferUpdate {
    pub id: TransferId,
    pub state: TransferState,
    pub done: u64,
    pub total: u64,
}

/// An existing destination the planned file would collide with.
#[derive(Debug, Clone)]
pub struct Conflict {
    pub existing_size: u64,
    pub existing_is_dir: bool,
    /// A free sibling name ("report (1).pdf") for "keep both".
    pub alt_name: String,
}

/// A file a [`Plan`] would move.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub local: PathBuf,
    pub remote: String,
    /// Path relative to the batch's destination, for display.
    pub display: String,
    pub size: u64,
    pub mode: Option<u32>,
    pub conflict: Option<Conflict>,
}

/// A folder a [`Plan`] needs at the destination.
#[derive(Debug, Clone)]
pub struct PlannedDir {
    pub local: PathBuf,
    pub remote: String,
    /// Already present at the destination — merged into, not created.
    pub exists: bool,
}

/// An expanded batch awaiting conflict resolution: every folder is walked, every
/// file resolved to a source and destination path.
#[derive(Debug, Clone)]
pub struct Plan {
    pub direction: Direction,
    /// Parents first, so creating them in order always works.
    pub dirs: Vec<PlannedDir>,
    pub files: Vec<PlannedFile>,
}

impl Plan {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// How the user resolved one [`Conflict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Replace,
    Skip,
    KeepBoth,
}

/// Remote file metadata, as far as the editor sync needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteStat {
    pub size: u64,
    pub mtime: u32,
    pub is_dir: bool,
}

// ---------------------------------------------------------------------------
// SFTP channel
// ---------------------------------------------------------------------------

/// A raw SFTP channel plus the request sizes the server accepts. Holds a clone
/// of its SSH session so the connection lives exactly as long as the channel.
struct Channel {
    sftp: Arc<RawSftpSession>,
    read_len: u64,
    write_len: u64,
    _ssh: SshSession,
}

impl Channel {
    async fn open(ssh: SshSession) -> anyhow::Result<Self> {
        let stream = ssh.open_sftp_channel().await?;
        let mut raw = RawSftpSession::new(stream);
        raw.set_timeout(REQUEST_TIMEOUT_SECS).await;
        let version = raw.init().await.context("SFTP handshake")?;
        let (mut read_len, mut write_len) = (FALLBACK_CHUNK, FALLBACK_CHUNK);
        let has_limits = version
            .extensions
            .get(russh_sftp::extensions::LIMITS)
            .is_some_and(|v| v == "1");
        if has_limits {
            if let Ok(ext) = raw.limits().await {
                let limits = Limits::from(ext);
                read_len = limits.read_len.unwrap_or(FALLBACK_CHUNK);
                write_len = limits.write_len.unwrap_or(FALLBACK_CHUNK);
                raw.set_limits(Arc::new(limits));
            }
        }
        Ok(Self {
            sftp: Arc::new(raw),
            read_len: read_len.clamp(1024, MAX_CHUNK),
            write_len: write_len.clamp(1024, MAX_CHUNK),
            _ssh: ssh,
        })
    }

    async fn stat(&self, path: &str) -> Result<Option<FileAttributes>, SftpError> {
        match self.sftp.stat(path).await {
            Ok(a) => Ok(Some(a.attrs)),
            Err(e) if is_status(&e, StatusCode::NoSuchFile) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every entry of a remote directory except `.` and `..`.
    async fn list(&self, path: &str) -> anyhow::Result<Vec<(String, FileAttributes)>> {
        let handle = self
            .sftp
            .opendir(path)
            .await
            .with_context(|| format!("open remote folder '{path}'"))?
            .handle;
        let mut out = Vec::new();
        let result = loop {
            match self.sftp.readdir(handle.as_str()).await {
                Ok(name) => {
                    for f in name.files {
                        if f.filename != "." && f.filename != ".." {
                            out.push((f.filename, f.attrs));
                        }
                    }
                }
                Err(e) if is_status(&e, StatusCode::Eof) => break Ok(()),
                Err(e) => break Err(anyhow!("read remote folder '{path}': {e}")),
            }
        };
        let _ = self.sftp.close(handle).await;
        result.map(|()| out)
    }
}

/// File-type tests on the mode's type bits. `FileAttributes::is_*` test single
/// bits, so e.g. a symlink (0o120000) also reads as a regular file there.
trait Kind {
    fn kind(&self) -> u32;
    fn kind_dir(&self) -> bool {
        self.kind() == 0o040000
    }
    fn kind_file(&self) -> bool {
        self.kind() == 0o100000
    }
    fn kind_link(&self) -> bool {
        self.kind() == 0o120000
    }
}

impl Kind for FileAttributes {
    fn kind(&self) -> u32 {
        self.permissions.unwrap_or(0) & 0o170000
    }
}

fn is_status(e: &SftpError, code: StatusCode) -> bool {
    matches!(e, SftpError::Status(s) if s.status_code == code)
}

fn perm_attrs(mode: Option<u32>) -> FileAttributes {
    let mut attrs = FileAttributes::empty();
    attrs.permissions = mode.map(|m| m & 0o777);
    attrs
}

// ---------------------------------------------------------------------------
// Jobs and units
// ---------------------------------------------------------------------------

const CANCELLED: &str = "Cancelled";

struct Job {
    spec: TransferSpec,
    segments: usize,
    segments_left: AtomicUsize,
    done: AtomicU64,
    /// Set on cancel or on any segment's failure: siblings stop at the next chunk.
    stop: AtomicBool,
    state: Mutex<TransferState>,
    dirty: AtomicBool,
    /// Multi-segment jobs create/truncate their target once, before any segment
    /// opens it for writing.
    prepared: OnceCell<Result<(), String>>,
    completion: Mutex<Option<oneshot::Sender<Result<(), String>>>>,
}

impl Job {
    fn new(spec: TransferSpec, segments: usize) -> Self {
        Self {
            spec,
            segments,
            segments_left: AtomicUsize::new(segments),
            done: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            state: Mutex::new(TransferState::Queued),
            dirty: AtomicBool::new(true),
            prepared: OnceCell::new(),
            completion: Mutex::new(None),
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn state(&self) -> TransferState {
        self.state.lock().expect("job state lock").clone()
    }

    fn progress(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn mark_running(&self) {
        let mut state = self.state.lock().expect("job state lock");
        if *state == TransferState::Queued {
            *state = TransferState::Running;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Move to a terminal state unless already in one. Returns whether it moved.
    fn settle(&self, next: TransferState) -> bool {
        let mut state = self.state.lock().expect("job state lock");
        if state.is_terminal() {
            return false;
        }
        let outcome = match &next {
            TransferState::Done => Ok(()),
            TransferState::Failed(e) => Err(e.clone()),
            _ => Err(CANCELLED.to_string()),
        };
        *state = next;
        drop(state);
        self.dirty.store(true, Ordering::Relaxed);
        if let Some(tx) = self.completion.lock().expect("job completion lock").take() {
            let _ = tx.send(outcome);
        }
        true
    }

    fn fail(&self, error: String) {
        self.stop.store(true, Ordering::Relaxed);
        if error != CANCELLED {
            self.settle(TransferState::Failed(error));
        }
    }

    /// Large new uploads go through a temp file renamed into place at the end.
    /// Small ones are written straight to their name — the rename would cost a
    /// round trip per file, and a failed one is deleted just the same.
    fn uses_remote_temp(&self) -> bool {
        !self.spec.in_place && self.spec.size > SMALL_FILE
    }

    /// Where the bytes are written while the transfer runs.
    fn remote_target(&self) -> String {
        if !self.uses_remote_temp() {
            self.spec.remote.clone()
        } else {
            let (dir, name) = remote_split(&self.spec.remote);
            remote_join(dir, &temp_name(name))
        }
    }

    fn local_target(&self) -> PathBuf {
        let name = self
            .spec
            .local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.spec.local.with_file_name(temp_name(&name))
    }
}

fn temp_name(name: &str) -> String {
    format!(".{name}.omnyssh-part")
}

struct Unit {
    job: Arc<Job>,
    start: u64,
    end: u64,
    big: bool,
}

fn split_units(job: &Arc<Job>) -> Vec<Unit> {
    let size = job.spec.size;
    let big = size > SMALL_FILE;
    let n = job.segments as u64;
    let step = size.div_ceil(n.max(1)).max(1);
    (0..n)
        .map(|i| Unit {
            job: job.clone(),
            start: (i * step).min(size),
            end: if i + 1 == n {
                size
            } else {
                ((i + 1) * step).min(size)
            },
            big,
        })
        .collect()
}

fn segments_for(size: u64, streams: usize) -> usize {
    if size < SEGMENT_THRESHOLD {
        return 1;
    }
    let by_size = (size / MIN_SEGMENT).max(1) as usize;
    by_size.min(streams.max(1) * CHANNELS_PER_LANE)
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct Shared {
    host: Host,
    streams: Arc<AtomicUsize>,
    queue: Mutex<VecDeque<Unit>>,
    wake: Notify,
    jobs: Mutex<HashMap<TransferId, Arc<Job>>>,
    /// Lanes spawned and not yet exited (connecting ones included).
    lanes: Mutex<usize>,
    closed: AtomicBool,
    next_id: AtomicU64,
}

/// Per-host transfer engine. Cheap to create: lanes connect only when the first
/// transfer is enqueued and disconnect again after [`LANE_IDLE`] without work.
pub struct TransferEngine {
    shared: Arc<Shared>,
    meta_ssh: SshSession,
    meta: tokio::sync::Mutex<Option<Arc<Channel>>>,
    plans: Mutex<HashMap<u64, Plan>>,
    next_plan: AtomicU64,
}

impl TransferEngine {
    /// Creates the engine. `meta_ssh` is the (already connected) browsing
    /// session; `streams` is the live lane-count setting, read whenever lanes
    /// are spawned; batched progress goes to `updates`.
    pub fn new(
        host: Host,
        meta_ssh: SshSession,
        streams: Arc<AtomicUsize>,
        updates: mpsc::Sender<Vec<TransferUpdate>>,
    ) -> Self {
        let shared = Arc::new(Shared {
            host,
            streams,
            queue: Mutex::new(VecDeque::new()),
            wake: Notify::new(),
            jobs: Mutex::new(HashMap::new()),
            lanes: Mutex::new(0),
            closed: AtomicBool::new(false),
            next_id: AtomicU64::new(0),
        });
        tokio::spawn(report_loop(Arc::downgrade(&shared), updates));
        Self {
            shared,
            meta_ssh,
            meta: tokio::sync::Mutex::new(None),
            plans: Mutex::new(HashMap::new()),
            next_plan: AtomicU64::new(0),
        }
    }

    fn streams(&self) -> usize {
        self.shared
            .streams
            .load(Ordering::Relaxed)
            .clamp(1, MAX_STREAMS)
    }

    /// The meta channel, (re)opened on demand over the browsing connection.
    async fn meta(&self) -> anyhow::Result<Arc<Channel>> {
        let mut slot = self.meta.lock().await;
        if let Some(ch) = slot.as_ref() {
            return Ok(ch.clone());
        }
        let ch = Arc::new(Channel::open(self.meta_ssh.clone()).await?);
        *slot = Some(ch.clone());
        Ok(ch)
    }

    /// Runs `op` on the meta channel, reopening it once if it died underneath.
    async fn with_meta<T, F, Fut>(&self, op: F) -> anyhow::Result<T>
    where
        F: Fn(Arc<Channel>) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        let ch = self.meta().await?;
        match op(ch).await {
            Ok(v) => Ok(v),
            Err(e) => {
                let dead = e.downcast_ref::<SftpError>().is_some_and(|e| {
                    matches!(e, SftpError::UnexpectedBehavior(_) | SftpError::Timeout)
                });
                if !dead {
                    return Err(e);
                }
                *self.meta.lock().await = None;
                op(self.meta().await?).await
            }
        }
    }

    /// Stat a remote path (following links). `None` when it does not exist.
    pub async fn stat_remote(&self, path: &str) -> anyhow::Result<Option<RemoteStat>> {
        let path = path.to_string();
        self.with_meta(|ch| {
            let path = path.clone();
            async move {
                let attrs = ch.stat(&path).await?;
                Ok(attrs.map(|a| RemoteStat {
                    size: a.size.unwrap_or(0),
                    mtime: a.mtime.unwrap_or(0),
                    is_dir: a.kind_dir(),
                }))
            }
        })
        .await
    }

    // -- planning -----------------------------------------------------------

    /// Expand `sources` (local paths to upload, or remote paths to download) into
    /// a [`Plan`] targeting `dest_dir`, and park it under the returned id until
    /// [`commit`](Self::commit) or [`discard`](Self::discard).
    pub async fn plan(
        &self,
        direction: Direction,
        sources: Vec<String>,
        dest_dir: String,
    ) -> anyhow::Result<(u64, Plan)> {
        // The user is about to transfer: connect the lanes while we plan and they
        // look at any conflicts. Idle lanes disconnect again on their own.
        self.ensure_lanes();
        let plan = match direction {
            Direction::Upload => self.plan_upload(sources, dest_dir).await?,
            Direction::Download => self.plan_download(sources, dest_dir).await?,
        };
        let id = self.next_plan.fetch_add(1, Ordering::Relaxed) + 1;
        self.plans
            .lock()
            .expect("plans lock")
            .insert(id, plan.clone());
        Ok((id, plan))
    }

    pub fn discard(&self, plan_id: u64) {
        self.plans.lock().expect("plans lock").remove(&plan_id);
    }

    async fn plan_upload(&self, sources: Vec<String>, dest_dir: String) -> anyhow::Result<Plan> {
        let tree = tokio::task::spawn_blocking(move || walk_local(&sources))
            .await
            .context("walk local files")??;
        let meta = self.meta().await?;

        // Destination listings, fetched level by level (all folders of a level at
        // once) and only for folders that already exist remotely.
        let mut listings: HashMap<String, HashMap<String, FileAttributes>> = HashMap::new();
        let mut level = vec![String::new()];
        while !level.is_empty() {
            let fetched = futures_util::future::join_all(level.iter().map(|rel| {
                let meta = meta.clone();
                let path = remote_under(&dest_dir, rel);
                async move { meta.list(&path).await }
            }))
            .await;
            let mut next = Vec::new();
            for (rel, listing) in level.into_iter().zip(fetched) {
                let map: HashMap<String, FileAttributes> = listing?.into_iter().collect();
                for item in tree
                    .iter()
                    .filter(|i| i.is_dir && parent_rel(&i.rel) == rel)
                {
                    if map.get(leaf(&item.rel)).is_some_and(|a| a.kind_dir()) {
                        next.push(item.rel.clone());
                    }
                }
                listings.insert(rel, map);
            }
            level = next;
        }

        let mut plan = Plan {
            direction: Direction::Upload,
            dirs: Vec::new(),
            files: Vec::new(),
        };
        let mut claimed: HashMap<String, HashSet<String>> = HashMap::new();
        for item in &tree {
            let parent = parent_rel(&item.rel);
            let name = leaf(&item.rel).to_string();
            let existing = listings.get(parent).and_then(|m| m.get(&name));
            let remote = remote_under(&dest_dir, &item.rel);
            if item.is_dir {
                plan.dirs.push(PlannedDir {
                    local: item.path.clone(),
                    remote,
                    exists: existing.is_some_and(|a| a.kind_dir()),
                });
                continue;
            }
            let conflict = existing.map(|a| {
                let taken = claimed.entry(parent.to_string()).or_default();
                let listing = listings.get(parent);
                let alt = free_name(&name, |n| {
                    taken.contains(n) || listing.is_some_and(|m| m.contains_key(n))
                });
                taken.insert(alt.clone());
                Conflict {
                    existing_size: a.size.unwrap_or(0),
                    existing_is_dir: a.kind_dir(),
                    alt_name: alt,
                }
            });
            plan.files.push(PlannedFile {
                local: item.path.clone(),
                remote,
                display: item.rel.clone(),
                size: item.size,
                mode: item.mode,
                conflict,
            });
        }
        Ok(plan)
    }

    async fn plan_download(&self, sources: Vec<String>, dest_dir: String) -> anyhow::Result<Plan> {
        let meta = self.meta().await?;
        let dest = PathBuf::from(&dest_dir);

        // Top-level sources: stat (following links) all at once.
        let stats = futures_util::future::join_all(sources.iter().map(|p| {
            let meta = meta.clone();
            let p = p.clone();
            async move { meta.stat(&p).await }
        }))
        .await;

        let mut items: Vec<RemoteItem> = Vec::new();
        let mut level: Vec<(String, String)> = Vec::new(); // (remote path, rel)
        for (path, stat) in sources.iter().zip(stats) {
            let attrs = stat
                .map_err(|e| anyhow!("stat '{path}': {e}"))?
                .ok_or_else(|| anyhow!("'{path}' no longer exists"))?;
            let name = remote_split(path).1.to_string();
            if !safe_name(&name) {
                continue;
            }
            items.push(RemoteItem::from_attrs(path.clone(), name.clone(), &attrs));
            if attrs.kind_dir() {
                level.push((path.clone(), name));
            }
        }

        // Walk folders breadth-first, a whole level concurrently.
        while !level.is_empty() {
            let fetched = futures_util::future::join_all(level.iter().map(|(path, _)| {
                let meta = meta.clone();
                let path = path.clone();
                async move { meta.list(&path).await }
            }))
            .await;
            let mut next = Vec::new();
            for ((dir, rel), listing) in level.into_iter().zip(fetched) {
                let mut links = Vec::new();
                for (name, attrs) in listing? {
                    if !safe_name(&name) {
                        tracing::warn!("skipping unsafe remote name {name:?} in {dir}");
                        continue;
                    }
                    let path = remote_join(&dir, &name);
                    let child_rel = format!("{rel}/{name}");
                    if attrs.kind_link() {
                        links.push((path, child_rel));
                    } else if attrs.kind_dir() {
                        items.push(RemoteItem::from_attrs(
                            path.clone(),
                            child_rel.clone(),
                            &attrs,
                        ));
                        next.push((path, child_rel));
                    } else if attrs.kind_file() {
                        items.push(RemoteItem::from_attrs(path, child_rel, &attrs));
                    }
                }
                // Links to files are copied as files; links to folders are skipped
                // (they can loop).
                let resolved = futures_util::future::join_all(links.iter().map(|(p, _)| {
                    let meta = meta.clone();
                    let p = p.clone();
                    async move { meta.stat(&p).await }
                }))
                .await;
                for ((path, rel), stat) in links.into_iter().zip(resolved) {
                    if let Ok(Some(a)) = stat {
                        if !a.kind_dir() {
                            items.push(RemoteItem::from_attrs(path, rel, &a));
                        }
                    }
                }
            }
            level = next;
        }

        let mut plan = Plan {
            direction: Direction::Download,
            dirs: Vec::new(),
            files: Vec::new(),
        };
        let mut claimed: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        for item in items {
            let local = item.rel.split('/').fold(dest.clone(), |p, c| p.join(c));
            let existing = std::fs::metadata(&local).ok();
            if item.is_dir {
                plan.dirs.push(PlannedDir {
                    local,
                    remote: item.path,
                    exists: existing.is_some_and(|m| m.is_dir()),
                });
                continue;
            }
            let conflict = existing.map(|m| {
                let parent = local.parent().map(Path::to_path_buf).unwrap_or_default();
                let taken = claimed.entry(parent.clone()).or_default();
                let name = leaf(&item.rel).to_string();
                let alt = free_name(&name, |n| taken.contains(n) || parent.join(n).exists());
                taken.insert(alt.clone());
                Conflict {
                    existing_size: m.len(),
                    existing_is_dir: m.is_dir(),
                    alt_name: alt,
                }
            });
            plan.files.push(PlannedFile {
                local,
                remote: item.path,
                display: item.rel,
                size: item.size,
                mode: item.mode,
                conflict,
            });
        }
        Ok(plan)
    }

    /// Create the plan's folders, then enqueue its files with `resolutions`
    /// applied (keyed by index into `plan.files`; unresolved conflicts are
    /// skipped). Returns the enqueued specs in plan order.
    pub async fn commit(
        &self,
        plan_id: u64,
        resolutions: &HashMap<usize, Resolution>,
    ) -> anyhow::Result<Vec<TransferSpec>> {
        let plan = self
            .plans
            .lock()
            .expect("plans lock")
            .remove(&plan_id)
            .ok_or_else(|| anyhow!("this transfer batch has expired"))?;

        let mut specs = Vec::new();
        for (i, file) in plan.files.iter().enumerate() {
            let mut local = file.local.clone();
            let mut remote = file.remote.clone();
            let mut in_place = false;
            if let Some(conflict) = &file.conflict {
                match resolutions.get(&i).copied().unwrap_or(Resolution::Skip) {
                    Resolution::Skip => continue,
                    Resolution::Replace => in_place = plan.direction == Direction::Upload,
                    Resolution::KeepBoth => match plan.direction {
                        Direction::Upload => {
                            remote = remote_join(remote_split(&remote).0, &conflict.alt_name)
                        }
                        Direction::Download => local = local.with_file_name(&conflict.alt_name),
                    },
                }
            }
            specs.push(TransferSpec {
                id: self.shared.next_id.fetch_add(1, Ordering::Relaxed) + 1,
                direction: plan.direction,
                local,
                remote,
                size: file.size,
                mode: file.mode,
                in_place,
            });
        }

        let new_dirs: Vec<&PlannedDir> = plan.dirs.iter().filter(|d| !d.exists).collect();
        match plan.direction {
            Direction::Download => {
                for dir in new_dirs {
                    tokio::fs::create_dir_all(&dir.local)
                        .await
                        .with_context(|| format!("create folder '{}'", dir.local.display()))?;
                }
            }
            Direction::Upload if !new_dirs.is_empty() => {
                // One round trip per depth level: siblings are created concurrently.
                let meta = self.meta().await?;
                let depth = |d: &&PlannedDir| d.remote.matches('/').count();
                let mut by_depth: Vec<&PlannedDir> = new_dirs;
                by_depth.sort_by_key(depth);
                for level in by_depth.chunk_by(|a, b| depth(a) == depth(b)) {
                    let results = futures_util::future::join_all(level.iter().map(|d| {
                        let meta = meta.clone();
                        let attrs = perm_attrs(local_mode(&d.local));
                        let path = d.remote.clone();
                        async move {
                            match meta.sftp.mkdir(path.as_str(), attrs).await {
                                Ok(_) => Ok(()),
                                // Raced into existence (or a re-run): fine if it is a folder.
                                Err(e) => match meta.stat(&path).await {
                                    Ok(Some(a)) if a.kind_dir() => Ok(()),
                                    _ => Err(anyhow!("create remote folder '{path}': {e}")),
                                },
                            }
                        }
                    }))
                    .await;
                    results.into_iter().collect::<anyhow::Result<Vec<()>>>()?;
                }
            }
            Direction::Upload => {}
        }

        self.enqueue(specs.clone());
        Ok(specs)
    }

    // -- queue --------------------------------------------------------------

    /// Enqueue one ready-made transfer and get notified when it settles. Used by
    /// the editor sync; the id is allocated here.
    pub fn enqueue_file(
        &self,
        mut spec: TransferSpec,
    ) -> (TransferSpec, oneshot::Receiver<Result<(), String>>) {
        spec.id = self.shared.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        let job = self.make_job(spec.clone());
        *job.completion.lock().expect("job completion lock") = Some(tx);
        self.push_jobs(vec![job]);
        (spec, rx)
    }

    fn enqueue(&self, specs: Vec<TransferSpec>) {
        let jobs = specs.into_iter().map(|s| self.make_job(s)).collect();
        self.push_jobs(jobs);
    }

    fn make_job(&self, spec: TransferSpec) -> Arc<Job> {
        let segments = segments_for(spec.size, self.streams());
        Arc::new(Job::new(spec, segments))
    }

    fn push_jobs(&self, jobs: Vec<Arc<Job>>) {
        if jobs.is_empty() {
            return;
        }
        {
            let mut map = self.shared.jobs.lock().expect("jobs lock");
            for job in &jobs {
                map.insert(job.spec.id, job.clone());
            }
        }
        {
            let mut queue = self.shared.queue.lock().expect("queue lock");
            for job in &jobs {
                queue.extend(split_units(job));
            }
        }
        self.ensure_lanes();
        self.shared.wake.notify_waiters();
    }

    fn ensure_lanes(&self) {
        let target = self.streams();
        let mut lanes = self.shared.lanes.lock().expect("lanes lock");
        while *lanes < target {
            *lanes += 1;
            tokio::spawn(lane_loop(self.shared.clone()));
        }
    }

    /// Cancel transfers. Queued ones never start; running ones stop at the next
    /// chunk and clean up their temp file.
    pub fn cancel(&self, ids: &[TransferId]) {
        let map = self.shared.jobs.lock().expect("jobs lock");
        for id in ids {
            if let Some(job) = map.get(id) {
                job.stop.store(true, Ordering::Relaxed);
                job.settle(TransferState::Cancelled);
            }
        }
        drop(map);
        self.shared.wake.notify_waiters();
    }

    /// Re-run failed or cancelled transfers from scratch, under the same ids.
    pub fn retry(&self, ids: &[TransferId]) {
        let jobs: Vec<Arc<Job>> = {
            let map = self.shared.jobs.lock().expect("jobs lock");
            ids.iter()
                .filter_map(|id| map.get(id))
                .filter(|job| {
                    matches!(
                        job.state(),
                        TransferState::Failed(_) | TransferState::Cancelled
                    ) && job.segments_left.load(Ordering::Acquire) == 0
                })
                .map(|job| self.make_job(job.spec.clone()))
                .collect()
        };
        self.push_jobs(jobs);
    }

    /// Drop finished transfers from memory (after the UI cleared them).
    pub fn forget(&self, ids: &[TransferId]) {
        let mut map = self.shared.jobs.lock().expect("jobs lock");
        for id in ids {
            if map.get(id).is_some_and(|j| j.state().is_terminal()) {
                map.remove(id);
            }
        }
    }

    /// Cancel everything and let the lanes wind down (tab closed).
    pub fn shutdown(&self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        let ids: Vec<TransferId> = self
            .shared
            .jobs
            .lock()
            .expect("jobs lock")
            .keys()
            .copied()
            .collect();
        self.cancel(&ids);
    }
}

impl Drop for TransferEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Lanes
// ---------------------------------------------------------------------------

async fn lane_loop(shared: Arc<Shared>) {
    let channels = match connect_lane(&shared.host).await {
        Ok(channels) => channels,
        Err(e) => {
            tracing::warn!("transfer lane failed to connect: {e:#}");
            lane_gone(&shared, Some(format!("Could not connect: {e}")));
            return;
        }
    };

    // One large unit per channel; small ones spread over whichever channel is
    // least busy with them.
    let mut running = FuturesUnordered::new();
    let mut big_on = [false; CHANNELS_PER_LANE];
    let mut small_on = [0usize; CHANNELS_PER_LANE];
    loop {
        let notified = shared.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        loop {
            let big_free = big_on.iter().position(|b| !b);
            let small_total: usize = small_on.iter().sum();
            let Some(unit) = take_unit(&shared, big_free.is_some(), small_total) else {
                break;
            };
            let slot = if unit.big {
                big_free.expect("take_unit only hands out a large unit to a free slot")
            } else {
                (0..CHANNELS_PER_LANE)
                    .min_by_key(|&i| small_on[i])
                    .expect("a lane has channels")
            };
            if unit.big {
                big_on[slot] = true;
            } else {
                small_on[slot] += 1;
            }
            let ch = channels[slot].clone();
            running.push(async move {
                let big = unit.big;
                run_unit(&ch, unit).await;
                (slot, big)
            });
        }

        if running.is_empty() {
            if shared.closed.load(Ordering::Relaxed) {
                break;
            }
            tokio::select! {
                _ = &mut notified => continue,
                _ = tokio::time::sleep(LANE_IDLE) => {
                    // Exit only if the queue is still empty, decided under the lane
                    // lock so a concurrent enqueue either sees us gone or we see it.
                    let mut lanes = shared.lanes.lock().expect("lanes lock");
                    if shared.queue.lock().expect("queue lock").is_empty() {
                        *lanes -= 1;
                        return;
                    }
                }
            }
        } else {
            tokio::select! {
                Some((slot, big)) = running.next() => {
                    if big { big_on[slot] = false } else { small_on[slot] -= 1 }
                }
                _ = &mut notified => {}
            }
        }
    }
    lane_gone(&shared, None);
}

/// Connects a lane: one SSH connection carrying [`CHANNELS_PER_LANE`] SFTP
/// channels, opened concurrently.
async fn connect_lane(host: &Host) -> anyhow::Result<Vec<Arc<Channel>>> {
    let ssh = SshSession::connect_bulk(host).await?;
    let opened =
        futures_util::future::join_all((0..CHANNELS_PER_LANE).map(|_| Channel::open(ssh.clone())))
            .await;
    // Servers may cap channels per connection (`MaxSessions`); make do with
    // whatever opened, as long as one did.
    let mut channels = Vec::new();
    let mut first_error = None;
    for ch in opened {
        match ch {
            Ok(ch) => channels.push(Arc::new(ch)),
            Err(e) => first_error = first_error.or(Some(e)),
        }
    }
    if channels.is_empty() {
        return Err(first_error.unwrap_or_else(|| anyhow!("no SFTP channel")));
    }
    while channels.len() < CHANNELS_PER_LANE {
        channels.push(channels[0].clone());
    }
    Ok(channels)
}

/// A lane exited. If it failed to connect and no other lane is left to pick up
/// the queue, fail everything still queued instead of stranding it.
fn lane_gone(shared: &Shared, error: Option<String>) {
    let mut lanes = shared.lanes.lock().expect("lanes lock");
    *lanes -= 1;
    if *lanes > 0 {
        return;
    }
    let Some(error) = error else { return };
    let stranded: Vec<Unit> = shared.queue.lock().expect("queue lock").drain(..).collect();
    drop(lanes);
    for unit in stranded {
        unit.job.fail(error.clone());
        unit.job.segments_left.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Next unit this lane may run: a large unit only while one of its channels has
/// none, small ones up to [`SMALL_PER_LANE`]. Skips over units that do not fit
/// rather than blocking on them.
fn take_unit(shared: &Shared, big_slot_free: bool, small_running: usize) -> Option<Unit> {
    let mut queue = shared.queue.lock().expect("queue lock");
    let pos = queue.iter().position(|u| {
        if u.big {
            big_slot_free
        } else {
            small_running < SMALL_PER_LANE
        }
    })?;
    queue.remove(pos)
}

async fn run_unit(ch: &Channel, unit: Unit) {
    let job = unit.job.clone();
    let result = if job.stopped() {
        Err(CANCELLED.to_string())
    } else {
        job.mark_running();
        match job.spec.direction {
            Direction::Upload => upload_range(ch, &unit).await,
            Direction::Download => download_range(ch, &unit).await,
        }
    };
    drop(unit);
    if let Err(e) = result {
        job.fail(e);
    }
    if job.segments_left.fetch_sub(1, Ordering::AcqRel) == 1 {
        finalize(ch, &job).await;
    }
}

/// The last segment of a job finished: move the temp file into place, or clean
/// it up if anything failed or the job was cancelled.
async fn finalize(ch: &Channel, job: &Job) {
    let spec = &job.spec;
    if job.stopped() || job.state().is_terminal() {
        match spec.direction {
            Direction::Upload if !spec.in_place => {
                let _ = ch.sftp.remove(job.remote_target()).await;
            }
            Direction::Upload => {}
            Direction::Download => {
                let _ = tokio::fs::remove_file(job.local_target()).await;
            }
        }
        job.settle(TransferState::Cancelled);
        return;
    }
    let result = match spec.direction {
        Direction::Upload if !job.uses_remote_temp() => Ok(()),
        Direction::Upload => {
            let temp = job.remote_target();
            match ch.sftp.rename(temp.as_str(), spec.remote.as_str()).await {
                Ok(_) => Ok(()),
                // SFTPv3 rename refuses to replace; the target appeared meanwhile.
                Err(_) => {
                    let _ = ch.sftp.remove(spec.remote.as_str()).await;
                    ch.sftp
                        .rename(temp.as_str(), spec.remote.as_str())
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("could not move the upload into place: {e}"))
                }
            }
            .inspect_err(|_| {
                let sftp = ch.sftp.clone();
                tokio::spawn(async move { sftp.remove(temp).await });
            })
        }
        Direction::Download => {
            let temp = job.local_target();
            set_local_mode(&temp, spec.mode);
            match tokio::fs::rename(&temp, &spec.local).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    let _ = tokio::fs::remove_file(&temp).await;
                    Err(format!("could not move the download into place: {e}"))
                }
            }
        }
    };
    match result {
        Ok(()) => job.settle(TransferState::Done),
        Err(e) => job.settle(TransferState::Failed(e)),
    };
}

// ---------------------------------------------------------------------------
// Data paths
// ---------------------------------------------------------------------------

fn inflight_requests(chunk: u64) -> usize {
    (INFLIGHT_BYTES / chunk).clamp(4, 1024) as usize
}

async fn prepare_remote(ch: &Channel, job: &Job) -> Result<(), String> {
    job.prepared
        .get_or_init(|| async {
            let target = job.remote_target();
            let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE;
            let handle = ch
                .sftp
                .open(target.as_str(), flags, perm_attrs(job.spec.mode))
                .await
                .map_err(|e| format!("create '{target}': {e}"))?
                .handle;
            let _ = ch.sftp.close(handle).await;
            Ok(())
        })
        .await
        .clone()
}

async fn upload_range(ch: &Channel, unit: &Unit) -> Result<(), String> {
    let job = &unit.job;
    let target = job.remote_target();
    let (flags, attrs) = if job.segments > 1 {
        prepare_remote(ch, job).await?;
        (OpenFlags::WRITE, FileAttributes::empty())
    } else {
        (
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            perm_attrs(job.spec.mode),
        )
    };
    let handle = ch
        .sftp
        .open(target.as_str(), flags, attrs)
        .await
        .map_err(|e| format!("open '{target}' for writing: {e}"))?
        .handle;
    let result = pump_upload(ch, &handle, unit).await;
    let closed = ch.sftp.close(handle).await;
    result?;
    closed
        .map(|_| ())
        .map_err(|e| format!("finish '{target}': {e}"))
}

async fn pump_upload(ch: &Channel, handle: &str, unit: &Unit) -> Result<(), String> {
    let job = &unit.job;
    let local = &job.spec.local;
    let mut file = tokio::fs::File::open(local)
        .await
        .map_err(|e| format!("open '{}': {e}", local.display()))?;
    if unit.start > 0 {
        file.seek(SeekFrom::Start(unit.start))
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut reader = BufReader::with_capacity(LOCAL_BUF, file);
    let chunk = ch.write_len;
    let max_inflight = inflight_requests(chunk);
    let mut offset = unit.start;
    let mut inflight = FuturesUnordered::new();
    loop {
        while offset < unit.end && inflight.len() < max_inflight {
            if job.stopped() {
                return Err(CANCELLED.into());
            }
            let n = chunk.min(unit.end - offset);
            let mut buf = vec![0u8; n as usize];
            reader
                .read_exact(&mut buf)
                .await
                .map_err(|e| format!("read '{}': {e} (did it change?)", local.display()))?;
            let sftp = ch.sftp.clone();
            let handle = handle.to_string();
            let at = offset;
            inflight.push(async move { sftp.write(handle, at, buf).await.map(|_| n) });
            offset += n;
        }
        match inflight.next().await {
            None => return Ok(()),
            Some(Ok(n)) => job.progress(n),
            Some(Err(e)) => return Err(format!("write: {e}")),
        }
        if job.stopped() {
            return Err(CANCELLED.into());
        }
    }
}

async fn prepare_local(job: &Job) -> Result<(), String> {
    job.prepared
        .get_or_init(|| async {
            let target = job.local_target();
            let file = tokio::fs::File::create(&target)
                .await
                .map_err(|e| format!("create '{}': {e}", target.display()))?;
            file.set_len(job.spec.size)
                .await
                .map_err(|e| format!("allocate '{}': {e}", target.display()))
        })
        .await
        .clone()
}

async fn download_range(ch: &Channel, unit: &Unit) -> Result<(), String> {
    let job = &unit.job;
    let remote = job.spec.remote.as_str();
    let target = job.local_target();
    let mut file = if job.segments > 1 {
        prepare_local(job).await?;
        tokio::fs::OpenOptions::new()
            .write(true)
            .open(&target)
            .await
            .map_err(|e| format!("open '{}': {e}", target.display()))?
    } else {
        tokio::fs::File::create(&target)
            .await
            .map_err(|e| format!("create '{}': {e}", target.display()))?
    };
    if unit.start > 0 {
        file.seek(SeekFrom::Start(unit.start))
            .await
            .map_err(|e| e.to_string())?;
    }
    let handle = ch
        .sftp
        .open(remote, OpenFlags::READ, FileAttributes::empty())
        .await
        .map_err(|e| format!("open '{remote}': {e}"))?
        .handle;
    let mut writer = BufWriter::with_capacity(LOCAL_BUF, file);
    let result = pump_download(ch, &handle, unit, &mut writer).await;
    let _ = ch.sftp.close(handle).await;
    result?;
    writer
        .flush()
        .await
        .map_err(|e| format!("write '{}': {e}", target.display()))
}

async fn pump_download(
    ch: &Channel,
    handle: &str,
    unit: &Unit,
    writer: &mut BufWriter<tokio::fs::File>,
) -> Result<(), String> {
    let job = &unit.job;
    let chunk = ch.read_len;
    let max_inflight = inflight_requests(chunk);
    let read = |at: u64, len: u64| {
        let sftp = ch.sftp.clone();
        let handle = handle.to_string();
        async move { (at, len, sftp.read(handle, at, len as u32).await) }
    };
    let mut next = unit.start;
    let mut pending = FuturesOrdered::new();
    loop {
        while next < unit.end && pending.len() < max_inflight {
            let len = chunk.min(unit.end - next);
            pending.push_back(read(next, len));
            next += len;
        }
        let Some((at, len, result)) = pending.next().await else {
            return Ok(());
        };
        if job.stopped() {
            return Err(CANCELLED.into());
        }
        let mut pos = at;
        let mut result = result;
        // A short read (servers may cap the request size) leaves a gap before the
        // next queued response: fill it before writing on.
        loop {
            match result {
                Ok(data) if !data.data.is_empty() => {
                    let n = data.data.len() as u64;
                    writer
                        .write_all(&data.data)
                        .await
                        .map_err(|e| format!("write local file: {e}"))?;
                    job.progress(n);
                    pos += n;
                }
                Ok(_) => return Ok(()),
                Err(e) if is_status(&e, StatusCode::Eof) => return Ok(()),
                Err(e) => return Err(format!("read: {e}")),
            }
            if pos >= at + len {
                break;
            }
            result = read(pos, at + len - pos).await.2;
        }
    }
}

// ---------------------------------------------------------------------------
// Local helpers
// ---------------------------------------------------------------------------

struct LocalItem {
    /// Slash-joined path relative to the destination ("dir/sub/file.txt").
    rel: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mode: Option<u32>,
}

/// Expand local sources recursively, parents before children. Links to files
/// are followed; links to folders are skipped (they can loop).
fn walk_local(sources: &[String]) -> anyhow::Result<Vec<LocalItem>> {
    fn visit(dir: &Path, rel: &str, out: &mut Vec<LocalItem>) -> anyhow::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("read folder '{}'", dir.display()))?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let Ok(lmeta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let meta = if lmeta.file_type().is_symlink() {
                match std::fs::metadata(&path) {
                    Ok(m) if m.is_file() => m,
                    _ => continue,
                }
            } else {
                lmeta
            };
            let child_rel = format!("{rel}/{name}");
            if meta.is_dir() {
                out.push(LocalItem {
                    rel: child_rel.clone(),
                    path: path.clone(),
                    is_dir: true,
                    size: 0,
                    mode: mode_of(&meta),
                });
                visit(&path, &child_rel, out)?;
            } else if meta.is_file() {
                out.push(LocalItem {
                    rel: child_rel,
                    path,
                    is_dir: false,
                    size: meta.len(),
                    mode: mode_of(&meta),
                });
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    for source in sources {
        let path = PathBuf::from(source);
        let meta =
            std::fs::metadata(&path).with_context(|| format!("read '{}'", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("cannot upload '{}'", path.display()))?;
        if meta.is_dir() {
            out.push(LocalItem {
                rel: name.clone(),
                path: path.clone(),
                is_dir: true,
                size: 0,
                mode: mode_of(&meta),
            });
            visit(&path, &name, &mut out)?;
        } else {
            out.push(LocalItem {
                rel: name,
                path,
                is_dir: false,
                size: meta.len(),
                mode: mode_of(&meta),
            });
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}

fn local_mode(path: &Path) -> Option<u32> {
    std::fs::metadata(path).ok().and_then(|m| mode_of(&m))
}

#[cfg(unix)]
fn set_local_mode(path: &Path, mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        // Never produce a file the user cannot read or write back.
        let mode = (mode & 0o777) | 0o600;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
}

#[cfg(not(unix))]
fn set_local_mode(_path: &Path, _mode: Option<u32>) {}

struct RemoteItem {
    path: String,
    rel: String,
    is_dir: bool,
    size: u64,
    mode: Option<u32>,
}

impl RemoteItem {
    fn from_attrs(path: String, rel: String, attrs: &FileAttributes) -> Self {
        Self {
            path,
            rel,
            is_dir: attrs.kind_dir(),
            size: attrs.size.unwrap_or(0),
            mode: attrs.permissions.map(|p| p & 0o777),
        }
    }
}

/// A remote file name that is safe to use as a local path component — a hostile
/// server must not be able to steer a download outside the chosen folder.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !(cfg!(windows) && name.contains([':', '*', '?', '"', '<', '>', '|']))
}

// ---------------------------------------------------------------------------
// Path helpers (remote paths are always '/'-separated)
// ---------------------------------------------------------------------------

/// Join a remote directory and a name.
pub fn remote_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Split a remote path into (parent, name). The parent of a top-level entry is `/`.
pub fn remote_split(path: &str) -> (&str, &str) {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some((dir, name)) => (dir, name),
        None => ("", path),
    }
}

fn remote_under(dest: &str, rel: &str) -> String {
    if rel.is_empty() {
        dest.to_string()
    } else {
        remote_join(dest, rel)
    }
}

fn parent_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(p, _)| p)
}

fn leaf(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, n)| n)
}

/// "name (1).ext", "name (2).ext", … — the first one `taken` says is free.
/// Dotfiles and extension-less names get the suffix at the end.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|k| format!("{stem} ({k}){ext}"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range always yields a free name")
}

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

async fn report_loop(shared: std::sync::Weak<Shared>, updates: mpsc::Sender<Vec<TransferUpdate>>) {
    let mut tick = tokio::time::interval(REPORT_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let batch: Vec<TransferUpdate> = {
            let mut map = shared.jobs.lock().expect("jobs lock");
            let batch = map
                .values()
                .filter(|job| job.dirty.swap(false, Ordering::Relaxed))
                .map(|job| TransferUpdate {
                    id: job.spec.id,
                    state: job.state(),
                    done: job.done.load(Ordering::Relaxed),
                    total: job.spec.size,
                })
                .collect();
            // Completed jobs need no retry: drop them once reported.
            map.retain(|_, job| {
                job.state() != TransferState::Done || job.dirty.load(Ordering::Relaxed)
            });
            batch
        };
        drop(shared);
        if !batch.is_empty() && updates.send(batch).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_name_appends_a_counter_before_the_extension() {
        let taken = ["a (1).txt"];
        assert_eq!(free_name("a.txt", |n| taken.contains(&n)), "a (2).txt");
        assert_eq!(free_name("Makefile", |_| false), "Makefile (1)");
        assert_eq!(free_name(".env", |_| false), ".env (1)");
        assert_eq!(free_name("x.tar.gz", |_| false), "x.tar (1).gz");
    }

    #[test]
    fn remote_paths_split_and_join() {
        assert_eq!(
            remote_split("/var/www/index.html"),
            ("/var/www", "index.html")
        );
        assert_eq!(remote_split("/etc"), ("/", "etc"));
        assert_eq!(remote_join("/", "etc"), "/etc");
        assert_eq!(remote_join("/var", "log"), "/var/log");
        assert_eq!(remote_under("/srv", "a/b"), "/srv/a/b");
        assert_eq!(parent_rel("a/b/c"), "a/b");
        assert_eq!(parent_rel("a"), "");
        assert_eq!(leaf("a/b/c"), "c");
    }

    #[test]
    fn hostile_remote_names_are_rejected() {
        for bad in ["", ".", "..", "a/b", "a\\b", "x\0y"] {
            assert!(!safe_name(bad), "{bad:?} must be rejected");
        }
        assert!(safe_name("report.pdf"));
        assert!(safe_name("..hidden"));
    }

    #[test]
    fn big_files_split_into_even_segments_covering_every_byte() {
        assert_eq!(segments_for(10 * 1024 * 1024, 4), 1);
        assert_eq!(segments_for(SEGMENT_THRESHOLD, 4), 2);
        assert_eq!(
            segments_for(10 * 1024 * 1024 * 1024, 4),
            4 * CHANNELS_PER_LANE
        );
        assert_eq!(segments_for(10 * 1024 * 1024 * 1024, 1), CHANNELS_PER_LANE);

        let size = 1_000_000_007u64;
        let spec = TransferSpec {
            id: 1,
            direction: Direction::Upload,
            local: PathBuf::from("/tmp/x"),
            remote: "/tmp/x".into(),
            size,
            mode: None,
            in_place: false,
        };
        let job = Arc::new(Job::new(spec, segments_for(size, 4)));
        let units = split_units(&job);
        assert_eq!(units.len(), job.segments);
        assert_eq!(units[0].start, 0);
        assert_eq!(units.last().unwrap().end, size);
        for pair in units.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
    }

    #[test]
    fn empty_files_still_get_one_unit() {
        let spec = TransferSpec {
            id: 1,
            direction: Direction::Download,
            local: PathBuf::from("/tmp/empty"),
            remote: "/tmp/empty".into(),
            size: 0,
            mode: None,
            in_place: false,
        };
        let job = Arc::new(Job::new(spec, segments_for(0, 4)));
        let units = split_units(&job);
        assert_eq!(units.len(), 1);
        assert_eq!((units[0].start, units[0].end), (0, 0));
    }

    #[test]
    fn walk_local_lists_parents_before_children() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        std::fs::create_dir_all(root.join("css")).unwrap();
        std::fs::write(root.join("index.html"), b"<html>").unwrap();
        std::fs::write(root.join("css/app.css"), b"body{}").unwrap();
        let items = walk_local(&[root.to_string_lossy().into_owned()]).unwrap();
        let rels: Vec<&str> = items.iter().map(|i| i.rel.as_str()).collect();
        assert_eq!(
            rels,
            ["site", "site/css", "site/css/app.css", "site/index.html"]
        );
        assert_eq!(items[3].size, 6);
    }
}
