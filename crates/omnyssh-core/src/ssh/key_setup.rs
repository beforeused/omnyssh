//! Auto SSH Key Setup
//!
//! Provides automated SSH key generation and server configuration for transitioning
//! from password-based to key-based authentication.
//!
//! ## Safety Invariants
//! - Never disable password authentication without verified key auth
//! - Append to authorized_keys, never overwrite
//! - Always backup sshd_config before modification
//! - Show warning about alternative access before disabling password
//! - Log all operations to key_setup.log
//! - Private key 600, .ssh directory 700
//! - Never transmit private key over network

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time;
use tracing::{error, info, warn};

use crate::ssh::client::Host;
use crate::ssh::session::{self, SshSession};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum length for sanitized hostname in key filename.
const MAX_HOSTNAME_LENGTH: usize = 64;

/// Total timeout for the entire key setup process.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout for individual SSH operations during key setup.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Key Type
// ---------------------------------------------------------------------------

/// Supported SSH key types for generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// Ed25519 (recommended, modern, fast).
    Ed25519,
}

impl KeyType {
    /// Returns the file extension for this key type.
    pub fn extension(&self) -> &'static str {
        match self {
            KeyType::Ed25519 => "ed25519",
        }
    }
}

// ---------------------------------------------------------------------------
// Key Setup Steps
// ---------------------------------------------------------------------------

/// Individual steps in the key setup process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeySetupStep {
    GenerateKey = 1,
    CopyPublicKey = 2,
    VerifyKeyAuth = 3,
    DisablePassword = 4,
    ReloadSshd = 5,
    FinalCheck = 6,
    /// "Password and key" mode on a server that had password logins off: turn them
    /// back on. Takes step 4's place in that mode.
    EnablePassword = 7,
}

impl KeySetupStep {
    /// Returns all steps in order.
    pub fn all_steps() -> Vec<Self> {
        vec![
            Self::GenerateKey,
            Self::CopyPublicKey,
            Self::VerifyKeyAuth,
            Self::DisablePassword,
            Self::ReloadSshd,
            Self::FinalCheck,
        ]
    }

    /// Human-readable description for UI display.
    pub fn description(&self) -> &'static str {
        match self {
            Self::GenerateKey => "Generating Ed25519 key pair",
            Self::CopyPublicKey => "Copying public key to server",
            Self::VerifyKeyAuth => "Verifying key authentication",
            Self::DisablePassword => "Disabling password authentication",
            Self::ReloadSshd => "Reloading SSH service",
            Self::FinalCheck => "Final verification",
            Self::EnablePassword => "Enabling password authentication",
        }
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Which key to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// Generate a new key pair in `~/.ssh` — named `file_name`, or
    /// `omnyssh_<host>_<type>` by default. An existing pair of that name is reused.
    Generate { file_name: Option<String> },
    /// Install this existing private key (its `.pub`, or a public key derived
    /// from it).
    Existing(PathBuf),
}

/// How the server should accept logins once the key is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// The key works, and so does the password (re-enabled if it was off).
    KeyAndPassword,
    /// Only keys: password logins are disabled — never before the key is verified.
    KeyOnly,
}

/// What [`setup_key_with_options`] does.
#[derive(Debug, Clone)]
pub struct KeySetupOptions {
    pub key_type: KeyType,
    pub source: KeySource,
    pub mode: AuthMode,
}

// ---------------------------------------------------------------------------
// Key Setup State
// ---------------------------------------------------------------------------

/// Result of the key setup process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySetupState {
    /// Setup not yet started.
    NotStarted,
    /// Setup is in progress.
    InProgress,
    /// Setup completed successfully (password auth disabled).
    Success,
    /// Setup partially succeeded (key works, but no sudo to disable password).
    PartialSuccess,
    /// Setup failed safely (password auth NOT disabled).
    FailedSafe,
    /// Setup failed after disabling password — needs rollback.
    NeedsRollback,
    /// Rollback completed.
    RolledBack,
}

/// State machine for tracking key setup progress.
#[derive(Debug)]
pub struct KeySetupMachine {
    state: KeySetupState,
    current_step: Option<KeySetupStep>,
    has_sudo: bool,
    password_disabled: bool,
}

impl KeySetupMachine {
    /// Creates a new state machine in the NotStarted state.
    pub fn new() -> Self {
        Self {
            state: KeySetupState::NotStarted,
            current_step: None,
            has_sudo: true,
            password_disabled: false,
        }
    }

    /// Returns the current state.
    pub fn state(&self) -> &KeySetupState {
        &self.state
    }

    /// Sets the sudo availability flag.
    pub fn set_has_sudo(&mut self, has_sudo: bool) {
        self.has_sudo = has_sudo;
    }

    /// Marks a step as complete with the given result.
    ///
    /// Updates the state machine based on which step completed and whether it succeeded.
    /// Implements the safety invariants from tech-2.md B.3.3.
    pub fn step_result(&mut self, step: KeySetupStep, result: Result<()>) {
        self.current_step = Some(step);

        match (step, result) {
            // Step 1-2: Safe to fail, no changes to server yet.
            (KeySetupStep::GenerateKey | KeySetupStep::CopyPublicKey, Err(_)) => {
                self.state = KeySetupState::FailedSafe;
            }

            // Step 3 (VerifyKeyAuth): CRITICAL — if this fails, STOP.
            // Never disable password without verified key.
            (KeySetupStep::VerifyKeyAuth, Err(_)) => {
                self.state = KeySetupState::FailedSafe;
            }
            (KeySetupStep::VerifyKeyAuth, Ok(())) if !self.has_sudo => {
                // Key works, but no sudo — partial success.
                self.state = KeySetupState::PartialSuccess;
            }

            // Step 4 (DisablePassword): Point of no return.
            (KeySetupStep::DisablePassword, Ok(())) => {
                self.password_disabled = true;
            }
            (KeySetupStep::DisablePassword, Err(_)) => {
                // Failed to disable password — safe, stop here.
                self.state = KeySetupState::FailedSafe;
            }
            // Turning passwords back on only ever widens access: a failure leaves
            // the server as it was.
            (KeySetupStep::EnablePassword, Err(_)) => {
                self.state = KeySetupState::FailedSafe;
            }

            // Step 5 (ReloadSshd): Mostly safe (reload doesn't kill existing connections).
            (KeySetupStep::ReloadSshd, Err(_)) => {
                // Reload failed, but password is already disabled.
                // This might be okay if the daemon auto-reloaded, but risky.
                self.state = KeySetupState::NeedsRollback;
            }

            // Step 6 (FinalCheck): Verify key still works after reload.
            // If this fails, password is disabled but key doesn't work — emergency rollback!
            (KeySetupStep::FinalCheck, Err(_)) => {
                self.state = KeySetupState::NeedsRollback;
            }
            (KeySetupStep::FinalCheck, Ok(())) => {
                self.state = KeySetupState::Success;
            }

            // All other OK results → continue.
            (_, Ok(())) => {
                self.state = KeySetupState::InProgress;
            }
        }
    }

    /// Marks the rollback as complete.
    pub fn rollback_complete(&mut self) {
        self.state = KeySetupState::RolledBack;
        self.password_disabled = false;
    }
}

impl Default for KeySetupMachine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Key Generation
// ---------------------------------------------------------------------------

/// Generates an Ed25519 SSH key pair and writes it to disk.
///
/// Returns the paths to the private and public key files.
///
/// # Errors
/// Returns an error if key generation or file I/O fails.
///
/// # Safety
/// - Private key is written with mode 0600.
/// - .ssh directory is created with mode 0700 if it doesn't exist.
pub async fn generate_key_pair(host_name: &str, key_type: KeyType) -> Result<(PathBuf, PathBuf)> {
    let sanitized = sanitize_hostname(host_name);
    let key_filename = format!("omnyssh_{}_{}", sanitized, key_type.extension());
    generate_key_pair_file(&key_filename, host_name, key_type).await
}

/// Like [`generate_key_pair`], but saved as `~/.ssh/<file_name>` (and `.pub`).
///
/// # Errors
/// An unsafe name (see [`validate_key_file_name`]), a private key of that name
/// without its `.pub` (never overwritten), or a key generation / I/O failure.
pub async fn generate_named_key_pair(
    file_name: &str,
    host_name: &str,
    key_type: KeyType,
) -> Result<(PathBuf, PathBuf)> {
    let file_name = file_name.trim();
    validate_key_file_name(file_name)?;
    generate_key_pair_file(file_name, host_name, key_type).await
}

/// A key file name the user typed: a plain file name inside `~/.ssh` — no
/// folders, no leading dot, no `.pub`, and only `A–Z a–z 0–9 . _ -`.
///
/// # Errors
/// Describes what is wrong with the name.
pub fn validate_key_file_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("The key name is empty");
    }
    if name.len() > 64 {
        anyhow::bail!("The key name is longer than 64 characters");
    }
    if name.starts_with('.') || name.starts_with('-') {
        anyhow::bail!("The key name cannot start with '.' or '-'");
    }
    if name.ends_with(".pub") {
        anyhow::bail!("The key name cannot end with .pub");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        anyhow::bail!("The key name may only contain letters, digits, '.', '_' and '-'");
    }
    const RESERVED: &[&str] = &["config", "known_hosts", "authorized_keys", "environment"];
    if RESERVED.contains(&name) || name.starts_with("known_hosts") {
        anyhow::bail!("'{name}' is a file ssh already uses");
    }
    Ok(())
}

async fn generate_key_pair_file(
    key_filename: &str,
    host_name: &str,
    key_type: KeyType,
) -> Result<(PathBuf, PathBuf)> {
    let ssh_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("Cannot determine home directory"))?
        .join(".ssh");

    // Create .ssh directory if it doesn't exist.
    tokio::fs::create_dir_all(&ssh_dir)
        .await
        .with_context(|| format!("Failed to create {}", ssh_dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        tokio::fs::set_permissions(&ssh_dir, perms)
            .await
            .with_context(|| format!("Failed to set permissions on {}", ssh_dir.display()))?;
    }

    let private_key_path = ssh_dir.join(key_filename);
    let public_key_path = ssh_dir.join(format!("{}.pub", key_filename));

    // A private key without its .pub is someone's key we cannot pair up: never
    // hand it to ssh-keygen, which would offer to overwrite it.
    if private_key_path.exists() && !public_key_path.exists() {
        return Err(anyhow!(
            "{} already exists — pick it under \"Use an existing key\" instead",
            private_key_path.display()
        ));
    }

    // Check if key already exists - if so, reuse it instead of failing.
    if private_key_path.exists() && public_key_path.exists() {
        info!(
            "Key already exists for {}, reusing: {}",
            host_name,
            private_key_path.display()
        );
        return Ok((private_key_path, public_key_path));
    }

    // Generate the key pair using ssh-keygen directly to ensure macOS OpenSSH compatibility.
    // Using ssh-keygen ensures the key is in the correct format (OpenSSH) from the start.
    info!(
        "Generating {} key pair for {}",
        key_type.extension(),
        host_name
    );

    let key_type_arg = match key_type {
        KeyType::Ed25519 => "ed25519",
    };

    let mut keygen_cmd = tokio::process::Command::new("ssh-keygen");
    keygen_cmd
        .arg("-t")
        .arg(key_type_arg)
        .arg("-f")
        .arg(&private_key_path)
        .arg("-N")
        .arg("") // No passphrase
        .arg("-C")
        .arg(format!("omnyssh-{}", host_name)); // Comment

    // CREATE_NO_WINDOW: the GUI has no console to lend the child, so without this
    // Windows opens one for it. Output is piped either way, so nothing is lost.
    #[cfg(windows)]
    keygen_cmd.creation_flags(0x0800_0000);

    let keygen_output = keygen_cmd
        .output()
        .await
        .context("Failed to run ssh-keygen for key generation")?;

    if !keygen_output.status.success() {
        let stderr = String::from_utf8_lossy(&keygen_output.stderr);
        return Err(anyhow!("ssh-keygen failed to generate key: {}", stderr));
    }

    // Ensure private key has correct permissions (ssh-keygen should set this, but be explicit)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(&private_key_path, perms)
            .await
            .with_context(|| {
                format!(
                    "Failed to set permissions on private key {}",
                    private_key_path.display()
                )
            })?;
    }

    info!(
        "Generated key pair:\n  Private: {}\n  Public: {}",
        private_key_path.display(),
        public_key_path.display()
    );

    Ok((private_key_path, public_key_path))
}

/// Sanitizes a hostname for use in a key filename.
///
/// - Replaces non-alphanumeric characters (except `-`, `_`, `.`) with `_`
/// - Truncates to MAX_HOSTNAME_LENGTH
/// - Returns "unnamed_host" for empty input
///
/// # Examples
/// ```
/// use omnyssh_core::ssh::key_setup::sanitize_hostname;
///
/// assert_eq!(sanitize_hostname("web-prod-1"), "web-prod-1");
/// assert_eq!(sanitize_hostname("my server (prod)"), "my_server__prod_");
/// assert_eq!(sanitize_hostname("../../etc/passwd"), "______etc_passwd");
/// assert_eq!(sanitize_hostname(""), "unnamed_host");
/// ```
pub fn sanitize_hostname(hostname: &str) -> String {
    if hostname.is_empty() {
        return "unnamed_host".to_string();
    }

    let sanitized: String = hostname
        .chars()
        .map(|c| {
            // Only allow alphanumerics, hyphens, and underscores.
            // Dots are replaced to prevent path traversal attacks (../../etc/passwd).
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_HOSTNAME_LENGTH)
        .collect();

    if sanitized.is_empty() {
        "unnamed_host".to_string()
    } else {
        sanitized
    }
}

// ---------------------------------------------------------------------------
// SSH Command Builders
// ---------------------------------------------------------------------------

/// Builds the command to append a public key to authorized_keys.
///
/// The command creates ~/.ssh if it doesn't exist, appends the key (never overwrites),
/// and sets correct permissions.
pub fn build_authorized_keys_command(public_key: &str) -> String {
    // Validate public key is a single line matching expected SSH format.
    let public_key = public_key.trim();
    if public_key.contains('\n') || public_key.contains('\r') || public_key.contains('\0') {
        panic!("Public key file contains invalid characters");
    }
    if crate::ssh::keys::validate_public_line(public_key).is_err() {
        panic!("Public key file has unrecognized key type");
    }

    // Escape single quotes in the public key.
    let escaped_key = public_key.replace('\'', "'\\''");

    // Idempotent: a key that is already authorised is not appended again.
    format!(
        r#"mkdir -p ~/.ssh && chmod 700 ~/.ssh && \
           touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys && \
           {{ grep -qxF -- '{escaped_key}' ~/.ssh/authorized_keys || \
              echo '{escaped_key}' >> ~/.ssh/authorized_keys; }}"#
    )
}

/// Builds the command to disable password authentication in sshd_config.
///
/// The command:
/// - Checks for sudo access
/// - Creates a timestamped backup of sshd_config
/// - Comments out Include directives to prevent overrides from sshd_config.d/*
/// - Disables password authentication (PasswordAuthentication, ChallengeResponseAuthentication, KbdInteractiveAuthentication)
/// - Disables UsePAM to prevent PAM from bypassing password auth restrictions
/// - Validates the config with `sshd -t`
///
/// ## Security Note
/// Even with `PasswordAuthentication no`, PAM (Pluggable Authentication Modules) can provide
/// alternative authentication methods (keyboard-interactive) that accept passwords.
/// Setting `UsePAM no` ensures password authentication is completely disabled and cannot be
/// bypassed with flags like `ssh -o PubkeyAuthentication=no`.
///
/// ## Include Directive Handling
/// Many cloud providers (AWS, DigitalOcean, etc.) use `/etc/ssh/sshd_config.d/*.conf` files
/// (e.g., `50-cloud-init.conf`) that override the main config. We comment out the Include
/// directive to prevent these files from re-enabling password authentication.
///
/// ## Missing Directives
/// `sed` only rewrites lines that already exist. A config that simply omits a
/// directive would otherwise keep sshd's compiled-in default (e.g. `UsePAM yes`),
/// so each directive is also prepended when the file contains no occurrence.
/// Prepending keeps it the first — and therefore effective — global value.
///
/// Returns the command string or an error if sudo is required but unavailable.
pub fn build_disable_password_command() -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let password = force_sshd_directive("PasswordAuthentication", "no");
    let challenge = force_sshd_directive("ChallengeResponseAuthentication", "no");
    let kbd = force_sshd_directive("KbdInteractiveAuthentication", "no");
    let pam = force_sshd_directive("UsePAM", "no");

    format!(
        r#"sudo -n true 2>/dev/null || {{ echo "OMNYSSH_NO_SUDO"; exit 1; }}; \
           sudo cp /etc/ssh/sshd_config /etc/ssh/sshd_config.omnyssh_backup.{timestamp} && \
           sudo sed -i.bak 's|^Include /etc/ssh/sshd_config.d/|#Include /etc/ssh/sshd_config.d/|' /etc/ssh/sshd_config && \
           {password} && \
           {challenge} && \
           {kbd} && \
           {pam} && \
           sudo sshd -t || {{ echo "OMNYSSH_CONFIG_ERROR"; sudo cp /etc/ssh/sshd_config.omnyssh_backup.{timestamp} /etc/ssh/sshd_config; exit 1; }}"#
    )
}

/// Builds the shell fragment that forces `sshd_config` to set `directive value`.
///
/// First rewrites every existing column-0 occurrence (commented or not) to the
/// desired value; then, if the file had none, prepends the directive so it
/// becomes the first global occurrence sshd reads.
fn force_sshd_directive(directive: &str, value: &str) -> String {
    assert!(
        directive
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "directive contains unsafe characters: {directive}"
    );
    assert!(
        value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' '),
        "value contains unsafe characters: {value}"
    );
    format!(
        r#"sudo sed -i.bak 's/^#\?{directive}.*/{directive} {value}/' /etc/ssh/sshd_config && \
           {{ sudo grep -qE '^{directive}[[:space:]]' /etc/ssh/sshd_config || \
              sudo sed -i '1i {directive} {value}' /etc/ssh/sshd_config; }}"#
    )
}

/// Builds the command to turn password authentication back on ("password and
/// key" mode on a server where it was disabled). Backs the config up, forces
/// `PasswordAuthentication yes` and `UsePAM yes` (the pair the key-only setup
/// turned off), and validates with `sshd -t`, restoring the backup if it fails.
pub fn build_enable_password_command() -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let password = force_sshd_directive("PasswordAuthentication", "yes");
    let pam = force_sshd_directive("UsePAM", "yes");
    format!(
        r#"sudo -n true 2>/dev/null || {{ echo "OMNYSSH_NO_SUDO"; exit 1; }}; \
           sudo cp /etc/ssh/sshd_config /etc/ssh/sshd_config.omnyssh_backup.{timestamp} && \
           {password} && \
           {pam} && \
           sudo sshd -t || {{ echo "OMNYSSH_CONFIG_ERROR"; sudo cp /etc/ssh/sshd_config.omnyssh_backup.{timestamp} /etc/ssh/sshd_config; exit 1; }}"#
    )
}

/// Asks sshd for its effective password setting (needs passwordless sudo).
/// Prints `passwordauthentication yes|no`, or nothing without sudo.
const PROBE_PASSWORD_AUTH: &str =
    "sudo -n sshd -T 2>/dev/null | grep -i '^passwordauthentication '";

/// Builds the command to reload the SSH daemon.
///
/// Uses `reload` instead of `restart` to avoid killing existing connections.
pub fn build_reload_sshd_command() -> String {
    r#"if command -v systemctl &>/dev/null; then \
           sudo systemctl reload sshd 2>/dev/null || sudo systemctl reload ssh 2>/dev/null; \
       elif command -v service &>/dev/null; then \
           sudo service sshd reload 2>/dev/null || sudo service ssh reload 2>/dev/null; \
       else \
           echo "OMNYSSH_NO_INIT_SYSTEM"; exit 1; \
       fi"#
    .to_string()
}

/// Builds the emergency rollback command.
///
/// Restores the most recent OmnySSH backup of sshd_config and reloads the daemon.
pub fn build_rollback_command() -> String {
    r#"BACKUP=$(find /etc/ssh -maxdepth 1 -name 'sshd_config.omnyssh_backup.*' 2>/dev/null | sort | tail -1); \
       if [ -n "$BACKUP" ]; then \
           sudo cp "$BACKUP" /etc/ssh/sshd_config && \
           (sudo systemctl reload sshd 2>/dev/null || sudo systemctl reload ssh 2>/dev/null || sudo service sshd reload 2>/dev/null || sudo service ssh reload); \
       else \
           echo "OMNYSSH_NO_BACKUP"; exit 1; \
       fi"#
    .to_string()
}

// ---------------------------------------------------------------------------
// High-Level Key Setup Orchestrator
// ---------------------------------------------------------------------------

/// Executes the complete key setup process for a host.
///
/// This is the main entry point for Auto SSH Key Setup. It orchestrates all 6 steps
/// with proper error handling, timeouts, and rollback on failure.
///
/// # Errors
/// Returns an error if any critical step fails. Password authentication is never
/// disabled unless key authentication has been verified.
pub async fn setup_key_for_host(
    host: &Host,
    password_session: &SshSession,
    key_type: KeyType,
    progress_tx: Option<tokio::sync::mpsc::Sender<KeySetupStep>>,
) -> Result<KeySetupResult> {
    let options = KeySetupOptions {
        key_type,
        source: KeySource::Generate { file_name: None },
        mode: AuthMode::KeyOnly,
    };
    setup_key_with_options(host, password_session, &options, progress_tx).await
}

/// Installs a key on `host` with the given [`KeySetupOptions`]: a new or an
/// existing key, and either key-only logins or key plus password.
///
/// # Errors
/// As [`setup_key_for_host`]. Password authentication is never disabled unless
/// the installed key was verified to log in on its own.
pub async fn setup_key_with_options(
    host: &Host,
    password_session: &SshSession,
    options: &KeySetupOptions,
    progress_tx: Option<tokio::sync::mpsc::Sender<KeySetupStep>>,
) -> Result<KeySetupResult> {
    let mut machine = KeySetupMachine::new();
    let mut result = KeySetupResult {
        key_path: PathBuf::new(),
        state: KeySetupState::NotStarted,
        error_message: None,
        password_auth_disabled: None,
    };

    // The two verification steps reconnect, so they walk the host's ProxyJump
    // chain like every other connect. A fixed budget would trip on a bastion
    // before the connection had the time the engine grants it per hop.
    let verify_timeout = STEP_TIMEOUT.max(session::connect_budget(host).await);
    let total_timeout = TOTAL_TIMEOUT + verify_timeout.saturating_sub(STEP_TIMEOUT) * 2;

    let error = match time::timeout(
        total_timeout,
        setup_key_internal(
            host,
            password_session,
            options,
            verify_timeout,
            &mut machine,
            progress_tx,
        ),
    )
    .await
    {
        Ok(Ok((key_path, password_disabled))) => {
            result.key_path = key_path;
            result.state = machine.state().clone();
            result.password_auth_disabled = password_disabled;
            return Ok(result);
        }
        Ok(Err(e)) => {
            error!("Key setup failed for {}: {}", host.name, e);
            e
        }
        Err(_) => {
            // Running out of time is only safe before the point of no return.
            // Past it the server has password auth disabled, so the run needs
            // the same rollback a failed final check would get.
            let step = if machine.password_disabled {
                KeySetupStep::FinalCheck
            } else {
                KeySetupStep::VerifyKeyAuth
            };
            machine.step_result(step, Err(anyhow!("timed out")));
            anyhow!(
                "Key setup timed out after {} seconds",
                total_timeout.as_secs()
            )
        }
    };

    result.state = machine.state().clone();
    result.error_message = Some(format!("{:#}", error));

    // Attempt rollback if needed.
    if matches!(machine.state(), KeySetupState::NeedsRollback) {
        if let Err(rollback_err) = emergency_rollback(password_session).await {
            error!("Rollback failed: {}", rollback_err);
            result.error_message = Some(format!(
                "Setup failed AND rollback failed: {}\nRollback error: {}",
                error, rollback_err
            ));
        } else {
            machine.rollback_complete();
            result.state = KeySetupState::RolledBack;
        }
    }

    Err(error)
}

/// Internal implementation of the key setup process. Returns the installed key
/// and, when known, whether the server now refuses password logins.
async fn setup_key_internal(
    host: &Host,
    password_session: &SshSession,
    options: &KeySetupOptions,
    verify_timeout: Duration,
    machine: &mut KeySetupMachine,
    progress_tx: Option<tokio::sync::mpsc::Sender<KeySetupStep>>,
) -> Result<(PathBuf, Option<bool>)> {
    // Step 1: Generate (or pick up) the key pair.
    info!("Step 1/6: Preparing key pair for {}", host.name);
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::GenerateKey).await;
    }
    let prepared = match &options.source {
        KeySource::Generate { file_name } => match file_name {
            Some(name) => generate_named_key_pair(name, &host.name, options.key_type).await,
            None => generate_key_pair(&host.name, options.key_type).await,
        }
        .and_then(|(private, public)| {
            let line =
                std::fs::read_to_string(&public).context("Failed to read public key file")?;
            Ok((private, line.trim().to_string()))
        }),
        KeySource::Existing(path) => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                if !path.is_file() {
                    anyhow::bail!("Key file not found: {}", path.display());
                }
                let line = crate::ssh::keys::public_key_line(&path)?;
                Ok((path, line))
            })
            .await
            .context("key read task panicked")?
        }
    };
    let (private_key_path, public_key_content) = match prepared {
        Ok(paths) => {
            machine.step_result(KeySetupStep::GenerateKey, Ok(()));
            paths
        }
        Err(e) => {
            machine.step_result(KeySetupStep::GenerateKey, Err(anyhow!("Generation failed")));
            return Err(e);
        }
    };

    // Step 2: Copy public key to server.
    info!("Step 2/6: Copying public key to server");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::CopyPublicKey).await;
    }

    // Validate public key format before embedding in shell command.
    let public_key_trimmed = public_key_content.trim();
    if let Err(e) = crate::ssh::keys::validate_public_line(public_key_trimmed) {
        machine.step_result(KeySetupStep::CopyPublicKey, Err(anyhow!("Copy failed")));
        return Err(anyhow!("Public key file: {e}"));
    }

    let copy_cmd = build_authorized_keys_command(public_key_trimmed);
    match time::timeout(STEP_TIMEOUT, password_session.run_command(&copy_cmd)).await {
        Ok(Ok(_)) => {
            machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(KeySetupStep::CopyPublicKey, Err(anyhow!("Copy failed")));
            return Err(anyhow!("Failed to copy public key to server: {}", e));
        }
        Err(_) => {
            machine.step_result(KeySetupStep::CopyPublicKey, Err(anyhow!("Copy failed")));
            return Err(anyhow!("Failed to copy public key to server: timeout"));
        }
    }

    // Step 3: Verify key authentication (CRITICAL). With this key and nothing
    // else — the agent or a default key logging in would prove nothing.
    info!("Step 3/6: Verifying key authentication");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::VerifyKeyAuth).await;
    }
    let mut test_host = host.clone();
    test_host.identity_file = Some(private_key_path.to_string_lossy().to_string());
    test_host.password = None; // Force key-only auth.

    match time::timeout(
        verify_timeout,
        SshSession::connect_with_key_only(&test_host, &private_key_path),
    )
    .await
    {
        Ok(Ok(test_session)) => {
            info!("Key authentication verified successfully!");
            test_session.disconnect().await;
            machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(KeySetupStep::VerifyKeyAuth, Err(anyhow!("Verify failed")));
            return Err(anyhow!(
                "Key authentication verification failed: {}. Password NOT disabled.",
                e
            ));
        }
        Err(_) => {
            machine.step_result(KeySetupStep::VerifyKeyAuth, Err(anyhow!("Verify failed")));
            return Err(anyhow!(
                "Key authentication verification timed out. Password NOT disabled."
            ));
        }
    }

    if options.mode == AuthMode::KeyAndPassword {
        return ensure_password_enabled(
            password_session,
            &test_host,
            &private_key_path,
            verify_timeout,
            machine,
            progress_tx,
        )
        .await
        .map(|disabled| (private_key_path, disabled));
    }

    // Check sudo availability. `run_command_checked` is required here — the
    // probe's exit status is the answer, and plain `run_command` ignores it.
    info!("Checking sudo availability");
    match password_session
        .run_command_checked("sudo -n true 2>/dev/null")
        .await
    {
        Ok(_) => {
            info!("Sudo access confirmed");
        }
        Err(_) => {
            warn!("No sudo access — password authentication will NOT be disabled");
            machine.set_has_sudo(false);
            machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(())); // Trigger PartialSuccess.
            return Ok((private_key_path, None));
        }
    }

    // Step 4: Disable password authentication.
    info!("Step 4/6: Disabling password authentication");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::DisablePassword).await;
    }
    let disable_cmd = build_disable_password_command();
    match time::timeout(STEP_TIMEOUT, password_session.run_command(&disable_cmd)).await {
        Ok(Ok(output)) if output.contains("OMNYSSH_NO_SUDO") => {
            machine.set_has_sudo(false);
            machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));
            return Ok((private_key_path, None));
        }
        Ok(Ok(output)) if output.contains("OMNYSSH_CONFIG_ERROR") => {
            machine.step_result(KeySetupStep::DisablePassword, Err(anyhow!("Config error")));
            return Err(anyhow!("sshd config validation failed. Backup restored."));
        }
        Ok(Ok(_)) => {
            machine.step_result(KeySetupStep::DisablePassword, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(
                KeySetupStep::DisablePassword,
                Err(anyhow!("Disable failed")),
            );
            return Err(anyhow!("Failed to disable password authentication: {}", e));
        }
        Err(_) => {
            machine.step_result(
                KeySetupStep::DisablePassword,
                Err(anyhow!("Disable failed")),
            );
            return Err(anyhow!(
                "Failed to disable password authentication: timeout"
            ));
        }
    }

    // Step 5: Reload sshd.
    info!("Step 5/6: Reloading SSH daemon");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::ReloadSshd).await;
    }
    let reload_cmd = build_reload_sshd_command();
    match time::timeout(STEP_TIMEOUT, password_session.run_command(&reload_cmd)).await {
        Ok(Ok(_)) => {
            machine.step_result(KeySetupStep::ReloadSshd, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(KeySetupStep::ReloadSshd, Err(anyhow!("Reload failed")));
            return Err(anyhow!("Failed to reload SSH daemon: {}", e));
        }
        Err(_) => {
            machine.step_result(KeySetupStep::ReloadSshd, Err(anyhow!("Reload failed")));
            return Err(anyhow!("Failed to reload SSH daemon: timeout"));
        }
    }

    // Step 6: Final check — verify key still works after reload.
    info!("Step 6/6: Final verification");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::FinalCheck).await;
    }
    match time::timeout(
        verify_timeout,
        SshSession::connect_with_key_only(&test_host, &private_key_path),
    )
    .await
    {
        Ok(Ok(final_session)) => {
            info!("Final verification passed! Key setup complete.");
            final_session.disconnect().await;
            machine.step_result(KeySetupStep::FinalCheck, Ok(()));
            Ok((private_key_path, Some(true)))
        }
        Ok(Err(e)) => {
            error!(
                "Final check failed: {}. Password is disabled but key doesn't work!",
                e
            );
            machine.step_result(KeySetupStep::FinalCheck, Err(anyhow!("Final check failed")));
            Err(anyhow!(
                "Final verification failed after disabling password. Attempting rollback."
            ))
        }
        Err(_) => {
            error!("Final check timed out. Password is disabled but key doesn't work!");
            machine.step_result(KeySetupStep::FinalCheck, Err(anyhow!("Final check failed")));
            Err(anyhow!(
                "Final verification timed out after disabling password. Attempting rollback."
            ))
        }
    }
}

/// Why "password and key" mode could not turn passwords back on.
const NEEDS_SUDO: &str =
    "Enabling password logins needs passwordless sudo on the server. The key is installed.";

/// "Password and key" mode, after the key is verified: make sure the server
/// still accepts passwords, turning them back on if an earlier key-only setup
/// (or anything else) disabled them. Returns whether passwords are refused
/// (`Some(false)` once confirmed on; `None` when sshd could not be asked —
/// without passwordless sudo there is nothing to change either way).
async fn ensure_password_enabled(
    password_session: &SshSession,
    test_host: &Host,
    key_path: &std::path::Path,
    verify_timeout: Duration,
    machine: &mut KeySetupMachine,
    progress_tx: Option<tokio::sync::mpsc::Sender<KeySetupStep>>,
) -> Result<Option<bool>> {
    let probe = time::timeout(
        STEP_TIMEOUT,
        password_session.run_command(PROBE_PASSWORD_AUTH),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default()
    .to_lowercase();
    if probe.contains("passwordauthentication yes") {
        machine.step_result(KeySetupStep::FinalCheck, Ok(()));
        return Ok(Some(false));
    }
    if !probe.contains("passwordauthentication no") {
        // No sudo (or no sshd -T): the key is installed; logins are as they were.
        info!("Could not read sshd's password setting; leaving it unchanged");
        machine.step_result(KeySetupStep::FinalCheck, Ok(()));
        return Ok(None);
    }

    info!("Password authentication is off — enabling it");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::EnablePassword).await;
    }
    let enable = time::timeout(
        STEP_TIMEOUT,
        password_session.run_command(&build_enable_password_command()),
    )
    .await;
    match enable {
        Ok(Ok(out)) if out.contains("OMNYSSH_NO_SUDO") => return Err(anyhow!(NEEDS_SUDO)),
        Ok(Ok(out)) if out.contains("OMNYSSH_CONFIG_ERROR") => {
            return Err(anyhow!("sshd config validation failed. Backup restored."))
        }
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(anyhow!("Failed to enable password authentication: {e}")),
        Err(_) => return Err(anyhow!("Failed to enable password authentication: timeout")),
    }

    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::ReloadSshd).await;
    }
    match time::timeout(
        STEP_TIMEOUT,
        password_session.run_command(&build_reload_sshd_command()),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(anyhow!("Failed to reload SSH daemon: {e}")),
        Err(_) => return Err(anyhow!("Failed to reload SSH daemon: timeout")),
    }

    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::FinalCheck).await;
    }
    match time::timeout(
        verify_timeout,
        SshSession::connect_with_key_only(test_host, key_path),
    )
    .await
    {
        Ok(Ok(session)) => {
            session.disconnect().await;
            machine.step_result(KeySetupStep::FinalCheck, Ok(()));
            Ok(Some(false))
        }
        Ok(Err(e)) => Err(anyhow!("The key stopped working after the reload: {e}")),
        Err(_) => Err(anyhow!("The key check timed out after the reload")),
    }
}

/// Attempts to rollback sshd_config to the most recent OmnySSH backup.
async fn emergency_rollback(session: &SshSession) -> Result<()> {
    warn!("Attempting emergency rollback of sshd_config");
    let rollback_cmd = build_rollback_command();

    match time::timeout(STEP_TIMEOUT, session.run_command(&rollback_cmd)).await {
        Ok(Ok(output)) if output.contains("OMNYSSH_NO_BACKUP") => {
            Err(anyhow!("No backup file found for rollback"))
        }
        Ok(Ok(_)) => {
            info!("Rollback successful — password authentication restored");
            Ok(())
        }
        Ok(Err(e)) => Err(anyhow!("Rollback failed: {}", e)),
        Err(_) => Err(anyhow!("Rollback failed: timeout")),
    }
}

// ---------------------------------------------------------------------------
// Result Type
// ---------------------------------------------------------------------------

/// Result of the key setup process.
#[derive(Debug, Clone)]
pub struct KeySetupResult {
    /// Path to the generated private key file.
    pub key_path: PathBuf,
    /// Final state of the setup process.
    pub state: KeySetupState,
    /// Error message if the setup failed.
    pub error_message: Option<String>,
    /// Whether the server now refuses password logins, when known.
    pub password_auth_disabled: Option<bool>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_file_names_are_plain_files_in_dot_ssh() {
        for ok in ["work", "id_ed25519_github", "prod-2024.key"] {
            assert!(validate_key_file_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "../id",
            "a/b",
            ".hidden",
            "-rf",
            "key.pub",
            "known_hosts",
            "config",
            "имя",
            "a b",
        ] {
            assert!(
                validate_key_file_name(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn test_sanitize_hostname() {
        assert_eq!(sanitize_hostname("web-prod-1"), "web-prod-1");
        assert_eq!(sanitize_hostname("my server (prod)"), "my_server__prod_");
        assert_eq!(sanitize_hostname("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_hostname(""), "unnamed_host");
        assert_eq!(sanitize_hostname("a/b\\c:d*e?f"), "a_b_c_d_e_f");

        // Test truncation.
        let long_name = "a".repeat(100);
        assert_eq!(sanitize_hostname(&long_name).len(), MAX_HOSTNAME_LENGTH);
    }

    #[tokio::test]
    #[ignore] // Run manually with: cargo test test_key_generation_and_conversion -- --ignored
    async fn test_key_generation_and_conversion() {
        // Generate a test key pair
        let test_host = "test_conversion_host";
        let result = generate_key_pair(test_host, KeyType::Ed25519).await;

        assert!(result.is_ok(), "Key generation should succeed");
        let (private_key_path, public_key_path) = result.unwrap();

        // Check that files exist
        assert!(private_key_path.exists(), "Private key file should exist");
        assert!(public_key_path.exists(), "Public key file should exist");

        // Read the private key and check format
        let private_key_content = tokio::fs::read_to_string(&private_key_path).await.unwrap();
        let first_line = private_key_content.lines().next().unwrap();

        // After conversion, it should be OpenSSH format, not PKCS#8
        assert!(
            first_line.contains("BEGIN OPENSSH PRIVATE KEY")
                || first_line.contains("BEGIN PRIVATE KEY"),
            "Key should be in OpenSSH or PKCS#8 format, got: {}",
            first_line
        );

        // Try to extract public key using ssh-keygen (validates the key is readable by OpenSSH)
        let output = tokio::process::Command::new("ssh-keygen")
            .args(["-y", "-f"])
            .arg(&private_key_path)
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "ssh-keygen should be able to read the generated key. stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Cleanup
        let _ = tokio::fs::remove_file(&private_key_path).await;
        let _ = tokio::fs::remove_file(&public_key_path).await;
    }

    #[test]
    fn test_key_setup_step_ordering() {
        let steps = KeySetupStep::all_steps();
        assert_eq!(steps[0], KeySetupStep::GenerateKey);
        assert_eq!(steps[5], KeySetupStep::FinalCheck);
        assert_eq!(steps.len(), 6);
    }

    #[test]
    fn test_key_setup_machine_verify_failure_stops_process() {
        let mut machine = KeySetupMachine::new();
        machine.step_result(KeySetupStep::GenerateKey, Ok(()));
        machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        machine.step_result(
            KeySetupStep::VerifyKeyAuth,
            Err(anyhow!("connection refused")),
        );

        // Password must NOT be disabled.
        assert_eq!(machine.state(), &KeySetupState::FailedSafe);
        assert!(!machine.password_disabled);
    }

    #[test]
    fn test_key_setup_machine_no_sudo_partial_success() {
        let mut machine = KeySetupMachine::new();
        machine.set_has_sudo(false);

        machine.step_result(KeySetupStep::GenerateKey, Ok(()));
        machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));

        // Key works but no sudo → PartialSuccess.
        assert_eq!(machine.state(), &KeySetupState::PartialSuccess);
        assert!(!machine.password_disabled);
    }

    #[test]
    fn test_key_setup_machine_final_check_failure_triggers_rollback() {
        let mut machine = KeySetupMachine::new();
        machine.step_result(KeySetupStep::GenerateKey, Ok(()));
        machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));
        machine.step_result(KeySetupStep::DisablePassword, Ok(()));
        machine.step_result(KeySetupStep::ReloadSshd, Ok(()));
        machine.step_result(KeySetupStep::FinalCheck, Err(anyhow!("timeout")));

        // Password is disabled but key doesn't work → rollback needed.
        assert_eq!(machine.state(), &KeySetupState::NeedsRollback);
        assert!(machine.password_disabled);
    }

    #[test]
    fn test_authorized_keys_command_escapes_quotes() {
        let pubkey = "ssh-ed25519 AAAA... user's key";
        let cmd = build_authorized_keys_command(pubkey);

        // Should escape single quotes.
        assert!(cmd.contains("user'\\''s"));
        // Should use >> (append, not overwrite).
        assert!(cmd.contains(">> ~/.ssh/authorized_keys"));
        // Should NOT use > (overwrite).
        assert!(!cmd.contains(" > ~/.ssh/authorized_keys"));
    }

    #[test]
    fn test_disable_password_command_creates_backup() {
        let cmd = build_disable_password_command();

        // Should create timestamped backup.
        assert!(cmd.contains("omnyssh_backup."));
        // Should run sshd -t for validation.
        assert!(cmd.contains("sshd -t"));
        // Should comment out cloud-init Include directive to prevent overrides.
        assert!(
            cmd.contains("'s|^Include /etc/ssh/sshd_config.d/|#Include /etc/ssh/sshd_config.d/|'")
        );
        // Should disable all password auth methods.
        assert!(cmd.contains("PasswordAuthentication no"));
        assert!(cmd.contains("ChallengeResponseAuthentication no"));
        assert!(cmd.contains("KbdInteractiveAuthentication no"));
        // Should disable PAM to prevent bypassing password auth.
        assert!(cmd.contains("UsePAM no"));
    }

    #[test]
    fn test_disable_password_command_adds_missing_directives() {
        let cmd = build_disable_password_command();

        // Each directive is prepended when the config contains no occurrence.
        assert!(cmd.contains("grep -qE '^PasswordAuthentication[[:space:]]'"));
        assert!(cmd.contains("sed -i '1i PasswordAuthentication no'"));
        assert!(cmd.contains("grep -qE '^UsePAM[[:space:]]'"));
        assert!(cmd.contains("sed -i '1i UsePAM no'"));
    }

    #[test]
    fn test_reload_sshd_uses_reload_not_restart() {
        let cmd = build_reload_sshd_command();

        // Should use reload, not restart.
        assert!(cmd.contains("reload"));
        assert!(!cmd.contains("restart"));
    }
}
