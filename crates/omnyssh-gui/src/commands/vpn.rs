//! OpenVPN through Tunnelblick (macOS): whether it is installed, installing it,
//! its configurations, and bringing one up or down.

use std::path::PathBuf;

use omnyssh_core::vpn::{self, InstallStage};
use tauri::AppHandle;
use tauri_specta::Event;

use crate::dto::{VpnInstallStageDto, VpnStatusDto};
use crate::error::CommandError;
use crate::events;

fn err(e: impl std::fmt::Display) -> CommandError {
    CommandError {
        message: e.to_string(),
    }
}

#[tauri::command]
#[specta::specta]
pub fn vpn_status() -> Result<VpnStatusDto, CommandError> {
    Ok(VpnStatusDto {
        supported: vpn::supported(),
        installed: vpn::tunnelblick_installed(),
        installs_version: vpn::TUNNELBLICK_VERSION.to_string(),
    })
}

/// Tunnelblick's configuration names.
#[tauri::command]
#[specta::specta]
pub async fn vpn_configurations() -> Result<Vec<String>, CommandError> {
    vpn::configurations()
        .await
        .map_err(|e| err(format!("{e:#}")))
}

/// Hand an .ovpn/.conf/.tblk file to Tunnelblick to install.
#[tauri::command]
#[specta::specta]
pub async fn vpn_import(path: String) -> Result<(), CommandError> {
    vpn::import_configuration(&PathBuf::from(path))
        .await
        .map_err(|e| err(format!("{e:#}")))
}

/// Tunnelblick's state for a configuration (`CONNECTED`, `EXITING`, …).
#[tauri::command]
#[specta::specta]
pub async fn vpn_state(name: String) -> Result<String, CommandError> {
    vpn::state(&name).await.map_err(|e| err(format!("{e:#}")))
}

#[tauri::command]
#[specta::specta]
pub async fn vpn_connect(name: String) -> Result<(), CommandError> {
    vpn::ensure_up(&name)
        .await
        .map_err(|e| err(format!("{e:#}")))
}

#[tauri::command]
#[specta::specta]
pub async fn vpn_disconnect(name: String) -> Result<(), CommandError> {
    vpn::disconnect(&name)
        .await
        .map_err(|e| err(format!("{e:#}")))
}

/// Download, verify and install Tunnelblick; progress arrives as
/// `vpn-install-progress`. Resolves when it is installed (and launched).
#[tauri::command]
#[specta::specta]
pub async fn vpn_install(app: AppHandle) -> Result<(), CommandError> {
    let emit = |stage: VpnInstallStageDto, done: u64, total: u64, error: Option<String>| {
        let _ = events::VpnInstallProgress {
            stage,
            done,
            total,
            error,
        }
        .emit(&app);
    };
    let result = vpn::install_tunnelblick(|stage| match stage {
        InstallStage::Downloading { done, total } => {
            emit(VpnInstallStageDto::Downloading, done, total, None)
        }
        InstallStage::Verifying => emit(VpnInstallStageDto::Verifying, 0, 0, None),
        InstallStage::Installing => emit(VpnInstallStageDto::Installing, 0, 0, None),
    })
    .await;
    match result {
        Ok(_) => {
            emit(VpnInstallStageDto::Done, 0, 0, None);
            Ok(())
        }
        Err(e) => {
            let message = format!("{e:#}");
            emit(VpnInstallStageDto::Failed, 0, 0, Some(message.clone()));
            Err(err(message))
        }
    }
}
