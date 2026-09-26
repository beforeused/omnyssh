//! OpenVPN through Tunnelblick (macOS).
//!
//! A host may name a Tunnelblick configuration; connecting to that host first
//! brings the VPN up (and waits until Tunnelblick reports it connected). The
//! app drives Tunnelblick over AppleScript, so it needs neither root nor its
//! own OpenVPN — Tunnelblick already holds the privileges and the credentials.
//! VPNs this app started are tracked so they can be disconnected on quit.
//!
//! [`install_tunnelblick`] fetches the official, notarised release, checks it
//! against a pinned SHA-256 before anything is copied, and installs it into
//! `/Applications`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

/// The Tunnelblick release [`install_tunnelblick`] installs, and its SHA-256 as
/// published on <https://tunnelblick.net/downloads.html>.
pub const TUNNELBLICK_VERSION: &str = "9.0.1 (build 6491)";
const TUNNELBLICK_URLS: &[&str] = &[
    "https://tunnelblick.net/iprelease/Tunnelblick_9.0.1_build_6491.dmg",
    "https://github.com/Tunnelblick/Tunnelblick/releases/download/v9.0.1/Tunnelblick_9.0.1_build_6491.dmg",
];
const TUNNELBLICK_SHA256: &str = "73ca843b7720cd9261a7fd6cf53b0901a06f69c25824ef09430e4094f0688ad0";

/// How long to wait for Tunnelblick to report a configuration connected.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

/// Configurations this app connected (and so disconnects on quit).
static STARTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Whether Tunnelblick can be used on this OS at all.
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Where Tunnelblick is installed, if it is.
pub fn tunnelblick_path() -> Option<PathBuf> {
    if !supported() {
        return None;
    }
    let mut candidates = vec![PathBuf::from("/Applications/Tunnelblick.app")];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Applications/Tunnelblick.app"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

pub fn tunnelblick_installed() -> bool {
    tunnelblick_path().is_some()
}

/// An AppleScript string literal for `s`.
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

async fn osascript(script: &str) -> Result<String> {
    if !supported() {
        bail!("Tunnelblick is only available on macOS");
    }
    if !tunnelblick_installed() {
        bail!("Tunnelblick is not installed");
    }
    let out = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .await
        .context("run osascript")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("-1743") || err.contains("Not authorized") {
            bail!("OmnySSH is not allowed to control Tunnelblick — allow it in System Settings → Privacy & Security → Automation");
        }
        bail!("Tunnelblick: {err}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The configuration names Tunnelblick knows.
///
/// Asks for `name of configurations` in one go and joins the list with line feeds:
/// Tunnelblick's scripting does not implement `count`, so `repeat with c in
/// configurations` fails with -1708.
pub async fn configurations() -> Result<Vec<String>> {
    let out = osascript(
        r#"tell application "Tunnelblick" to set names to name of configurations
set AppleScript's text item delimiters to linefeed
return names as text"#,
    )
    .await?;
    let mut names: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    Ok(names)
}

/// Tunnelblick's state for `name` (`CONNECTED`, `EXITING`, `GET_CONFIG`, …).
pub async fn state(name: &str) -> Result<String> {
    osascript(&format!(
        "tell application \"Tunnelblick\" to get state of first configuration where name = {}",
        applescript_string(name)
    ))
    .await
    .map(|s| s.trim().to_uppercase())
}

/// Make sure `name` is connected, connecting it if needed, and wait until it is.
pub async fn ensure_up(name: &str) -> Result<()> {
    if state(name).await.is_ok_and(|s| s == "CONNECTED") {
        return Ok(());
    }
    tracing::info!("connecting VPN '{name}' through Tunnelblick");
    let accepted = osascript(&format!(
        "tell application \"Tunnelblick\" to connect {}",
        applescript_string(name)
    ))
    .await?;
    if accepted.trim() == "false" {
        bail!("Tunnelblick has no configuration named '{name}'");
    }
    if let Ok(mut started) = STARTED.lock() {
        started
            .get_or_insert_with(HashSet::new)
            .insert(name.to_string());
    }
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = state(name).await.unwrap_or_default();
        if now == "CONNECTED" {
            // Let routes settle before the SSH connect goes out.
            tokio::time::sleep(Duration::from_millis(300)).await;
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            bail!(
                "VPN '{name}' did not connect within {} s (state: {now})",
                CONNECT_TIMEOUT.as_secs()
            );
        }
    }
}

/// Whether `name` is connected right now (never starts it).
pub async fn is_up(name: &str) -> bool {
    state(name).await.is_ok_and(|s| s == "CONNECTED")
}

pub async fn disconnect(name: &str) -> Result<()> {
    osascript(&format!(
        "tell application \"Tunnelblick\" to disconnect {}",
        applescript_string(name)
    ))
    .await?;
    if let Ok(mut started) = STARTED.lock() {
        if let Some(set) = started.as_mut() {
            set.remove(name);
        }
    }
    Ok(())
}

/// Disconnect every VPN this app brought up (on quit).
pub async fn disconnect_started() {
    let names: Vec<String> = STARTED
        .lock()
        .ok()
        .and_then(|mut s| s.take())
        .map(|s| s.into_iter().collect())
        .unwrap_or_default();
    for name in names {
        let _ = disconnect(&name).await;
    }
}

/// Hand an `.ovpn`/`.conf`/`.tblk` file to Tunnelblick, which installs it after
/// asking the user (only them or all users, admin password).
pub async fn import_configuration(path: &Path) -> Result<()> {
    if !tunnelblick_installed() {
        bail!("Tunnelblick is not installed");
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "ovpn" | "conf" | "tblk") {
        bail!("choose an .ovpn, .conf or .tblk file");
    }
    let status = tokio::process::Command::new("open")
        .arg("-a")
        .arg("Tunnelblick")
        .arg(path)
        .status()
        .await
        .context("open the configuration in Tunnelblick")?;
    if !status.success() {
        bail!("Tunnelblick did not accept {}", path.display());
    }
    Ok(())
}

/// Install progress for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStage {
    Downloading { done: u64, total: u64 },
    Verifying,
    Installing,
}

/// Download the pinned Tunnelblick release, verify its SHA-256, and copy
/// `Tunnelblick.app` into `/Applications`; then launch it so it can finish its
/// own setup (it asks for an administrator password once).
pub async fn install_tunnelblick(progress: impl Fn(InstallStage)) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    if !supported() {
        bail!("Tunnelblick is only available on macOS");
    }
    if let Some(path) = tunnelblick_path() {
        return Ok(path);
    }
    let dir = std::env::temp_dir().join(format!("omnyssh-tunnelblick-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await?;
    let dmg = dir.join("Tunnelblick.dmg");

    let client = reqwest::Client::builder()
        .user_agent(concat!("OmnySSH/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut last_err = None;
    let mut downloaded = false;
    for url in TUNNELBLICK_URLS {
        let attempt = async {
            let mut resp = client.get(*url).send().await?.error_for_status()?;
            let total = resp.content_length().unwrap_or(0);
            let mut file = tokio::fs::File::create(&dmg).await?;
            let mut hasher = Sha256::new();
            let mut done = 0u64;
            while let Some(chunk) = resp.chunk().await? {
                hasher.update(&chunk);
                file.write_all(&chunk).await?;
                done += chunk.len() as u64;
                progress(InstallStage::Downloading { done, total });
            }
            file.flush().await?;
            progress(InstallStage::Verifying);
            let digest = format!("{:x}", hasher.finalize());
            if digest != TUNNELBLICK_SHA256 {
                bail!("the download does not match the published checksum");
            }
            anyhow::Ok(())
        };
        match attempt.await {
            Ok(()) => {
                downloaded = true;
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    if !downloaded {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(last_err.unwrap_or_else(|| anyhow!("download failed")))
            .context("download Tunnelblick");
    }

    progress(InstallStage::Installing);
    let mount = dir.join("mnt");
    let result = async {
        run(
            "hdiutil",
            &[
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
            ],
            &[&mount, &dmg],
        )
        .await?;
        let copied = async {
            let app = find_app(&mount)
                .ok_or_else(|| anyhow!("Tunnelblick.app not found in the download"))?;
            let target = PathBuf::from("/Applications/Tunnelblick.app");
            run("ditto", &[], &[&app, &target])
                .await
                .context("copy Tunnelblick into /Applications")?;
            anyhow::Ok(target)
        }
        .await;
        let _ = run("hdiutil", &["detach", "-quiet"], &[&mount]).await;
        copied
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let target = result?;
    let _ = tokio::process::Command::new("open")
        .arg(&target)
        .status()
        .await;
    Ok(target)
}

fn find_app(mount: &Path) -> Option<PathBuf> {
    let direct = mount.join("Tunnelblick.app");
    if direct.is_dir() {
        return Some(direct);
    }
    std::fs::read_dir(mount)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
}

async fn run(cmd: &str, args: &[&str], paths: &[&Path]) -> Result<()> {
    let out = tokio::process::Command::new(cmd)
        .args(args)
        .args(paths)
        .output()
        .await
        .with_context(|| format!("run {cmd}"))?;
    if !out.status.success() {
        bail!(
            "{cmd} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_strings_are_escaped() {
        assert_eq!(applescript_string("office"), "\"office\"");
        assert_eq!(applescript_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn the_pinned_checksum_is_a_sha256() {
        assert_eq!(TUNNELBLICK_SHA256.len(), 64);
        assert!(TUNNELBLICK_SHA256.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
