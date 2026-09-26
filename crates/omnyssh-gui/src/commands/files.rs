//! File-manager operations beyond SFTP's verbs: on the server (recursive delete,
//! archives, chmod, copy/move, new file) over an exec channel on the SFTP tab's
//! own connection, and on this machine (new folder/file, rename, move to Trash).

use std::time::Duration;

use omnyssh_core::ssh::remote_fs::{self, ArchiveFormat};
use tauri::State;

use crate::dto::{ArchiveFormatDto, LocalFsOpDto, RemoteFsOpDto};
use crate::error::CommandError;
use crate::state::GuiState;

/// Archives and recursive operations on big trees take time; the UI shows a
/// busy state meanwhile.
const REMOTE_OP_BUDGET: Duration = Duration::from_secs(30 * 60);

fn err(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
    }
}

/// Run a file operation on the server of SFTP tab `session_id`.
#[tauri::command]
#[specta::specta]
pub async fn remote_fs_op(
    state: State<'_, GuiState>,
    session_id: u64,
    op: RemoteFsOpDto,
) -> Result<(), CommandError> {
    let ssh = state
        .sftp_ssh(session_id)
        .ok_or_else(|| err("this SFTP session is closed"))?;
    let command = match &op {
        RemoteFsOpDto::Delete { paths } => remote_fs::delete_command(paths),
        RemoteFsOpDto::Compress {
            dir,
            names,
            archive,
            format,
        } => remote_fs::compress_command(
            dir,
            names,
            archive,
            match format {
                ArchiveFormatDto::TarGz => ArchiveFormat::TarGz,
                ArchiveFormatDto::Zip => ArchiveFormat::Zip,
            },
        ),
        RemoteFsOpDto::Extract { archive, dest } => remote_fs::extract_command(archive, dest),
        RemoteFsOpDto::Chmod {
            paths,
            mode,
            recursive,
        } => remote_fs::chmod_command(paths, *mode, *recursive),
        RemoteFsOpDto::NewFile { path } => remote_fs::new_file_command(path),
        RemoteFsOpDto::Copy { paths, dest } => remote_fs::copy_command(paths, dest),
        RemoteFsOpDto::Move { paths, dest } => remote_fs::move_command(paths, dest),
    }
    .map_err(|e| err(format!("{e:#}")))?;
    let out = ssh
        .run_script(&command, REMOTE_OP_BUDGET)
        .await
        .map_err(|e| err(format!("{e:#}")))?;
    if out.ok() {
        Ok(())
    } else {
        Err(err(out.error_text()))
    }
}

/// Run a file operation on this machine.
#[tauri::command]
#[specta::specta]
pub async fn local_fs_op(op: LocalFsOpDto) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(move || match op {
        LocalFsOpDto::Mkdir { path } => {
            std::fs::create_dir(&path).map_err(|e| err(format!("{path}: {e}")))
        }
        LocalFsOpDto::Rename { from, to } => {
            if std::path::Path::new(&to).exists() {
                return Err(err(format!("{to} already exists")));
            }
            std::fs::rename(&from, &to).map_err(|e| err(format!("{from}: {e}")))
        }
        LocalFsOpDto::NewFile { path } => std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map(|_| ())
            .map_err(|e| err(format!("{path}: {e}"))),
        LocalFsOpDto::Trash { paths } => move_to_trash(&paths),
    })
    .await
    .map_err(|e| err(e.to_string()))?
}

fn move_to_trash(paths: &[String]) -> Result<(), CommandError> {
    if paths.is_empty() {
        return Ok(());
    }
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    // The Finder route asks for Automation permission; the file-manager API needs
    // none and still supports "Put Back".
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx.delete_all(paths)
        .map_err(|e| err(format!("move to Trash: {e}")))
}
