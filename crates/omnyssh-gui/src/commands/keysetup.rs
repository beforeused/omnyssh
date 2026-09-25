//! SSH key commands: finding the user's keys, the app-wide default key, and key
//! setup (tech-gui.md §4.2). `start_key_setup` connects with whatever the host
//! already logs in with, installs a new or an existing key, and — per the chosen
//! mode — turns password logins off (key only) or makes sure they stay on (key and
//! password). Progress and the outcome arrive as `key-setup-*` events; on success
//! the key is written onto the host in `hosts.toml` (an `~/.ssh/config` import is
//! adopted first, as editing does), so a `reload_hosts` shows it on the card.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;
use tokio::sync::mpsc;

use omnyssh_core::config::app_config::{load_app_config, save_default_identity};
use omnyssh_core::config::{load_hosts, save_hosts};
use omnyssh_core::ssh::client::{Host, HostSource};
use omnyssh_core::ssh::key_setup::{
    setup_key_with_options, validate_key_file_name, AuthMode, KeySetupOptions, KeySetupState,
    KeySetupStep, KeySource, KeyType,
};
use omnyssh_core::ssh::keys::{discover_keys, inspect_key};
use omnyssh_core::ssh::session::{set_default_identity, SshSession};

use crate::dto::{AuthModeDto, HostAuthDto, KeyChoiceDto, SshKeyDto};
use crate::error::CommandError;
use crate::events;
use crate::state::GuiState;

fn err(e: impl std::fmt::Display) -> CommandError {
    CommandError {
        message: e.to_string(),
    }
}

/// The private keys in `~/.ssh`, for the key pickers.
#[tauri::command]
#[specta::specta]
pub async fn list_ssh_keys() -> Result<Vec<SshKeyDto>, CommandError> {
    tauri::async_runtime::spawn_blocking(|| discover_keys().iter().map(SshKeyDto::from).collect())
        .await
        .map_err(err)
}

/// Describe a key file the user picked by hand. Errors when it is not a private key.
#[tauri::command]
#[specta::specta]
pub async fn inspect_ssh_key(path: String) -> Result<SshKeyDto, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_key(&PathBuf::from(&path))
            .map(|k| SshKeyDto::from(&k))
            .ok_or_else(|| err(format!("{path} is not an SSH private key")))
    })
    .await
    .map_err(err)?
}

/// The app-wide default key (used by hosts that name none), if set.
#[tauri::command]
#[specta::specta]
pub async fn get_default_key() -> Result<Option<String>, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        load_app_config(None).map(|c| c.general.default_identity_file)
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// Set (or clear, with `None`) the app-wide default key. Takes effect for new
/// connections straight away.
#[tauri::command]
#[specta::specta]
pub async fn set_default_key(path: Option<String>) -> Result<(), CommandError> {
    let path = path.filter(|p| !p.trim().is_empty());
    let saved = path.clone();
    tauri::async_runtime::spawn_blocking(move || save_default_identity(saved))
        .await
        .map_err(err)?
        .map_err(err)?;
    set_default_identity(path);
    Ok(())
}

/// Which key a host uses and whether it has a stored password — for the host form
/// and the key dialog. Never the password or key material.
#[tauri::command]
#[specta::specta]
pub fn host_auth(
    state: State<'_, GuiState>,
    host_name: String,
) -> Result<HostAuthDto, CommandError> {
    let host = state
        .host_by_name(&host_name)
        .ok_or_else(|| err(format!("unknown host '{host_name}'")))?;
    Ok(HostAuthDto {
        identity_file: host.identity_file.clone(),
        has_password: host.password.as_deref().is_some_and(|p| !p.is_empty()),
    })
}

/// Install a key on `host_name` (tech-gui.md §4.2): a new one or `key`, with
/// logins afterwards per `mode`. Fire-and-forget; reports via `key-setup-*` events.
/// One run at a time, so two runs never race a `hosts.toml` write.
#[tauri::command]
#[specta::specta]
pub fn start_key_setup(
    app: AppHandle,
    state: State<'_, GuiState>,
    host_name: String,
    key: KeyChoiceDto,
    mode: AuthModeDto,
) -> Result<(), CommandError> {
    let host = state
        .host_by_name(&host_name)
        .ok_or_else(|| err(format!("unknown host '{host_name}'")))?;
    let source = match key {
        KeyChoiceDto::Generate { name } => {
            let file_name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            if let Some(n) = &file_name {
                validate_key_file_name(n).map_err(|e| err(format!("{e:#}")))?;
            }
            KeySource::Generate { file_name }
        }
        KeyChoiceDto::Existing { path } => KeySource::Existing(expand_tilde(&path)),
    };
    let options = KeySetupOptions {
        key_type: KeyType::Ed25519,
        source,
        mode: mode.into(),
    };
    state
        .try_begin_key_setup(&host_name)
        .map_err(|message| CommandError { message })?;
    tauri::async_runtime::spawn(run_key_setup(app, host, options));
    Ok(())
}

fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(rest)),
        None => PathBuf::from(path),
    }
}

/// Releases the single key-setup slot on drop, so it frees on every exit of the spawned
/// task — a normal outcome or a panic in the core connect/setup path — and never wedges
/// all future key-setups (§4.2).
struct KeySetupSlot(AppHandle);

impl Drop for KeySetupSlot {
    fn drop(&mut self) {
        self.0.state::<GuiState>().end_key_setup();
    }
}

/// The background flow: forward each step as `key-setup-progress`, connect, run the
/// setup, persist the key, then emit exactly one terminal event.
async fn run_key_setup(app: AppHandle, host: Host, options: KeySetupOptions) {
    // Frees the slot on any return path, including an unwind (see `KeySetupSlot`).
    let _slot = KeySetupSlot(app.clone());

    let (progress_tx, mut progress_rx) = mpsc::channel::<KeySetupStep>(8);
    let step_app = app.clone();
    let step_host = host.name.clone();
    tokio::spawn(async move {
        while let Some(step) = progress_rx.recv().await {
            let _ = events::KeySetupProgress {
                host_name: step_host.clone(),
                step: step.into(),
            }
            .emit(&step_app);
        }
    });

    let failed = |error: String| {
        let _ = events::KeySetupFailed {
            host_name: host.name.clone(),
            error,
        }
        .emit(&app);
    };

    let session = match SshSession::connect(&host).await {
        Ok(session) => session,
        Err(e) => return failed(format!("Connection failed: {e:#}")),
    };
    let outcome = setup_key_with_options(&host, &session, &options, Some(progress_tx)).await;
    session.disconnect().await;

    match outcome {
        Ok(result)
            if matches!(
                result.state,
                KeySetupState::Success | KeySetupState::PartialSuccess
            ) =>
        {
            let partial =
                options.mode == AuthMode::KeyOnly && result.state == KeySetupState::PartialSuccess;
            // Persist BEFORE emitting so the frontend's reload sees it.
            persist_key(
                &app,
                &host,
                &result.key_path.to_string_lossy(),
                result.password_auth_disabled,
            )
            .await;
            let _ = events::KeySetupComplete {
                host_name: host.name.clone(),
                key_path: result.key_path.to_string_lossy().into_owned(),
                password_auth_disabled: result.password_auth_disabled,
                partial,
            }
            .emit(&app);
        }
        Ok(result) if result.state == KeySetupState::RolledBack => {
            let _ = events::KeySetupRollback {
                host_name: host.name.clone(),
                result: result
                    .error_message
                    .unwrap_or_else(|| "Rolled back.".to_string()),
            }
            .emit(&app);
        }
        Ok(result) => failed(
            result
                .error_message
                .unwrap_or_else(|| "Key setup failed.".to_string()),
        ),
        Err(e) => failed(format!("{e:#}")),
    }
}

/// Write the installed key onto the host in `hosts.toml`: set `identity_file` and
/// `key_setup_date`, and record the password state when it is known — dropping the
/// stored password once the server refuses passwords. An `~/.ssh/config` import is
/// adopted into `hosts.toml` first (the same copy editing it would make), so its
/// key is not lost. Best-effort: the completion event fires regardless.
async fn persist_key(
    app: &AppHandle,
    host: &Host,
    key_path: &str,
    password_disabled: Option<bool>,
) {
    let imported = (host.source == HostSource::SshConfig).then(|| host.clone());
    let name = host.name.clone();
    let key_path = key_path.to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || -> Option<()> {
        let mut hosts = load_hosts().ok()?;
        if !hosts.iter().any(|h| h.name == name) {
            let mut adopted = imported?;
            adopted.source = HostSource::Manual;
            adopted.original_ssh_host = Some(adopted.name.clone());
            hosts.push(adopted);
        }
        let host = hosts.iter_mut().find(|h| h.name == name)?;
        host.identity_file = Some(key_path);
        host.key_setup_date = Some(chrono::Utc::now().to_rfc3339());
        match password_disabled {
            Some(true) => {
                host.password_auth_disabled = Some(true);
                host.password = None;
            }
            Some(false) => host.password_auth_disabled = Some(false),
            None => {}
        }
        save_hosts(&hosts).ok()
    })
    .await;
    // The cached host list feeds connects until the frontend's reload lands.
    if let Ok(hosts) = omnyssh_core::config::load_all_hosts() {
        app.state::<GuiState>().set_hosts(hosts);
    }
}
