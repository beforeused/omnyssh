//! SFTP session commands (tech-gui.md §4.2). `sftp_open` awaits the core connect and
//! spawns a per-session forwarder that stamps every `sftp-*` event with the tab's
//! session id (§3.4); the remote ops are thin `SftpCommand` enqueues whose results
//! arrive as events. Uploads and downloads go through the tab's transfer engine
//! (`commands::transfer`) instead, so they never hold up browsing. Local filesystem listing/preview return directly — the GUI never
//! emits `LocalDirListed` (§4.3).

use tauri::{AppHandle, State};
use tokio::sync::mpsc;

use omnyssh_core::event::CoreEvent;
use omnyssh_core::ssh::sftp::{
    list_local_dir as core_list_local_dir, preview_local_file as core_preview_local_file,
    SftpCommand, SftpManager,
};
use omnyssh_core::ssh::transfer::TransferEngine;

use crate::bridge;
use crate::dto::FileEntryDto;
use crate::error::CommandError;
use crate::state::GuiState;

/// A tab's dedicated core-event channel buffer. Comfortably absorbs the connect ack
/// plus a burst of listings and op acks before the forwarder drains them (§3.4).
const SFTP_EVENT_BUFFER: usize = 256;

/// Batched transfer-progress buffer. The engine sends one batch per ~150 ms, so a
/// handful of slots is plenty; a full buffer only delays its reporter.
const TRANSFER_UPDATE_BUFFER: usize = 16;

/// Open an SFTP session for `host_name` (tech-gui.md §4.2). Awaits the core connect,
/// registers the manager under a fresh public id, and spawns the per-session
/// forwarder; the `sftp-connected` ack then arrives stamped with that id (§3.4).
#[tauri::command]
#[specta::specta]
pub async fn sftp_open(
    app: AppHandle,
    state: State<'_, GuiState>,
    host_name: String,
) -> Result<u64, CommandError> {
    let host = state.host_by_name(&host_name).ok_or_else(|| CommandError {
        message: format!("unknown host '{host_name}'"),
    })?;
    // A dedicated channel per tab: its owner is the session id, so the forwarder can
    // attribute the core's session-less `sftp-*` events to this tab (§3.4).
    let (tx, rx) = mpsc::channel::<CoreEvent>(SFTP_EVENT_BUFFER);
    let manager = SftpManager::connect(&host, tx)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?;
    // The tab's transfer engine plans over the browsing connection and moves data
    // over its own lanes, connected on the first transfer.
    let (updates_tx, updates_rx) = mpsc::channel(TRANSFER_UPDATE_BUFFER);
    let engine = TransferEngine::new(
        host,
        manager.ssh_session(),
        state.transfer_streams(),
        updates_tx,
    );
    let session_id = state.register_sftp(manager, engine);
    tauri::async_runtime::spawn(bridge::forward_sftp_events(app.clone(), session_id, rx));
    tauri::async_runtime::spawn(bridge::forward_transfer_updates(
        app, session_id, updates_rx,
    ));
    Ok(session_id)
}

/// List a remote directory (tech-gui.md §4.2); the result arrives as `sftp-dir-listed`.
#[tauri::command]
#[specta::specta]
pub fn sftp_list(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::ListDir(path));
    Ok(())
}

/// Create a remote directory (tech-gui.md §4.2); completion arrives as `sftp-op-done`.
#[tauri::command]
#[specta::specta]
pub fn sftp_mkdir(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::MkDir(path));
    Ok(())
}

/// Rename / move a remote path (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub fn sftp_rename(
    state: State<'_, GuiState>,
    session_id: u64,
    from: String,
    to: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::Rename { from, to });
    Ok(())
}

/// Delete a remote file (falls back to an empty directory in the core) (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub fn sftp_delete(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::Delete(path));
    Ok(())
}

/// Read a remote file's preview bytes (tech-gui.md §4.2); arrives as `file-preview`.
#[tauri::command]
#[specta::specta]
pub fn sftp_preview(
    state: State<'_, GuiState>,
    session_id: u64,
    path: String,
) -> Result<(), CommandError> {
    state.send_sftp(session_id, SftpCommand::ReadPreview(path));
    Ok(())
}

/// Close an SFTP session and its connection (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub fn sftp_close(state: State<'_, GuiState>, session_id: u64) -> Result<(), CommandError> {
    state.close_sftp(session_id);
    Ok(())
}

/// List a local directory (tech-gui.md §4.2). Returns directly off the async worker;
/// the GUI never emits `LocalDirListed` (§4.3). The core prepends a `..` entry and
/// sorts dirs-first.
#[tauri::command]
#[specta::specta]
pub async fn list_local_dir(path: String) -> Result<Vec<FileEntryDto>, CommandError> {
    let entries = core_list_local_dir(&path).await.map_err(|e| CommandError {
        message: e.to_string(),
    })?;
    Ok(entries.iter().map(FileEntryDto::from).collect())
}

/// Read up to 4 KiB of a local file as UTF-8 for preview (tech-gui.md §4.2).
#[tauri::command]
#[specta::specta]
pub async fn preview_local_file(path: String) -> Result<String, CommandError> {
    core_preview_local_file(&path)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}
