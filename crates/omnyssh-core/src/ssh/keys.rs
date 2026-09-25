//! Finding the user's SSH keys, and reading a key's public half.
//!
//! [`discover_keys`] lists the private keys in `~/.ssh` (by content, not by name,
//! so `work`, `github_ed25519` and `id_rsa` all show up) for the desktop app's key
//! pickers. [`public_key_line`] returns the `authorized_keys` line for a private
//! key, from its `.pub` sibling or — when that is missing — derived from the key.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use russh_keys::PublicKeyBase64;

/// A private key found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshKeyInfo {
    pub path: PathBuf,
    /// File name, e.g. `id_ed25519`.
    pub name: String,
    /// `ed25519`, `rsa`, `ecdsa`, … — from the public key; `None` if unknown.
    pub kind: Option<String>,
    /// The public key's comment (often `user@host`).
    pub comment: Option<String>,
    /// Protected by a passphrase (the app cannot use it without the agent).
    pub encrypted: bool,
}

/// Private keys are small; anything larger is not one.
const MAX_KEY_BYTES: u64 = 64 * 1024;

const PRIVATE_MARKERS: &[&str] = &[
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "-----BEGIN RSA PRIVATE KEY-----",
    "-----BEGIN EC PRIVATE KEY-----",
    "-----BEGIN DSA PRIVATE KEY-----",
    "-----BEGIN PRIVATE KEY-----",
    "-----BEGIN ENCRYPTED PRIVATE KEY-----",
];

/// The private keys in `~/.ssh`, sorted by name.
pub fn discover_keys() -> Vec<SshKeyInfo> {
    let Some(dir) = dirs::home_dir().map(|h| h.join(".ssh")) else {
        return Vec::new();
    };
    discover_keys_in(&dir)
}

/// The private keys directly inside `dir`, sorted by name.
pub fn discover_keys_in(dir: &Path) -> Vec<SshKeyInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut keys: Vec<SshKeyInfo> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_none_or(|ext| ext != "pub"))
        .filter_map(|p| inspect_key(&p))
        .collect();
    keys.sort_by_key(|k| k.name.to_lowercase());
    keys
}

/// Describe `path` if it is a private key file.
pub fn inspect_key(path: &Path) -> Option<SshKeyInfo> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_KEY_BYTES || meta.len() == 0 {
        return None;
    }
    let mut head = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(256)
        .read_to_string(&mut head)
        .ok()?;
    let head = head.trim_start();
    if !PRIVATE_MARKERS.iter().any(|m| head.starts_with(m)) {
        return None;
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (mut kind, comment) = read_public_sibling(path)
        .map(|line| parse_public_line(&line))
        .unwrap_or((None, None));
    let loaded = russh_keys::load_secret_key(path, None);
    let encrypted = loaded.is_err() && head.contains("ENCRYPTED")
        || matches!(loaded, Err(russh_keys::Error::KeyIsEncrypted));
    if kind.is_none() {
        if let Ok(key) = &loaded {
            kind = Some(short_kind(key.name()));
        }
    }
    Some(SshKeyInfo {
        path: path.to_path_buf(),
        name,
        kind,
        comment,
        encrypted,
    })
}

fn read_public_sibling(private: &Path) -> Option<String> {
    let mut pub_path = private.as_os_str().to_owned();
    pub_path.push(".pub");
    let text = std::fs::read_to_string(PathBuf::from(pub_path)).ok()?;
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// `ssh-ed25519 AAAA… comment` → (`ed25519`, `comment`).
fn parse_public_line(line: &str) -> (Option<String>, Option<String>) {
    let mut parts = line.split_whitespace();
    let kind = parts.next().map(short_kind);
    let _blob = parts.next();
    let comment: Vec<&str> = parts.collect();
    let comment = (!comment.is_empty()).then(|| comment.join(" "));
    (kind, comment)
}

fn short_kind(algorithm: &str) -> String {
    let a = algorithm.trim_start_matches("ssh-");
    if a.starts_with("ecdsa") {
        "ecdsa".to_string()
    } else if a.starts_with("sk-ssh-ed25519") || a.starts_with("sk-ed25519") {
        "ed25519-sk".to_string()
    } else if a.starts_with("sk-ecdsa") {
        "ecdsa-sk".to_string()
    } else {
        a.to_string()
    }
}

/// The single-line public key (`ssh-ed25519 AAAA… comment`) for a private key:
/// its `.pub` sibling if present, otherwise derived from the (unencrypted) key.
///
/// # Errors
/// The key cannot be read, is passphrase-protected without a `.pub` next to it,
/// or the `.pub` is not a single valid public key line.
pub fn public_key_line(private: &Path) -> anyhow::Result<String> {
    if let Some(line) = read_public_sibling(private) {
        validate_public_line(&line)?;
        return Ok(line);
    }
    let key = russh_keys::load_secret_key(private, None).map_err(|e| match e {
        russh_keys::Error::KeyIsEncrypted => anyhow!(
            "{} is protected by a passphrase and has no .pub file next to it",
            private.display()
        ),
        e => anyhow!("could not read {}: {e}", private.display()),
    })?;
    let public = key.clone_public_key().context("derive the public key")?;
    let line = format!("{} {}", public.name(), public.public_key_base64());
    validate_public_line(&line)?;
    Ok(line)
}

/// A public key line safe to put into `authorized_keys` via a shell command.
pub fn validate_public_line(line: &str) -> anyhow::Result<()> {
    let line = line.trim();
    // Single quotes are fine: the authorized_keys command escapes them.
    if line.contains(['\n', '\r', '\0']) {
        anyhow::bail!("the public key contains invalid characters");
    }
    let known = [
        "ssh-ed25519 ",
        "ssh-rsa ",
        "ecdsa-sha2-",
        "sk-ssh-ed25519@openssh.com ",
        "sk-ecdsa-sha2-",
    ];
    if !known.iter().any(|k| line.starts_with(k)) {
        anyhow::bail!("unrecognised public key type");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_lines_parse_into_kind_and_comment() {
        assert_eq!(
            parse_public_line("ssh-ed25519 AAAAC3Nz me@laptop"),
            (Some("ed25519".into()), Some("me@laptop".into()))
        );
        assert_eq!(
            parse_public_line("ecdsa-sha2-nistp256 AAAA"),
            (Some("ecdsa".into()), None)
        );
        assert_eq!(
            parse_public_line("ssh-rsa AAAA a b").1.as_deref(),
            Some("a b")
        );
    }

    #[test]
    fn unsafe_public_lines_are_refused() {
        assert!(validate_public_line("ssh-ed25519 AAAA me").is_ok());
        assert!(validate_public_line("ssh-ed25519 AAAA\r rm -rf ~").is_err());
        assert!(validate_public_line("ssh-ed25519 AAAA\nssh-rsa BBBB").is_err());
        assert!(validate_public_line("garbage").is_err());
    }

    #[test]
    fn discovery_finds_private_keys_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "me@test", "-f"])
            .arg(d.join("work"))
            .status();
        if !status.is_ok_and(|s| s.success()) {
            return; // No ssh-keygen on this machine.
        }
        std::fs::write(d.join("config"), "Host *\n").unwrap();
        std::fs::write(d.join("known_hosts"), "x ssh-ed25519 AAAA\n").unwrap();
        let keys = discover_keys_in(d);
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert_eq!(keys[0].name, "work");
        assert_eq!(keys[0].kind.as_deref(), Some("ed25519"));
        assert_eq!(keys[0].comment.as_deref(), Some("me@test"));
        assert!(!keys[0].encrypted);

        // The .pub is optional: the public line is derived from the key itself.
        let from_pub = public_key_line(&d.join("work")).unwrap();
        std::fs::remove_file(d.join("work.pub")).unwrap();
        let derived = public_key_line(&d.join("work")).unwrap();
        let blob = |l: &str| l.split_whitespace().nth(1).unwrap().to_string();
        assert_eq!(blob(&from_pub), blob(&derived));
    }
}
