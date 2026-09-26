//! Shell commands for file operations SFTP has no verb for — recursive delete,
//! archives, `chmod -R` — run over an exec channel on the SFTP tab's connection.
//!
//! Every path goes into the command single-quoted (see [`quote`]), and every
//! builder refuses targets that would be catastrophic to get wrong (`/`, empty,
//! relative, `.`/`..` components), so a hostile or odd file name can neither
//! inject shell syntax nor widen what a command touches.

use anyhow::{bail, Result};

/// Archive formats the file manager creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarGz,
    Zip,
}

impl ArchiveFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::TarGz => ".tar.gz",
            Self::Zip => ".zip",
        }
    }
}

/// Single-quote `s` for a POSIX shell: `it's` → `'it'\''s'`.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// An absolute remote path that is safe to act on.
fn checked_path(path: &str) -> Result<&str> {
    let trimmed = path.trim_end_matches('/');
    if !path.starts_with('/') {
        bail!("not an absolute path: {path}");
    }
    if trimmed.is_empty() {
        bail!("refusing to operate on /");
    }
    if path.contains('\0') || path.contains('\n') {
        bail!("the path contains invalid characters");
    }
    if trimmed.split('/').any(|c| c == "." || c == "..") {
        bail!("the path contains '.' or '..': {path}");
    }
    Ok(trimmed)
}

/// A plain name inside a folder (no slashes, not `.`/`..`).
fn checked_name(name: &str) -> Result<&str> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0', '\n']) {
        bail!("invalid file name: {name:?}");
    }
    Ok(name)
}

fn quoted_paths(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        bail!("nothing selected");
    }
    Ok(paths
        .iter()
        .map(|p| checked_path(p).map(quote))
        .collect::<Result<Vec<_>>>()?
        .join(" "))
}

/// Fails with a clear message when `tool` is missing on the server.
fn require(tool: &str) -> String {
    format!(
        "command -v {tool} >/dev/null 2>&1 || {{ echo '{tool} is not installed on the server' >&2; exit 127; }}"
    )
}

/// Delete files and folders (recursively).
pub fn delete_command(paths: &[String]) -> Result<String> {
    Ok(format!("rm -rf -- {}", quoted_paths(paths)?))
}

/// Pack `names` (entries of `dir`) into `dir/archive`. Paths inside the archive
/// are relative to `dir`, so it unpacks into a folder of the user's choosing.
pub fn compress_command(
    dir: &str,
    names: &[String],
    archive: &str,
    format: ArchiveFormat,
) -> Result<String> {
    let dir = if dir == "/" { "/" } else { checked_path(dir)? };
    let archive = checked_name(archive)?;
    if names.is_empty() {
        bail!("nothing selected");
    }
    let members = names
        .iter()
        .map(|n| checked_name(n).map(quote))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let pack = match format {
        ArchiveFormat::TarGz => format!("tar -czf {} -- {members}", quote(archive)),
        ArchiveFormat::Zip => format!(
            "{} && zip -rqy {} -- {members}",
            require("zip"),
            quote(archive)
        ),
    };
    Ok(format!(
        "cd -- {} || exit 1; [ ! -e {a} ] || {{ echo {exists} >&2; exit 1; }}; {pack}",
        quote(dir),
        a = quote(archive),
        exists = quote(&format!("{archive} already exists")),
    ))
}

/// Whether the file manager can unpack `name`.
pub fn is_archive(name: &str) -> bool {
    extract_tool(name).is_some()
}

fn extract_tool(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    const TAR: &[&str] = &[
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz", ".tbz2", ".tar.xz", ".txz", ".tar.zst",
    ];
    if TAR.iter().any(|e| n.ends_with(e)) {
        Some("tar")
    } else if n.ends_with(".zip") {
        Some("unzip")
    } else if n.ends_with(".gz") {
        Some("gunzip")
    } else {
        None
    }
}

/// Unpack `archive` into `dest`. Existing files are overwritten, like
/// double-clicking an archive in a desktop file manager would not — so the UI
/// asks first.
pub fn extract_command(archive: &str, dest: &str) -> Result<String> {
    let archive = checked_path(archive)?;
    let dest = if dest == "/" {
        "/"
    } else {
        checked_path(dest)?
    };
    let name = archive.rsplit('/').next().unwrap_or(archive);
    let unpack = match extract_tool(name) {
        Some("tar") => format!("tar -xf {}", quote(archive)),
        Some("unzip") => format!("{} && unzip -oq {}", require("unzip"), quote(archive)),
        // A lone .gz becomes its name without .gz, next to the original (kept).
        Some(_) => format!("gunzip -kf {}", quote(archive)),
        None => bail!("{name} is not an archive the file manager can unpack"),
    };
    Ok(format!("cd -- {} && {unpack}", quote(dest)))
}

/// `chmod [-R] <mode> -- paths`. `mode` is permission bits (at most `0o7777`).
pub fn chmod_command(paths: &[String], mode: u32, recursive: bool) -> Result<String> {
    if mode > 0o7777 {
        bail!("invalid mode {mode:o}");
    }
    let flag = if recursive { "-R " } else { "" };
    // `--` before the mode: BSD chmod stops option parsing at the mode, so a
    // `--` after it would be taken for a file name.
    Ok(format!("chmod {flag}-- {mode:o} {}", quoted_paths(paths)?))
}

/// Create an empty file, refusing to touch one that exists.
pub fn new_file_command(path: &str) -> Result<String> {
    let p = quote(checked_path(path)?);
    Ok(format!(
        "[ ! -e {p} ] || {{ echo 'a file with this name already exists' >&2; exit 1; }}; : > {p}"
    ))
}

/// Copy `paths` into `dest_dir` on the server (keeping modes and times).
pub fn copy_command(paths: &[String], dest_dir: &str) -> Result<String> {
    let dest = if dest_dir == "/" {
        "/"
    } else {
        checked_path(dest_dir)?
    };
    Ok(format!(
        "cp -Rp -- {} {}",
        quoted_paths(paths)?,
        quote(dest)
    ))
}

/// Move `paths` into `dest_dir` on the server.
pub fn move_command(paths: &[String], dest_dir: &str) -> Result<String> {
    let dest = if dest_dir == "/" {
        "/"
    } else {
        checked_path(dest_dir)?
    };
    Ok(format!("mv -- {} {}", quoted_paths(paths)?, quote(dest)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quoting_neutralises_shell_syntax() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
    }

    #[test]
    fn delete_refuses_dangerous_targets() {
        assert_eq!(
            delete_command(&v(&["/srv/a b", "/tmp/x'y"])).unwrap(),
            "rm -rf -- '/srv/a b' '/tmp/x'\\''y'"
        );
        for bad in ["/", "//", "", "relative", "/srv/../etc", "/srv/./x"] {
            assert!(
                delete_command(&v(&[bad])).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(delete_command(&[]).is_err());
    }

    #[test]
    fn compress_uses_relative_members_and_refuses_overwrite() {
        let cmd = compress_command(
            "/srv/app",
            &v(&["logs", "a.txt"]),
            "backup.tar.gz",
            ArchiveFormat::TarGz,
        )
        .unwrap();
        assert!(cmd.contains("cd -- '/srv/app'"));
        assert!(cmd.contains("tar -czf 'backup.tar.gz' -- 'logs' 'a.txt'"));
        assert!(cmd.contains("already exists"));
        let zip = compress_command("/", &v(&["etc"]), "etc.zip", ArchiveFormat::Zip).unwrap();
        assert!(zip.contains("command -v zip"));
        assert!(compress_command("/srv", &v(&["../etc"]), "x.tgz", ArchiveFormat::TarGz).is_err());
        assert!(compress_command("/srv", &v(&["a"]), "sub/x.tgz", ArchiveFormat::TarGz).is_err());
    }

    #[test]
    fn extract_picks_the_tool_by_extension() {
        assert_eq!(
            extract_command("/srv/site.tar.gz", "/srv").unwrap(),
            "cd -- '/srv' && tar -xf '/srv/site.tar.gz'"
        );
        assert!(extract_command("/srv/a.ZIP", "/srv")
            .unwrap()
            .contains("unzip -oq '/srv/a.ZIP'"));
        assert!(extract_command("/srv/dump.sql.gz", "/srv")
            .unwrap()
            .contains("gunzip -kf"));
        assert!(extract_command("/srv/readme.txt", "/srv").is_err());
        assert!(is_archive("x.tgz") && is_archive("x.tar.xz") && !is_archive("x.txt"));
    }

    #[test]
    fn chmod_formats_octal_and_validates() {
        assert_eq!(
            chmod_command(&v(&["/srv/run.sh"]), 0o755, false).unwrap(),
            "chmod -- 755 '/srv/run.sh'"
        );
        assert_eq!(
            chmod_command(&v(&["/srv/www"]), 0o2775, true).unwrap(),
            "chmod -R -- 2775 '/srv/www'"
        );
        assert!(chmod_command(&v(&["/srv"]), 0o17777, false).is_err());
    }

    #[test]
    fn new_file_copy_and_move() {
        assert!(new_file_command("/srv/notes.md")
            .unwrap()
            .ends_with(": > '/srv/notes.md'"));
        assert!(new_file_command("notes.md").is_err());
        assert_eq!(
            copy_command(&v(&["/srv/a"]), "/backup").unwrap(),
            "cp -Rp -- '/srv/a' '/backup'"
        );
        assert_eq!(
            move_command(&v(&["/srv/a"]), "/").unwrap(),
            "mv -- '/srv/a' '/'"
        );
    }
}
