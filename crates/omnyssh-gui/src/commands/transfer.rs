//! Transfer queue commands. A batch is two steps: `transfer_prepare` walks the
//! sources and reports name conflicts, the frontend asks the user about them, and
//! `transfer_commit` enqueues the batch with the answers. Progress arrives as
//! batched `transfers-updated` events; nothing here blocks on data moving.

use std::collections::HashMap;
use std::sync::Arc;

use omnyssh_core::ssh::transfer::{Resolution, TransferEngine};
use tauri::State;

use crate::dto::{
    ConflictResolutionDto, PreparedBatchDto, TransferConflictDto, TransferDirectionDto,
    TransferItemDto,
};
use crate::error::CommandError;
use crate::state::GuiState;

fn engine(state: &GuiState, session_id: u64) -> Result<Arc<TransferEngine>, CommandError> {
    state
        .transfer_engine(session_id)
        .ok_or_else(|| CommandError {
            message: "this SFTP session is closed".into(),
        })
}

fn command_error(e: impl std::fmt::Display) -> CommandError {
    CommandError {
        message: e.to_string(),
    }
}

/// Expand `sources` (local paths for an upload, remote paths for a download)
/// into a batch targeting `dest_dir`, listing every destination that already
/// exists. The batch waits for `transfer_commit` or `transfer_discard`.
#[tauri::command]
#[specta::specta]
pub async fn transfer_prepare(
    state: State<'_, GuiState>,
    session_id: u64,
    direction: TransferDirectionDto,
    sources: Vec<String>,
    dest_dir: String,
) -> Result<PreparedBatchDto, CommandError> {
    let engine = engine(&state, session_id)?;
    let (batch_id, plan) = engine
        .plan(direction.into(), sources, dest_dir)
        .await
        .map_err(|e| command_error(format!("{e:#}")))?;
    let conflicts = plan
        .files
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let c = f.conflict.as_ref()?;
            let destination = match direction {
                TransferDirectionDto::Upload => f.remote.clone(),
                TransferDirectionDto::Download => f.local.to_string_lossy().into_owned(),
            };
            Some(TransferConflictDto {
                index: i as u32,
                name: f.display.clone(),
                destination,
                source_size: f.size,
                existing_size: c.existing_size,
                existing_is_dir: c.existing_is_dir,
                alt_name: c.alt_name.clone(),
            })
        })
        .collect();
    Ok(PreparedBatchDto {
        batch_id,
        files: plan.files.len() as u32,
        bytes: plan.total_bytes(),
        conflicts,
    })
}

/// Enqueue a prepared batch. Conflicts without an answer are skipped. Returns
/// the enqueued transfers so the queue panel can list them straight away.
#[tauri::command]
#[specta::specta]
pub async fn transfer_commit(
    state: State<'_, GuiState>,
    session_id: u64,
    batch_id: u64,
    resolutions: Vec<ConflictResolutionDto>,
) -> Result<Vec<TransferItemDto>, CommandError> {
    let engine = engine(&state, session_id)?;
    let resolutions: HashMap<usize, Resolution> = resolutions
        .into_iter()
        .map(|r| (r.index as usize, r.action.into()))
        .collect();
    let specs = engine
        .commit(batch_id, &resolutions)
        .await
        .map_err(|e| command_error(format!("{e:#}")))?;
    Ok(specs.iter().map(TransferItemDto::from).collect())
}

/// Drop a prepared batch the user backed out of.
#[tauri::command]
#[specta::specta]
pub fn transfer_discard(
    state: State<'_, GuiState>,
    session_id: u64,
    batch_id: u64,
) -> Result<(), CommandError> {
    if let Some(engine) = state.transfer_engine(session_id) {
        engine.discard(batch_id);
    }
    Ok(())
}

/// Cancel queued or running transfers; running ones clean up after themselves.
#[tauri::command]
#[specta::specta]
pub fn transfer_cancel(
    state: State<'_, GuiState>,
    session_id: u64,
    ids: Vec<u64>,
) -> Result<(), CommandError> {
    engine(&state, session_id)?.cancel(&ids);
    Ok(())
}

/// Re-run failed or cancelled transfers.
#[tauri::command]
#[specta::specta]
pub fn transfer_retry(
    state: State<'_, GuiState>,
    session_id: u64,
    ids: Vec<u64>,
) -> Result<(), CommandError> {
    engine(&state, session_id)?.retry(&ids);
    Ok(())
}

/// Forget finished transfers the user cleared from the queue panel.
#[tauri::command]
#[specta::specta]
pub fn transfer_forget(
    state: State<'_, GuiState>,
    session_id: u64,
    ids: Vec<u64>,
) -> Result<(), CommandError> {
    if let Some(engine) = state.transfer_engine(session_id) {
        engine.forget(&ids);
    }
    Ok(())
}

/// Set how many parallel connections each host's transfers may use (1–8).
/// Applies to connections opened from now on.
#[tauri::command]
#[specta::specta]
pub fn set_transfer_streams(state: State<'_, GuiState>, streams: u32) -> Result<(), CommandError> {
    state.set_transfer_streams(streams as usize);
    Ok(())
}
