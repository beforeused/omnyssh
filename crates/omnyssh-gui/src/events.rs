//! Typed IPC events (tech-gui.md §4.3). Each `tauri_specta::Event` derives its
//! wire name from the struct name (`HostsLoaded` -> "hosts-loaded",
//! `MetricsUpdated` -> "metrics-updated"), so the frontend consumes them through
//! the generated `events` object.

use serde::{Deserialize, Serialize};

use crate::dto::{
    ConnectionStatusDto, EditSyncStateDto, FileEntryDto, HostDto, KeySetupStepDto, MetricsDto,
    ServiceDto, TransferUpdateDto, UpdateInfoDto,
};

/// Full host list broadcast. Emitted by `reload_hosts` after refreshing the
/// cache; the bridge does not map `HostsLoaded` (tech-gui.md §3.4).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct HostsLoaded(pub Vec<HostDto>);

/// A host's connection status changed (tech-gui.md §4.3).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct HostStatusChanged {
    pub host_name: String,
    pub status: ConnectionStatusDto,
}

/// A fresh metrics sample for a host (tech-gui.md §4.3).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MetricsUpdated {
    pub host_name: String,
    pub metrics: MetricsDto,
}

/// Services detected on a host by the discovery quick-scan (tech-gui.md §4.3).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServicesDetected {
    pub host_name: String,
    pub services: Vec<ServiceDto>,
}

/// Discovery failed for a host (tech-gui.md §4.3).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServicesFailed {
    pub host_name: String,
    pub message: String,
}

/// Result of running a snippet on one host (tech-gui.md §4.3). Emitted directly by
/// `execute_snippet` per host (one-shot `SshSession::run_command`), not via the
/// shared bridge — the same "the command owns the result" pattern the SFTP
/// per-session forwarder uses (§3.4). The core's `Result<String, String>` is
/// flattened to `ok` + `output` (stdout on success, the error message on failure).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct SnippetResult {
    pub host_name: String,
    pub snippet_name: String,
    pub ok: bool,
    pub output: String,
}

/// A terminal session's remote shell exited or its connection dropped (tech-gui.md
/// §4.3). Carries the **public** registry id (the bridge maps the core's inner PTY
/// id, §3.4); the frontend tears the tab down. User-initiated closes never emit this.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExited {
    pub session_id: u64,
}

/// An SFTP session connected (tech-gui.md §4.3). The per-session forwarder stamps
/// `sessionId` from the channel's owner — the core `SftpConnected` carries only the
/// host name (§3.4).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct SftpConnected {
    pub session_id: u64,
    pub host_name: String,
}

/// A remote directory listing completed for one SFTP tab (tech-gui.md §4.3). Stamped
/// with `sessionId`; the core `FileDirListed` carries only the path (§3.4).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct SftpDirListed {
    pub session_id: u64,
    pub path: String,
    pub entries: Vec<FileEntryDto>,
}

/// A mutating SFTP op (upload/download/mkdir/rename/delete) finished (tech-gui.md
/// §4.3). The core's `Result<(), String>` is flattened to `ok` + an optional `error`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct SftpOpDone {
    pub session_id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// An SFTP operation reported a failure (tech-gui.md §4.3). The core emits this on a
/// listing error with a human reason; it does not by itself tear the session down.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct SftpDisconnected {
    pub session_id: u64,
    pub reason: String,
}

/// Preview bytes for a remote file (tech-gui.md §4.3). Stamped with `sessionId`; the
/// core `FilePreviewReady` carries only the path + content (§3.4).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub session_id: u64,
    pub path: String,
    pub content: String,
}

/// Batched progress for one SFTP tab's transfer queue. The engine reports a few
/// times a second, carrying only the transfers that changed since the last batch.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct TransfersUpdated {
    pub session_id: u64,
    pub updates: Vec<TransferUpdateDto>,
}

/// A remote file open in an external editor was saved locally and is being (or
/// was) pushed back, or needs the user because the server copy changed too.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct EditSync {
    pub session_id: u64,
    pub remote_path: String,
    pub state: EditSyncStateDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A progress step of an auto key-setup run (tech-gui.md §4.3). Mapped by the shared
/// engine bridge from `CoreEvent::KeySetupProgress`; the host name identifies the run.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct KeySetupProgress {
    pub host_name: String,
    pub step: KeySetupStepDto,
}

/// Key setup finished successfully — key auth is configured (tech-gui.md §4.3).
/// `keyPath` is the generated private-key path (a path, never key material, §3.4).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct KeySetupComplete {
    pub host_name: String,
    pub key_path: String,
    /// Whether the server now refuses password logins, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_auth_disabled: Option<bool>,
    /// Key-only was asked for but the key could only be installed (no sudo to
    /// turn passwords off).
    pub partial: bool,
}

/// Key setup failed before touching the server's auth config (tech-gui.md §4.3).
/// Password authentication is never disabled unless a key was verified first.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct KeySetupFailed {
    pub host_name: String,
    pub error: String,
}

/// Key setup rolled the server's sshd config back after a late failure (tech-gui.md
/// §4.3). `result` is the human-readable rollback outcome.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct KeySetupRollback {
    pub host_name: String,
    pub result: String,
}

/// A newer release was found by the startup check (tech-gui.md §4.3). Mapped by the
/// shared engine bridge from `CoreEvent::UpdateAvailable`; drives the update banner.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAvailable {
    pub info: UpdateInfoDto,
}

/// A background error surfaced to the user.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct Error {
    pub message: String,
}
