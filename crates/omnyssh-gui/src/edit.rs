//! Editing remote files in an external editor.
//!
//! Opening a remote file downloads it into the app's cache (under its own name,
//! so the editor picks the right syntax), launches the editor, and starts
//! watching the copy. Once a save has settled, an `edit-sync` `modified` event
//! asks the user whether to upload it; a yes uploads the file back in place —
//! keeping the server file's owner, mode and links. If the server copy changed
//! since it was downloaded, nothing is overwritten: a `conflict` asks first.
//!
//! Watching polls size + mtime instead of using filesystem notifications: many
//! editors save by writing a new file and renaming it over the old one, which
//! silently ends an inotify/FSEvents watch on the original.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use omnyssh_core::event::SessionId;
use omnyssh_core::ssh::transfer::{
    remote_split, Direction, RemoteStat, TransferEngine, TransferSpec,
};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::dto::EditSyncStateDto;
use crate::events;
use crate::state::GuiState;

/// How often the local copies are checked for saves.
const POLL: Duration = Duration::from_millis(700);
/// A save is uploaded once the file has been stable this long, so an editor
/// writing in several steps is uploaded once, complete.
const SETTLE: Duration = Duration::from_millis(400);

type Fingerprint = (SystemTime, u64);

struct Entry {
    local: PathBuf,
    /// Last local size+mtime observed.
    seen: Option<Fingerprint>,
    /// The version last uploaded (or downloaded) — a different one is a save.
    synced: Option<Fingerprint>,
    /// When `seen` last changed.
    changed_at: Option<Instant>,
    /// The server copy as of our last download/upload; a mismatch is a conflict.
    baseline: Option<RemoteStat>,
    busy: bool,
    /// Waiting for the user: to confirm an upload, or to resolve a conflict.
    asking: bool,
}

/// Remote files of one SFTP tab that are open in an editor.
pub struct EditWatcher {
    session_id: SessionId,
    dir: PathBuf,
    entries: Mutex<HashMap<String, Entry>>,
    stopped: AtomicBool,
}

impl EditWatcher {
    /// Creates the watcher for `session_id` and starts its poll loop.
    pub fn start(app: &AppHandle, session_id: SessionId) -> Arc<Self> {
        let dir = cache_root(app).join(session_id.to_string());
        let watcher = Arc::new(Self {
            session_id,
            dir,
            entries: Mutex::new(HashMap::new()),
            stopped: AtomicBool::new(false),
        });
        tauri::async_runtime::spawn(poll_loop(app.clone(), Arc::downgrade(&watcher)));
        watcher
    }

    /// Stop syncing and remove this tab's local copies.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// The local copy for `remote`: `<cache>/<tab>/<hash of the remote folder>/<name>`.
    fn local_path(&self, remote: &str) -> PathBuf {
        let (dir, name) = remote_split(remote);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        dir.hash(&mut hasher);
        self.dir
            .join(format!("{:016x}", hasher.finish()))
            .join(sanitize(name))
    }

    /// Download `remote` (unless a copy is already being edited) and return the
    /// local path to open.
    pub async fn open(&self, engine: &TransferEngine, remote: &str) -> Result<PathBuf, String> {
        if let Some(entry) = self.entries.lock().expect("edit entries lock").get(remote) {
            if entry.local.exists() {
                return Ok(entry.local.clone());
            }
        }
        let stat = engine
            .stat_remote(remote)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("'{remote}' no longer exists"))?;
        if stat.is_dir {
            return Err(format!("'{remote}' is a folder"));
        }
        let local = self.local_path(remote);
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("create '{}': {e}", parent.display()))?;
        }
        download(engine, remote, &local, stat.size).await?;
        let fingerprint = fingerprint(&local);
        self.entries.lock().expect("edit entries lock").insert(
            remote.to_string(),
            Entry {
                local: local.clone(),
                seen: fingerprint,
                synced: fingerprint,
                changed_at: None,
                baseline: Some(stat),
                busy: false,
                asking: false,
            },
        );
        Ok(local)
    }

    /// Resolve a conflict: push the local copy over the server's (`overwrite`),
    /// or replace the local copy with the server's version.
    pub async fn resolve(&self, app: &AppHandle, remote: &str, overwrite: bool) {
        {
            let mut entries = self.entries.lock().expect("edit entries lock");
            let Some(entry) = entries.get_mut(remote) else {
                return;
            };
            if entry.busy {
                return;
            }
            entry.busy = true;
            entry.asking = false;
        }
        let Some(engine) = engine_for(app, self.session_id) else {
            return;
        };
        if overwrite {
            self.upload(app, &engine, remote).await;
        } else {
            self.reload(app, &engine, remote).await;
        }
    }

    /// The user answered "upload this save?". Yes checks the server copy is still
    /// the one we started from and uploads; no keeps this version local-only — the
    /// next save asks again.
    pub async fn confirm(&self, app: &AppHandle, remote: &str, upload: bool) {
        {
            let mut entries = self.entries.lock().expect("edit entries lock");
            let Some(entry) = entries.get_mut(remote) else {
                return;
            };
            if entry.busy || !entry.asking {
                return;
            }
            entry.asking = false;
            if !upload {
                entry.synced = entry.seen;
                return;
            }
            entry.busy = true;
        }
        if let Some(engine) = engine_for(app, self.session_id) {
            self.sync(app, &engine, remote).await;
        }
    }

    /// Replace the local copy with the server's current version.
    async fn reload(&self, app: &AppHandle, engine: &TransferEngine, remote: &str) {
        let Some(local) = self.local_of(remote) else {
            return;
        };
        let result = async {
            let stat = engine
                .stat_remote(remote)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("'{remote}' no longer exists"))?;
            download(engine, remote, &local, stat.size).await?;
            Ok::<_, String>(stat)
        }
        .await;
        let mut entries = self.entries.lock().expect("edit entries lock");
        let Some(entry) = entries.get_mut(remote) else {
            return;
        };
        entry.busy = false;
        match result {
            Ok(stat) => {
                let fp = fingerprint(&local);
                (entry.seen, entry.synced, entry.baseline) = (fp, fp, Some(stat));
                emit(app, self.session_id, remote, EditSyncStateDto::Synced, None);
            }
            Err(e) => emit(
                app,
                self.session_id,
                remote,
                EditSyncStateDto::Failed,
                Some(e),
            ),
        }
    }

    fn local_of(&self, remote: &str) -> Option<PathBuf> {
        self.entries
            .lock()
            .expect("edit entries lock")
            .get(remote)
            .map(|e| e.local.clone())
    }

    /// Saves that have settled and need the user's go-ahead (marks them asking).
    fn due(&self) -> Vec<String> {
        let now = Instant::now();
        let mut due = Vec::new();
        let mut entries = self.entries.lock().expect("edit entries lock");
        for (remote, entry) in entries.iter_mut() {
            if entry.busy || entry.asking {
                continue;
            }
            let current = fingerprint(&entry.local);
            if current.is_none() {
                continue; // Mid-save (renamed away) or deleted: look again later.
            }
            if current != entry.seen {
                entry.seen = current;
                entry.changed_at = Some(now);
                continue;
            }
            let settled = entry
                .changed_at
                .is_some_and(|at| now.duration_since(at) >= SETTLE);
            if settled && current != entry.synced {
                entry.changed_at = None;
                entry.asking = true;
                due.push(remote.clone());
            }
        }
        due
    }

    /// Check the server copy is still the one we started from, then upload.
    async fn sync(&self, app: &AppHandle, engine: &TransferEngine, remote: &str) {
        let baseline = self
            .entries
            .lock()
            .expect("edit entries lock")
            .get(remote)
            .and_then(|e| e.baseline);
        match engine.stat_remote(remote).await {
            Ok(now) if now.is_some() && now != baseline => {
                if let Some(entry) = self
                    .entries
                    .lock()
                    .expect("edit entries lock")
                    .get_mut(remote)
                {
                    entry.busy = false;
                    entry.asking = true;
                }
                emit(
                    app,
                    self.session_id,
                    remote,
                    EditSyncStateDto::Conflict,
                    None,
                );
            }
            Ok(_) => self.upload(app, engine, remote).await,
            Err(e) => {
                if let Some(entry) = self
                    .entries
                    .lock()
                    .expect("edit entries lock")
                    .get_mut(remote)
                {
                    entry.busy = false;
                }
                emit(
                    app,
                    self.session_id,
                    remote,
                    EditSyncStateDto::Failed,
                    Some(e.to_string()),
                );
            }
        }
    }

    async fn upload(&self, app: &AppHandle, engine: &TransferEngine, remote: &str) {
        let Some(local) = self.local_of(remote) else {
            return;
        };
        let uploaded = fingerprint(&local);
        emit(
            app,
            self.session_id,
            remote,
            EditSyncStateDto::Uploading,
            None,
        );
        let result = async {
            let size = uploaded.map(|(_, len)| len).unwrap_or(0);
            let (_, done) = engine.enqueue_file(TransferSpec {
                id: 0,
                direction: Direction::Upload,
                local: local.clone(),
                remote: remote.to_string(),
                size,
                mode: None,
                in_place: true,
            });
            done.await
                .map_err(|_| "the upload was dropped".to_string())??;
            engine.stat_remote(remote).await.map_err(|e| e.to_string())
        }
        .await;
        let mut entries = self.entries.lock().expect("edit entries lock");
        let Some(entry) = entries.get_mut(remote) else {
            return;
        };
        entry.busy = false;
        match result {
            Ok(stat) => {
                entry.synced = uploaded;
                entry.baseline = stat;
                emit(app, self.session_id, remote, EditSyncStateDto::Synced, None);
            }
            Err(e) => {
                // Leave `synced` behind so the next save retries the upload.
                emit(
                    app,
                    self.session_id,
                    remote,
                    EditSyncStateDto::Failed,
                    Some(e),
                );
            }
        }
    }
}

async fn poll_loop(app: AppHandle, watcher: Weak<EditWatcher>) {
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let Some(watcher) = watcher.upgrade() else {
            return;
        };
        if watcher.stopped.load(Ordering::Relaxed) {
            return;
        }
        for remote in watcher.due() {
            emit(
                &app,
                watcher.session_id,
                &remote,
                EditSyncStateDto::Modified,
                None,
            );
        }
    }
}

fn engine_for(app: &AppHandle, session_id: SessionId) -> Option<Arc<TransferEngine>> {
    app.state::<GuiState>().transfer_engine(session_id)
}

async fn download(
    engine: &TransferEngine,
    remote: &str,
    local: &Path,
    size: u64,
) -> Result<(), String> {
    let (_, done) = engine.enqueue_file(TransferSpec {
        id: 0,
        direction: Direction::Download,
        local: local.to_path_buf(),
        remote: remote.to_string(),
        size,
        mode: None,
        in_place: false,
    });
    done.await
        .map_err(|_| "the download was dropped".to_string())?
}

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn emit(
    app: &AppHandle,
    session_id: SessionId,
    remote: &str,
    state: EditSyncStateDto,
    error: Option<String>,
) {
    let _ = events::EditSync {
        session_id,
        remote_path: remote.to_string(),
        state,
        error,
    }
    .emit(app);
}

/// Where local copies of edited remote files live.
pub fn cache_root(app: &AppHandle) -> PathBuf {
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("omnyssh"))
        .join("edit")
}

/// A remote name as a local file name: path separators and characters Windows
/// rejects are replaced, so the copy always lands inside its folder.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    match cleaned.as_str() {
        "" | "." | ".." => "file".to_string(),
        _ => cleaned,
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn remote_names_become_safe_local_names() {
        assert_eq!(sanitize("nginx.conf"), "nginx.conf");
        assert_eq!(sanitize(".."), "file");
        assert_eq!(sanitize("a/b"), "a_b");
        assert_eq!(sanitize("c:d?.txt"), "c_d_.txt");
    }
}
