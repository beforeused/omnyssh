//! Opening files in the user's editor. Local files open directly; remote files
//! are downloaded and opened, and each save is offered for upload (see `crate::edit`).

use tauri::{AppHandle, State};

use crate::dto::{EditorAppDto, EditorDto};
use crate::edit::EditWatcher;
use crate::editor;
use crate::error::CommandError;
use crate::state::GuiState;

/// Editors found installed on this machine, for the Settings picker.
#[tauri::command]
#[specta::specta]
pub async fn detect_editors() -> Result<Vec<EditorAppDto>, CommandError> {
    tauri::async_runtime::spawn_blocking(editor::detect)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })
}

/// Open a local file in `editor`.
#[tauri::command]
#[specta::specta]
pub fn open_local_file(path: String, editor: EditorDto) -> Result<(), CommandError> {
    editor::open(&editor, std::path::Path::new(&path)).map_err(|message| CommandError { message })
}

/// Download a remote file, open it in `editor`, and upload every save back.
/// Resolves once the editor was launched; sync progress arrives as `edit-sync`.
#[tauri::command]
#[specta::specta]
pub async fn edit_remote_file(
    app: AppHandle,
    state: State<'_, GuiState>,
    session_id: u64,
    remote_path: String,
    editor: EditorDto,
) -> Result<(), CommandError> {
    let engine = state
        .transfer_engine(session_id)
        .ok_or_else(|| CommandError {
            message: "this SFTP session is closed".into(),
        })?;
    let watcher = state.edit_watcher(session_id, || EditWatcher::start(&app, session_id));
    let local = watcher
        .open(&engine, &remote_path)
        .await
        .map_err(|message| CommandError { message })?;
    editor::open(&editor, &local).map_err(|message| CommandError { message })
}

/// Answer an `edit-sync` conflict: `overwrite` pushes the local copy over the
/// server's; otherwise the local copy is replaced with the server's version.
#[tauri::command]
#[specta::specta]
pub async fn edit_resolve_conflict(
    app: AppHandle,
    state: State<'_, GuiState>,
    session_id: u64,
    remote_path: String,
    overwrite: bool,
) -> Result<(), CommandError> {
    if let Some(watcher) = state.existing_edit_watcher(session_id) {
        watcher.resolve(&app, &remote_path, overwrite).await;
    }
    Ok(())
}

/// Answer an `edit-sync` `modified` prompt: upload this save, or keep it local.
#[tauri::command]
#[specta::specta]
pub async fn edit_confirm_upload(
    app: AppHandle,
    state: State<'_, GuiState>,
    session_id: u64,
    remote_path: String,
    upload: bool,
) -> Result<(), CommandError> {
    if let Some(watcher) = state.existing_edit_watcher(session_id) {
        watcher.confirm(&app, &remote_path, upload).await;
    }
    Ok(())
}
