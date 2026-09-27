//! DTOs crossing the IPC boundary (tech-gui.md §4.1). Every type derives serde +
//! `specta::Type` so `bindings.ts` is generated, never hand-written. Secret
//! fields (`password`, key material) never appear here.

use serde::{Deserialize, Serialize};

use omnyssh_core::config::app_config::UpdateConfig;
use omnyssh_core::config::snippets::{Snippet, SnippetScope};
use omnyssh_core::event::{
    DetectedService, MetricValue, Metrics, ProcessInfo, ServiceKind, ServiceMetric,
};
use omnyssh_core::ssh::client::{ConnectionStatus, Host, HostSource, MonitorMode};
use omnyssh_core::ssh::key_setup::{AuthMode, KeySetupStep};
use omnyssh_core::ssh::keys::SshKeyInfo;
use omnyssh_core::ssh::sftp::FileEntry;
use omnyssh_core::ssh::transfer::{
    remote_split, Direction, Resolution, TransferSpec, TransferState, TransferUpdate,
};
use omnyssh_core::update::UpdateInfo;

/// Host origin, mirrors `omnyssh_core::ssh::client::HostSource`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum HostSourceDto {
    SshConfig,
    Manual,
}

/// How a host is watched, mirrors `omnyssh_core::ssh::client::MonitorMode`
/// (tech-gui.md §4.1). `tcpPort` means reachability only — no login, no metrics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum MonitorModeDto {
    Ssh,
    TcpPort,
}

impl From<MonitorMode> for MonitorModeDto {
    fn from(mode: MonitorMode) -> Self {
        match mode {
            MonitorMode::Ssh => Self::Ssh,
            MonitorMode::TcpPort => Self::TcpPort,
        }
    }
}

impl From<MonitorModeDto> for MonitorMode {
    fn from(mode: MonitorModeDto) -> Self {
        match mode {
            MonitorModeDto::Ssh => Self::Ssh,
            MonitorModeDto::TcpPort => Self::TcpPort,
        }
    }
}

/// A host as the frontend sees it — password and private-key material omitted
/// (tech-gui.md §3.4). `hasKey` reports whether an identity file is configured;
/// the key path itself never crosses the boundary.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HostDto {
    pub name: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub source: HostSourceDto,
    pub has_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_auth_disabled: Option<bool>,
    pub monitoring: MonitorModeDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_port: Option<u16>,
    /// Tunnelblick configuration brought up before connecting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vpn: Option<String>,
}

/// Inbound host form payload for `save_host` (tech-gui.md §4.1, Stage 4.1). Always
/// builds a **manual** `Host`: editing an SSH-config import saves a copy that shadows
/// it, and `~/.ssh/config` itself is never written.
/// `password`/`identityFile` arrive here (the create/edit form owns them) but never
/// travel back out: the outbound `HostDto` omits both (§3.4). Inbound only, so it
/// derives `Deserialize` (not `Serialize`).
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HostInputDto {
    pub name: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    #[serde(default)]
    pub identity_file: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub proxy_jump: Option<String>,
    /// Drop the stored identity file (the form's "default key / agent" choice).
    /// Without it, an absent `identityFile` keeps the stored one.
    #[serde(default)]
    pub clear_identity: Option<bool>,
    // `tags[]` is required on the wire (tech-gui.md §4.1); the form always sends an
    // array, so no `serde(default)` — that would emit an optional `tags?` and drift.
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub monitoring: Option<MonitorModeDto>,
    #[serde(default)]
    pub monitor_port: Option<u16>,
    /// Tunnelblick configuration to bring up first; absent means none.
    #[serde(default)]
    pub vpn: Option<String>,
}

/// Live connection state for a host (tech-gui.md §4.1). Internally tagged so the
/// frontend consumes a discriminated union keyed on `kind`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConnectionStatusDto {
    Unknown,
    Connecting,
    Connected,
    Failed { message: String },
}

/// A single process in the "top processes" panel (tech-gui.md §4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDto {
    pub name: String,
    pub cpu_percent: f64,
    pub mem_percent: f64,
}

/// A metrics snapshot for a host (tech-gui.md §4.1). The core's `Instant` is
/// flattened to `ageSeconds` (seconds since the sample) so it can serialise.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetricsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_avg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_info: Option<String>,
    pub top_processes: Vec<ProcessDto>,
    pub age_seconds: u64,
}

/// A service kind detected on a host, mirrors `omnyssh_core::event::ServiceKind`.
/// Wire names are lowercase (`docker`, `nginx`, `postgresql`, `redis`, `nodejs`);
/// if the core adds a kind, extend this enum so it is never silently dropped
/// (tech-gui.md §4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKindDto {
    Docker,
    Nginx,
    PostgreSQL,
    Redis,
    NodeJS,
}

/// One quick-scan metric for a detected service (tech-gui.md §4.1). `MetricValue`
/// is integer-only today; widen this if the core adds a non-integral variant.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServiceMetricDto {
    pub name: String,
    pub value: i64,
}

/// A service detected on a host with its quick-scan metrics (tech-gui.md §4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDto {
    pub kind: ServiceKindDto,
    pub metrics: Vec<ServiceMetricDto>,
}

/// Snippet scope, mirrors `omnyssh_core::config::snippets::SnippetScope`. Wire
/// names are lowercase (`global`, `host`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum SnippetScopeDto {
    Global,
    Host,
}

/// A saved command snippet as the frontend sees it (tech-gui.md §4.1). Crosses the
/// boundary both ways — outbound for `list_snippets`, inbound for `save_snippet` —
/// so it derives `Deserialize` too. Optional fields are omitted when absent, matching
/// the sparse `snippets.toml` the TUI writes.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SnippetDto {
    pub name: String,
    pub command: String,
    pub scope: SnippetScopeDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<String>>,
}

/// A file or directory in an SFTP panel listing (tech-gui.md §4.1). Maps from the
/// core `FileEntry`; `path` is the absolute path the frontend marks entries by.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FileEntryDto {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    /// Last modification, seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
    /// Permission bits (`0o7777` mask).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_link: bool,
}

/// Which way a transfer moves bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirectionDto {
    Upload,
    Download,
}

impl From<TransferDirectionDto> for Direction {
    fn from(d: TransferDirectionDto) -> Self {
        match d {
            TransferDirectionDto::Upload => Direction::Upload,
            TransferDirectionDto::Download => Direction::Download,
        }
    }
}

impl From<Direction> for TransferDirectionDto {
    fn from(d: Direction) -> Self {
        match d {
            Direction::Upload => TransferDirectionDto::Upload,
            Direction::Download => TransferDirectionDto::Download,
        }
    }
}

/// A destination that already exists, awaiting the user's choice. `index` keys
/// the answer back to the planned file; `altName` is the "keep both" name.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TransferConflictDto {
    pub index: u32,
    /// Path relative to the destination folder ("site/css/app.css").
    pub name: String,
    pub destination: String,
    pub source_size: u64,
    pub existing_size: u64,
    pub existing_is_dir: bool,
    pub alt_name: String,
}

/// An expanded transfer batch (folders walked) awaiting `transfer_commit`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PreparedBatchDto {
    pub batch_id: u64,
    pub files: u32,
    pub bytes: u64,
    pub conflicts: Vec<TransferConflictDto>,
}

/// How the user resolved one conflict.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ConflictActionDto {
    Replace,
    Skip,
    KeepBoth,
}

impl From<ConflictActionDto> for Resolution {
    fn from(a: ConflictActionDto) -> Self {
        match a {
            ConflictActionDto::Replace => Resolution::Replace,
            ConflictActionDto::Skip => Resolution::Skip,
            ConflictActionDto::KeepBoth => Resolution::KeepBoth,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ConflictResolutionDto {
    pub index: u32,
    pub action: ConflictActionDto,
}

/// One enqueued transfer, returned by `transfer_commit` so the queue panel can
/// list it before its first progress update lands.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TransferItemDto {
    pub id: u64,
    pub direction: TransferDirectionDto,
    pub name: String,
    pub local: String,
    pub remote: String,
    pub size: u64,
}

impl From<&TransferSpec> for TransferItemDto {
    fn from(spec: &TransferSpec) -> Self {
        let name = match spec.direction {
            Direction::Upload => remote_split(&spec.remote).1.to_string(),
            Direction::Download => spec
                .local
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        Self {
            id: spec.id,
            direction: spec.direction.into(),
            name,
            local: spec.local.to_string_lossy().into_owned(),
            remote: spec.remote.clone(),
            size: spec.size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum TransferStateDto {
    Queued,
    Running,
    /// The connection dropped; resuming once it is back.
    Reconnecting,
    Done,
    Failed,
    Cancelled,
}

/// Batched progress/state for one transfer (the engine reports ~7×/s).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TransferUpdateDto {
    pub id: u64,
    pub state: TransferStateDto,
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<&TransferUpdate> for TransferUpdateDto {
    fn from(u: &TransferUpdate) -> Self {
        let (state, error) = match &u.state {
            TransferState::Queued => (TransferStateDto::Queued, None),
            TransferState::Running => (TransferStateDto::Running, None),
            TransferState::Reconnecting => (TransferStateDto::Reconnecting, None),
            TransferState::Done => (TransferStateDto::Done, None),
            TransferState::Failed(e) => (TransferStateDto::Failed, Some(e.clone())),
            TransferState::Cancelled => (TransferStateDto::Cancelled, None),
        };
        Self {
            id: u.id,
            state,
            done: u.done,
            total: u.total,
            error,
        }
    }
}

/// Which program opens files (Settings → Files). Internally tagged like
/// `ConnectionStatusDto`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EditorDto {
    /// The OS default app for the file type.
    System,
    /// A specific application (a `.app` bundle on macOS, an executable elsewhere).
    App { name: String, path: String },
    /// A command line; `{file}` is replaced by the path (appended when absent).
    Command { command: String },
}

/// An editor found installed on this machine.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EditorAppDto {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum EditSyncStateDto {
    /// Saved locally; asking whether to upload the new version.
    Modified,
    /// Saved locally; uploading the new version.
    Uploading,
    /// The server has the saved version.
    Synced,
    /// The file changed on the server since it was opened — asks before overwriting.
    Conflict,
    Failed,
}

/// A newer release the app can offer (tech-gui.md §4.1). `version` is the latest
/// version (no leading `v`); `url` is the release page; `canSelfUpdate` mirrors the
/// core's self-update eligibility. The core `UpdateInfo` has no release-notes field, so
/// none is invented (§4.1).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfoDto {
    pub version: String,
    pub url: String,
    pub tag: String,
    pub can_self_update: bool,
}

/// Update-checker preferences, mirrors core `UpdateConfig` (tech-gui.md §4.3). Crosses
/// both ways: outbound for the settings screen, inbound for `save_update_config`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfigDto {
    pub check_on_startup: bool,
    pub skip_version: String,
}

/// One step of the auto key-setup flow, for the progress view (tech-gui.md §4.2/§4.3).
/// `index` is 1-based (`1..=total`); `description` is the core's human-readable label.
/// Maps from the core `KeySetupStep`. `id` names the step for the frontend's
/// translations; `description` is the core's English text.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct KeySetupStepDto {
    pub id: String,
    pub index: u8,
    pub total: u8,
    pub description: String,
}

/// A private key found in `~/.ssh` (or picked by hand), for the key pickers.
/// `encrypted` marks a passphrase-protected private half; `publicKey` is the
/// shareable public half and is absent only when it cannot be read without one.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyDto {
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

impl From<&SshKeyInfo> for SshKeyDto {
    fn from(k: &SshKeyInfo) -> Self {
        Self {
            path: k.path.to_string_lossy().into_owned(),
            name: k.name.clone(),
            kind: k.kind.clone(),
            comment: k.comment.clone(),
            encrypted: k.encrypted,
            public_key: k.public_key.clone(),
        }
    }
}

/// Which key key-setup installs.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum KeyChoiceDto {
    /// A new Ed25519 key in `~/.ssh`, named `name` (default
    /// `omnyssh_<host>_ed25519`; an existing pair of that name is reused).
    Generate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// An existing private key.
    Existing { path: String },
}

/// How the server accepts logins after key setup.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum AuthModeDto {
    KeyAndPassword,
    KeyOnly,
}

impl From<AuthModeDto> for AuthMode {
    fn from(m: AuthModeDto) -> Self {
        match m {
            AuthModeDto::KeyAndPassword => AuthMode::KeyAndPassword,
            AuthModeDto::KeyOnly => AuthMode::KeyOnly,
        }
    }
}

/// A host's stored credentials as far as the key dialog and the host form need
/// them: which key file it uses (a path, never key material) and whether a
/// password is stored (never the password itself). Fetched per host on demand,
/// never broadcast with the host list.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HostAuthDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    pub has_password: bool,
}

/// Raw PTY output bytes for a terminal session's per-session `Channel` (tech-gui.md
/// §3.3/§3.6). Deliberately **not** `Serialize`: that dodges the blanket
/// `Serialize -> IpcResponse` mapping (which would JSON-encode to a slow `number[]`),
/// so the bytes ride the channel as a raw `ArrayBuffer` that xterm writes directly.
/// Only ever sent, never received — the sole non-DTO on the boundary.
#[derive(specta::Type)]
#[specta(transparent)]
pub struct TerminalBytes(pub Vec<u8>);

impl tauri::ipc::IpcResponse for TerminalBytes {
    fn body(self) -> tauri::Result<tauri::ipc::InvokeResponseBody> {
        Ok(tauri::ipc::InvokeResponseBody::Raw(self.0))
    }
}

impl From<&HostSource> for HostSourceDto {
    fn from(source: &HostSource) -> Self {
        match source {
            HostSource::SshConfig => Self::SshConfig,
            HostSource::Manual => Self::Manual,
        }
    }
}

impl From<&Host> for HostDto {
    fn from(host: &Host) -> Self {
        Self {
            name: host.name.clone(),
            hostname: host.hostname.clone(),
            user: host.user.clone(),
            port: host.port,
            tags: host.tags.clone(),
            notes: host.notes.clone(),
            source: (&host.source).into(),
            has_key: host.identity_file.is_some(),
            password_auth_disabled: host.password_auth_disabled,
            monitoring: host.monitoring.into(),
            monitor_port: host.monitor_port,
            vpn: host.vpn.clone(),
        }
    }
}

impl From<HostInputDto> for Host {
    /// Build a **manual** host from the form payload (tech-gui.md §4.1, Stage 4.1).
    /// `source` is forced to `Manual` (the form only ever authors manual entries);
    /// blank optional fields collapse to `None` so an empty identity path never reads
    /// as `hasKey` and an empty password is not persisted. Key-setup metadata
    /// (`password_auth_disabled`, `key_setup_date`) and the SSH-config rename origin
    /// are not the form's to set — `save_host` carries them over across an edit.
    fn from(dto: HostInputDto) -> Self {
        // The frontend already trims; collapse an exact-empty string to `None` as a
        // last guard. Password is not trimmed — its bytes are preserved verbatim.
        let non_empty = |s: Option<String>| s.filter(|v| !v.is_empty());
        let monitoring: MonitorMode = dto.monitoring.map(Into::into).unwrap_or_default();
        Host {
            name: dto.name,
            hostname: dto.hostname,
            user: dto.user,
            port: dto.port,
            identity_file: non_empty(dto.identity_file),
            password: non_empty(dto.password),
            proxy_jump: non_empty(dto.proxy_jump),
            tags: dto.tags,
            notes: non_empty(dto.notes),
            source: HostSource::Manual,
            original_ssh_host: None,
            monitoring,
            // Port 0 is not dialable, and an SSH host has nothing to probe: drop
            // both, so a later mode switch cannot inherit a stale target.
            monitor_port: dto
                .monitor_port
                .filter(|&p| p != 0 && monitoring == MonitorMode::TcpPort),
            key_setup_date: None,
            password_auth_disabled: None,
            vpn: non_empty(dto.vpn.map(|v| v.trim().to_string())),
        }
    }
}

impl From<&ConnectionStatus> for ConnectionStatusDto {
    fn from(status: &ConnectionStatus) -> Self {
        match status {
            ConnectionStatus::Unknown => Self::Unknown,
            ConnectionStatus::Connecting => Self::Connecting,
            ConnectionStatus::Connected => Self::Connected,
            ConnectionStatus::Failed(message) => Self::Failed {
                message: message.clone(),
            },
        }
    }
}

impl From<&ProcessInfo> for ProcessDto {
    fn from(process: &ProcessInfo) -> Self {
        Self {
            name: process.name.clone(),
            cpu_percent: process.cpu_percent,
            mem_percent: process.mem_percent,
        }
    }
}

impl From<&Metrics> for MetricsDto {
    fn from(metrics: &Metrics) -> Self {
        Self {
            cpu_percent: metrics.cpu_percent,
            ram_percent: metrics.ram_percent,
            disk_percent: metrics.disk_percent,
            uptime: metrics.uptime.clone(),
            load_avg: metrics.load_avg.clone(),
            os_info: metrics.os_info.clone(),
            top_processes: metrics
                .top_processes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(ProcessDto::from)
                .collect(),
            age_seconds: metrics.last_updated.elapsed().as_secs(),
        }
    }
}

impl From<&ServiceKind> for ServiceKindDto {
    fn from(kind: &ServiceKind) -> Self {
        match kind {
            ServiceKind::Docker => Self::Docker,
            ServiceKind::Nginx => Self::Nginx,
            ServiceKind::PostgreSQL => Self::PostgreSQL,
            ServiceKind::Redis => Self::Redis,
            ServiceKind::NodeJS => Self::NodeJS,
        }
    }
}

impl From<&ServiceMetric> for ServiceMetricDto {
    fn from(metric: &ServiceMetric) -> Self {
        let MetricValue::Integer(value) = metric.value;
        Self {
            name: metric.name.clone(),
            value,
        }
    }
}

impl From<&DetectedService> for ServiceDto {
    fn from(service: &DetectedService) -> Self {
        Self {
            kind: (&service.kind).into(),
            metrics: service.metrics.iter().map(ServiceMetricDto::from).collect(),
        }
    }
}

impl From<&SnippetScope> for SnippetScopeDto {
    fn from(scope: &SnippetScope) -> Self {
        match scope {
            SnippetScope::Global => Self::Global,
            SnippetScope::Host => Self::Host,
        }
    }
}

impl From<&SnippetScopeDto> for SnippetScope {
    fn from(scope: &SnippetScopeDto) -> Self {
        match scope {
            SnippetScopeDto::Global => Self::Global,
            SnippetScopeDto::Host => Self::Host,
        }
    }
}

impl From<&Snippet> for SnippetDto {
    fn from(snippet: &Snippet) -> Self {
        Self {
            name: snippet.name.clone(),
            command: snippet.command.clone(),
            scope: (&snippet.scope).into(),
            host: snippet.host.clone(),
            tags: snippet.tags.clone(),
            params: snippet.params.clone(),
        }
    }
}

impl From<SnippetDto> for Snippet {
    fn from(dto: SnippetDto) -> Self {
        Self {
            name: dto.name,
            command: dto.command,
            scope: (&dto.scope).into(),
            host: dto.host,
            tags: dto.tags,
            params: dto.params,
        }
    }
}

impl From<&FileEntry> for FileEntryDto {
    fn from(entry: &FileEntry) -> Self {
        Self {
            name: entry.name.clone(),
            path: entry.path.clone(),
            size: entry.size,
            is_dir: entry.is_dir,
            modified: entry.modified,
            permissions: entry.permissions,
            is_link: entry.is_link,
        }
    }
}

impl From<&UpdateInfo> for UpdateInfoDto {
    fn from(info: &UpdateInfo) -> Self {
        Self {
            version: info.latest.clone(),
            url: info.release_url(),
            tag: info.tag.clone(),
            can_self_update: info.can_self_update,
        }
    }
}

impl From<&UpdateConfig> for UpdateConfigDto {
    fn from(config: &UpdateConfig) -> Self {
        Self {
            check_on_startup: config.check_on_startup,
            skip_version: config.skip_version.clone(),
        }
    }
}

impl From<UpdateConfigDto> for UpdateConfig {
    fn from(dto: UpdateConfigDto) -> Self {
        Self {
            check_on_startup: dto.check_on_startup,
            skip_version: dto.skip_version,
        }
    }
}

impl From<KeySetupStep> for KeySetupStepDto {
    fn from(step: KeySetupStep) -> Self {
        let (id, index) = match step {
            KeySetupStep::GenerateKey => ("generateKey", 1),
            KeySetupStep::CopyPublicKey => ("copyPublicKey", 2),
            KeySetupStep::VerifyKeyAuth => ("verifyKeyAuth", 3),
            KeySetupStep::DisablePassword => ("disablePassword", 4),
            // "Password and key" mode's step 4 in place of disabling.
            KeySetupStep::EnablePassword => ("enablePassword", 4),
            KeySetupStep::ReloadSshd => ("reloadSshd", 5),
            KeySetupStep::FinalCheck => ("finalCheck", 6),
        };
        Self {
            id: id.to_string(),
            index,
            total: KeySetupStep::all_steps().len() as u8,
            description: step.description().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_with_secret() -> Host {
        Host {
            name: "web-prod-1".to_string(),
            hostname: "10.0.0.1".to_string(),
            user: "deploy".to_string(),
            port: 2222,
            identity_file: Some("/home/me/.ssh/id_ed25519".to_string()),
            password: Some("s3cr3t-p4ss".to_string()),
            tags: vec!["prod".to_string()],
            notes: Some("primary".to_string()),
            source: HostSource::Manual,
            password_auth_disabled: Some(true),
            ..Host::default()
        }
    }

    #[test]
    fn host_dto_never_serialises_a_password_or_key() {
        let dto = HostDto::from(&host_with_secret());
        let json = serde_json::to_string(&dto).expect("serialise HostDto");
        // The wire form must carry neither the secret field nor its value.
        // (`passwordAuthDisabled` is a public boolean flag, not the password.)
        assert!(
            !json.contains(r#""password""#),
            "password field leaked: {json}"
        );
        assert!(!json.contains("s3cr3t"), "password value leaked: {json}");
        assert!(!json.contains("identityFile"), "key field leaked: {json}");
        assert!(!json.contains("id_ed25519"), "key path leaked: {json}");
    }

    #[test]
    fn host_dto_maps_public_fields() {
        let dto = HostDto::from(&host_with_secret());
        assert_eq!(dto.name, "web-prod-1");
        assert_eq!(dto.hostname, "10.0.0.1");
        assert_eq!(dto.user, "deploy");
        assert_eq!(dto.port, 2222);
        assert_eq!(dto.tags, vec!["prod".to_string()]);
        assert_eq!(dto.notes.as_deref(), Some("primary"));
        assert!(matches!(dto.source, HostSourceDto::Manual));
        // `hasKey` is derived from the identity file, which itself stays backend-side.
        assert!(dto.has_key);
        assert_eq!(dto.password_auth_disabled, Some(true));
    }

    #[test]
    fn host_dto_has_key_is_false_without_identity_file() {
        let host = Host {
            identity_file: None,
            ..Host::default()
        };
        assert!(!HostDto::from(&host).has_key);
    }

    fn full_input() -> HostInputDto {
        HostInputDto {
            name: "web-prod-1".to_string(),
            hostname: "10.0.0.1".to_string(),
            user: "deploy".to_string(),
            port: 2222,
            identity_file: Some("/home/me/.ssh/id_ed25519".to_string()),
            password: Some("s3cr3t-p4ss".to_string()),
            proxy_jump: Some("bastion".to_string()),
            clear_identity: None,
            tags: vec!["prod".to_string()],
            notes: Some("primary".to_string()),
            monitoring: None,
            monitor_port: None,
            vpn: None,
        }
    }

    #[test]
    fn host_input_maps_to_a_manual_host() {
        // The form only ever authors manual entries — editing an import produces a
        // manual copy (tech-gui.md §4.1) — so `source` is forced regardless of input.
        let host = Host::from(full_input());
        assert_eq!(host.name, "web-prod-1");
        assert_eq!(host.hostname, "10.0.0.1");
        assert_eq!(host.user, "deploy");
        assert_eq!(host.port, 2222);
        assert_eq!(
            host.identity_file.as_deref(),
            Some("/home/me/.ssh/id_ed25519")
        );
        assert_eq!(host.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(host.tags, vec!["prod".to_string()]);
        assert_eq!(host.notes.as_deref(), Some("primary"));
        assert_eq!(host.source, HostSource::Manual);
        // Key-setup metadata + rename origin are never the form's to set.
        assert!(host.original_ssh_host.is_none());
        assert!(host.key_setup_date.is_none());
        assert!(host.password_auth_disabled.is_none());
    }

    #[test]
    fn host_input_keeps_the_password_backend_side() {
        // The password rides inbound into the backend `Host`, then the outbound
        // `HostDto` must drop it: it never reaches the webview (§3.4).
        let host = Host::from(full_input());
        assert_eq!(host.password.as_deref(), Some("s3cr3t-p4ss"));
        let json = serde_json::to_string(&HostDto::from(&host)).expect("serialise HostDto");
        assert!(!json.contains("password"), "password leaked: {json}");
        assert!(!json.contains("s3cr3t"), "password value leaked: {json}");
    }

    #[test]
    fn host_input_collapses_blank_optionals_to_none() {
        // An empty identity path must not read as `hasKey`; an empty password/proxy/
        // notes must not persist an empty string.
        let host = Host::from(HostInputDto {
            name: "h".to_string(),
            hostname: "example.com".to_string(),
            user: "root".to_string(),
            port: 22,
            identity_file: Some(String::new()),
            password: Some(String::new()),
            proxy_jump: Some(String::new()),
            clear_identity: None,
            tags: vec![],
            notes: Some(String::new()),
            monitoring: None,
            monitor_port: None,
            vpn: None,
        });
        assert!(host.identity_file.is_none());
        assert!(host.password.is_none());
        assert!(host.proxy_jump.is_none());
        assert!(host.notes.is_none());
        assert!(!HostDto::from(&host).has_key);
    }

    #[test]
    fn connection_status_dto_maps_every_variant() {
        assert!(matches!(
            ConnectionStatusDto::from(&ConnectionStatus::Unknown),
            ConnectionStatusDto::Unknown
        ));
        assert!(matches!(
            ConnectionStatusDto::from(&ConnectionStatus::Connecting),
            ConnectionStatusDto::Connecting
        ));
        assert!(matches!(
            ConnectionStatusDto::from(&ConnectionStatus::Connected),
            ConnectionStatusDto::Connected
        ));
        let failed = ConnectionStatusDto::from(&ConnectionStatus::Failed("boom".to_string()));
        match failed {
            ConnectionStatusDto::Failed { message } => assert_eq!(message, "boom"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn connection_status_dto_tags_on_kind() {
        let json = serde_json::to_string(&ConnectionStatusDto::from(&ConnectionStatus::Failed(
            "down".to_string(),
        )))
        .expect("serialise status");
        assert_eq!(json, r#"{"kind":"failed","message":"down"}"#);
        let connected = serde_json::to_string(&ConnectionStatusDto::Connected).unwrap();
        assert_eq!(connected, r#"{"kind":"connected"}"#);
    }

    #[test]
    fn metrics_dto_passes_none_through() {
        let dto = MetricsDto::from(&Metrics::default());
        assert!(dto.cpu_percent.is_none());
        assert!(dto.ram_percent.is_none());
        assert!(dto.disk_percent.is_none());
        assert!(dto.uptime.is_none());
        assert!(dto.load_avg.is_none());
        assert!(dto.os_info.is_none());
        assert!(dto.top_processes.is_empty());
        // A freshly stamped sample is age zero.
        assert_eq!(dto.age_seconds, 0);
    }

    #[test]
    fn metrics_dto_maps_populated_fields() {
        let metrics = Metrics {
            cpu_percent: Some(42.5),
            ram_percent: Some(70.0),
            disk_percent: Some(12.0),
            uptime: Some("3 days".to_string()),
            load_avg: Some("0.5 0.4 0.3".to_string()),
            os_info: Some("Ubuntu 22.04".to_string()),
            top_processes: Some(vec![ProcessInfo {
                name: "postgres".to_string(),
                cpu_percent: 30.0,
                mem_percent: 15.0,
            }]),
            ..Metrics::default()
        };
        let dto = MetricsDto::from(&metrics);
        assert_eq!(dto.cpu_percent, Some(42.5));
        assert_eq!(dto.ram_percent, Some(70.0));
        assert_eq!(dto.disk_percent, Some(12.0));
        assert_eq!(dto.uptime.as_deref(), Some("3 days"));
        assert_eq!(dto.os_info.as_deref(), Some("Ubuntu 22.04"));
        assert_eq!(dto.top_processes.len(), 1);
        assert_eq!(dto.top_processes[0].name, "postgres");
        assert_eq!(dto.top_processes[0].cpu_percent, 30.0);
        assert_eq!(dto.top_processes[0].mem_percent, 15.0);
    }

    fn metric(name: &str, value: i64) -> ServiceMetric {
        ServiceMetric {
            name: name.to_string(),
            value: MetricValue::Integer(value),
        }
    }

    #[test]
    fn service_kind_dto_uses_lowercase_wire_names() {
        // The frontend switches on these exact strings (tech-gui.md §4.1).
        let names = [
            (ServiceKind::Docker, r#""docker""#),
            (ServiceKind::Nginx, r#""nginx""#),
            (ServiceKind::PostgreSQL, r#""postgresql""#),
            (ServiceKind::Redis, r#""redis""#),
            (ServiceKind::NodeJS, r#""nodejs""#),
        ];
        for (kind, wire) in names {
            let json = serde_json::to_string(&ServiceKindDto::from(&kind)).expect("serialise kind");
            assert_eq!(json, wire, "kind {kind:?} must map to {wire}");
        }
    }

    #[test]
    fn service_dto_maps_kind_and_integer_metrics() {
        let service = DetectedService {
            kind: ServiceKind::Docker,
            metrics: vec![
                metric("containers_running", 4),
                metric("containers_stopped", 1),
            ],
        };
        let dto = ServiceDto::from(&service);
        assert!(matches!(dto.kind, ServiceKindDto::Docker));
        assert_eq!(dto.metrics.len(), 2);
        assert_eq!(dto.metrics[0].name, "containers_running");
        assert_eq!(dto.metrics[0].value, 4);
        assert_eq!(dto.metrics[1].name, "containers_stopped");
        assert_eq!(dto.metrics[1].value, 1);
    }

    #[test]
    fn service_dto_keeps_an_empty_metric_list() {
        let dto = ServiceDto::from(&DetectedService {
            kind: ServiceKind::Nginx,
            metrics: vec![],
        });
        assert!(matches!(dto.kind, ServiceKindDto::Nginx));
        assert!(dto.metrics.is_empty());
    }

    fn full_snippet() -> Snippet {
        Snippet {
            name: "restart-svc".to_string(),
            command: "systemctl restart {{service}}".to_string(),
            scope: SnippetScope::Host,
            host: Some("web-1".to_string()),
            tags: Some(vec!["ops".to_string()]),
            params: Some(vec!["service".to_string()]),
        }
    }

    #[test]
    fn snippet_dto_maps_every_field() {
        let dto = SnippetDto::from(&full_snippet());
        assert_eq!(dto.name, "restart-svc");
        assert_eq!(dto.command, "systemctl restart {{service}}");
        assert_eq!(dto.scope, SnippetScopeDto::Host);
        assert_eq!(dto.host.as_deref(), Some("web-1"));
        assert_eq!(dto.tags, Some(vec!["ops".to_string()]));
        assert_eq!(dto.params, Some(vec!["service".to_string()]));
    }

    #[test]
    fn snippet_scope_dto_uses_lowercase_wire_names() {
        // The frontend switches on these exact strings (tech-gui.md §4.1).
        let global = serde_json::to_string(&SnippetScopeDto::Global).unwrap();
        assert_eq!(global, r#""global""#);
        let host = serde_json::to_string(&SnippetScopeDto::Host).unwrap();
        assert_eq!(host, r#""host""#);
    }

    #[test]
    fn snippet_dto_omits_absent_optionals_on_the_wire() {
        let dto = SnippetDto::from(&Snippet {
            name: "ls".to_string(),
            command: "ls -la".to_string(),
            scope: SnippetScope::Global,
            host: None,
            tags: None,
            params: None,
        });
        let json = serde_json::to_string(&dto).unwrap();
        assert_eq!(json, r#"{"name":"ls","command":"ls -la","scope":"global"}"#);
    }

    #[test]
    fn snippet_dto_round_trips_through_snippet() {
        let original = full_snippet();
        let back: Snippet = SnippetDto::from(&original).into();
        assert_eq!(back.name, original.name);
        assert_eq!(back.command, original.command);
        assert_eq!(back.scope, original.scope);
        assert_eq!(back.host, original.host);
        assert_eq!(back.tags, original.tags);
        assert_eq!(back.params, original.params);
    }

    #[test]
    fn snippet_dto_deserialises_a_sparse_inbound_payload() {
        // A minimal save_snippet payload: optionals absent -> None (tech-gui.md §4.1).
        let dto: SnippetDto =
            serde_json::from_str(r#"{"name":"pwd","command":"pwd","scope":"global"}"#).unwrap();
        assert_eq!(dto.scope, SnippetScopeDto::Global);
        assert!(dto.host.is_none());
        assert!(dto.tags.is_none());
        assert!(dto.params.is_none());
    }

    #[test]
    fn file_entry_dto_maps_a_file_and_a_directory() {
        let file = FileEntry {
            name: "config.toml".to_string(),
            path: "/etc/omnyssh/config.toml".to_string(),
            size: 4096,
            is_dir: false,
            ..Default::default()
        };
        let dto = FileEntryDto::from(&file);
        assert_eq!(dto.name, "config.toml");
        assert_eq!(dto.path, "/etc/omnyssh/config.toml");
        assert_eq!(dto.size, 4096);
        assert!(!dto.is_dir);

        let dir = FileEntry {
            name: "..".to_string(),
            path: "/etc".to_string(),
            size: 0,
            is_dir: true,
            ..Default::default()
        };
        let dto = FileEntryDto::from(&dir);
        assert!(dto.is_dir);
        assert_eq!(dto.size, 0);
    }

    #[test]
    fn file_entry_dto_uses_camel_case_is_dir_on_the_wire() {
        // The frontend reads `isDir` (tech-gui.md §4.1); a snake-case leak would
        // silently render every entry as a file.
        let json = serde_json::to_string(&FileEntryDto::from(&FileEntry {
            name: "srv".to_string(),
            path: "/srv".to_string(),
            size: 0,
            is_dir: true,
            ..Default::default()
        }))
        .expect("serialise FileEntryDto");
        assert_eq!(
            json,
            r#"{"name":"srv","path":"/srv","size":0,"isDir":true}"#
        );
    }

    #[test]
    fn update_info_dto_maps_from_core_and_omits_notes() {
        use omnyssh_core::update::InstallMethod;
        let info = UpdateInfo {
            current: "1.0.0".to_string(),
            latest: "1.2.0".to_string(),
            tag: "v1.2.0".to_string(),
            method: InstallMethod::Manual,
            can_self_update: true,
        };
        let dto = UpdateInfoDto::from(&info);
        // `version` is the latest release, `url` the core's release page (tech-gui.md §4.1).
        assert_eq!(dto.version, "1.2.0");
        assert_eq!(dto.tag, "v1.2.0");
        assert_eq!(dto.url, info.release_url());
        assert!(dto.can_self_update);
        // No release-notes field is invented.
        let json = serde_json::to_string(&dto).expect("serialise UpdateInfoDto");
        assert!(!json.contains("notes"), "invented a notes field: {json}");
        assert!(
            json.contains(r#""canSelfUpdate":true"#),
            "wire name drift: {json}"
        );
    }

    #[test]
    fn update_config_dto_round_trips_through_core() {
        // The settings screen edits these and `save_update_config` persists them; the
        // round-trip must be lossless (tech-gui.md §4.3 test obligation).
        let core = UpdateConfig {
            check_on_startup: false,
            skip_version: "1.2.3".to_string(),
        };
        let dto = UpdateConfigDto::from(&core);
        assert!(!dto.check_on_startup);
        assert_eq!(dto.skip_version, "1.2.3");
        let json = serde_json::to_string(&dto).expect("serialise UpdateConfigDto");
        assert_eq!(json, r#"{"checkOnStartup":false,"skipVersion":"1.2.3"}"#);

        let back: UpdateConfig = dto.into();
        assert!(!back.check_on_startup);
        assert_eq!(back.skip_version, "1.2.3");
    }

    #[test]
    fn key_setup_step_dto_maps_index_total_and_label() {
        // The progress view reads a 1-based `index` out of `total` plus the core's
        // label (tech-gui.md §4.2). The first/last steps pin the discriminant range.
        let dto = KeySetupStepDto::from(KeySetupStep::VerifyKeyAuth);
        assert_eq!(dto.index, 3);
        assert_eq!(dto.total, 6);
        assert_eq!(dto.description, "Verifying key authentication");
        assert_eq!(KeySetupStepDto::from(KeySetupStep::GenerateKey).index, 1);
        assert_eq!(KeySetupStepDto::from(KeySetupStep::FinalCheck).index, 6);
    }

    #[test]
    fn key_setup_step_dto_uses_camel_case_wire_names() {
        // The frontend reads `index`/`total`/`description` (tech-gui.md §4.2).
        let json = serde_json::to_string(&KeySetupStepDto::from(KeySetupStep::CopyPublicKey))
            .expect("serialise KeySetupStepDto");
        assert_eq!(
            json,
            r#"{"id":"copyPublicKey","index":2,"total":6,"description":"Copying public key to server"}"#
        );
    }

    #[test]
    fn enabling_passwords_stands_in_for_step_four() {
        let dto = KeySetupStepDto::from(KeySetupStep::EnablePassword);
        assert_eq!(
            (dto.id.as_str(), dto.index, dto.total),
            ("enablePassword", 4, 6)
        );
    }

    #[test]
    fn transfer_update_dto_flattens_the_failure_reason() {
        let failed = TransferUpdate {
            id: 7,
            state: TransferState::Failed("disk full".into()),
            done: 512,
            total: 2048,
        };
        assert_eq!(
            serde_json::to_string(&TransferUpdateDto::from(&failed)).unwrap(),
            r#"{"id":7,"state":"failed","done":512,"total":2048,"error":"disk full"}"#
        );
        let done = TransferUpdate {
            state: TransferState::Done,
            ..failed
        };
        assert_eq!(
            serde_json::to_string(&TransferUpdateDto::from(&done)).unwrap(),
            r#"{"id":7,"state":"done","done":512,"total":2048}"#
        );
    }

    #[test]
    fn editor_dto_is_internally_tagged() {
        let app: EditorDto =
            serde_json::from_str(r#"{"kind":"app","name":"Zed","path":"/Applications/Zed.app"}"#)
                .unwrap();
        assert!(matches!(app, EditorDto::App { ref name, .. } if name == "Zed"));
        let system: EditorDto = serde_json::from_str(r#"{"kind":"system"}"#).unwrap();
        assert!(matches!(system, EditorDto::System));
    }
}

// ---------------------------------------------------------------------------
// File manager, Docker, VPN
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveFormatDto {
    TarGz,
    Zip,
}

/// A file operation on the server that SFTP has no verb for.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RemoteFsOpDto {
    /// Delete files and folders (recursively).
    Delete {
        paths: Vec<String>,
    },
    /// Pack entries of `dir` into `dir/archive`.
    Compress {
        dir: String,
        names: Vec<String>,
        archive: String,
        format: ArchiveFormatDto,
    },
    Extract {
        archive: String,
        dest: String,
    },
    Chmod {
        paths: Vec<String>,
        mode: u32,
        recursive: bool,
    },
    NewFile {
        path: String,
    },
    Copy {
        paths: Vec<String>,
        dest: String,
    },
    Move {
        paths: Vec<String>,
        dest: String,
    },
}

/// A file operation on this machine.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LocalFsOpDto {
    Mkdir {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    /// Move to the Trash (recoverable).
    Trash {
        paths: Vec<String>,
    },
    NewFile {
        path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDto {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: String,
    pub created: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

impl From<&omnyssh_core::ssh::docker::Container> for ContainerDto {
    fn from(c: &omnyssh_core::ssh::docker::Container) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            image: c.image.clone(),
            state: c.state.clone(),
            status: c.status.clone(),
            ports: c.ports.clone(),
            created: c.created.clone(),
            cpu: c.cpu.clone(),
            memory: c.memory.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DockerListDto {
    pub containers: Vec<ContainerDto>,
    /// The login user needed `sudo` for docker (exec shells need it too).
    pub sudo: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum DockerActionDto {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VpnStatusDto {
    /// Tunnelblick can be used on this OS (macOS).
    pub supported: bool,
    pub installed: bool,
    /// The Tunnelblick release the app installs.
    pub installs_version: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum VpnInstallStageDto {
    Downloading,
    Verifying,
    Installing,
    Done,
    Failed,
}
