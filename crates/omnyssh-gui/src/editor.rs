//! Opening files in the user's chosen editor (Settings → Files), and finding the
//! editors installed on this machine to offer in that setting.
//!
//! Programs are always spawned directly with an argument list — never through a
//! shell — so a file name can never be interpreted as a command.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::dto::{EditorAppDto, EditorDto};

/// Open `file` with `editor`.
pub fn open(editor: &EditorDto, file: &Path) -> Result<(), String> {
    match editor {
        EditorDto::System => tauri_plugin_opener::open_path(file, None::<&str>)
            .map_err(|e| format!("could not open {}: {e}", file.display())),
        EditorDto::App { name, path } => tauri_plugin_opener::open_path(file, Some(path))
            .map_err(|e| format!("could not open {} in {name}: {e}", file.display())),
        EditorDto::Command { command } => {
            let argv = command_argv(command, file)?;
            let program = resolve_program(&argv[0]);
            spawn_detached(Command::new(program).args(&argv[1..]))
                .map_err(|e| format!("could not run '{}': {e}", argv[0]))
        }
    }
}

/// Split a command line into words (single/double quotes and backslash escapes,
/// no expansion of any kind) and put `file` in place of `{file}` — or after the
/// last word when the command does not mention it.
pub fn command_argv(command: &str, file: &Path) -> Result<Vec<String>, String> {
    let mut words = split_words(command)?;
    if words.is_empty() {
        return Err("the editor command is empty".into());
    }
    let file = file.to_string_lossy();
    let mut placed = false;
    for word in words.iter_mut().skip(1) {
        if word.contains("{file}") {
            *word = word.replace("{file}", &file);
            placed = true;
        }
    }
    if !placed {
        words.push(file.into_owned());
    }
    Ok(words)
}

fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("unterminated ' in the editor command".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c) => word.push(c),
                            None => return Err("dangling \\ in the editor command".into()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("unterminated \" in the editor command".into()),
                    }
                }
            }
            // Backslash escapes only outside Windows, where it is the path separator.
            '\\' if !cfg!(windows) => {
                in_word = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => return Err("dangling \\ in the editor command".into()),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// Resolve a bare program name against `PATH` plus the usual install folders. A
/// desktop app launched from the Dock or Finder inherits a minimal `PATH`, so
/// `code` from `/usr/local/bin` or Homebrew would otherwise not be found.
fn resolve_program(program: &str) -> PathBuf {
    if program.contains(['/', '\\']) {
        return PathBuf::from(program);
    }
    search_dirs()
        .into_iter()
        .flat_map(|dir| executable_names(program).map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(program))
}

fn executable_names(program: &str) -> impl Iterator<Item = String> + '_ {
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    exts.iter().map(move |ext| format!("{program}{ext}"))
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if !cfg!(windows) {
        for extra in [
            "/usr/local/bin",
            "/opt/homebrew/bin",
            "/usr/bin",
            "/snap/bin",
        ] {
            dirs.push(PathBuf::from(extra));
        }
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".local/bin"));
        }
    }
    dirs
}

/// Start `cmd` without tying it to the app: no inherited stdio, and a thread
/// reaps it so a long-running editor (`code --wait`) never lingers as a zombie.
fn spawn_detached(cmd: &mut Command) -> std::io::Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Editors installed on this machine, in a stable, friendly order.
pub fn detect() -> Vec<EditorAppDto> {
    let mut found = Vec::new();
    for (name, candidates) in known_editors() {
        if let Some(path) = candidates.into_iter().find(|p| p.exists()) {
            found.push(EditorAppDto {
                name: name.to_string(),
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    found
}

#[cfg(target_os = "macos")]
fn known_editors() -> Vec<(&'static str, Vec<PathBuf>)> {
    let home_apps = dirs::home_dir().map(|h| h.join("Applications"));
    let bundles = [
        ("Visual Studio Code", "Visual Studio Code.app"),
        ("Cursor", "Cursor.app"),
        ("Windsurf", "Windsurf.app"),
        ("Zed", "Zed.app"),
        ("Sublime Text", "Sublime Text.app"),
        ("Nova", "Nova.app"),
        ("BBEdit", "BBEdit.app"),
        ("CotEditor", "CotEditor.app"),
        ("TextMate", "TextMate.app"),
        ("VSCodium", "VSCodium.app"),
    ];
    let mut out: Vec<(&'static str, Vec<PathBuf>)> = bundles
        .into_iter()
        .map(|(name, bundle)| {
            let mut paths = vec![Path::new("/Applications").join(bundle)];
            if let Some(home) = &home_apps {
                paths.push(home.join(bundle));
            }
            (name, paths)
        })
        .collect();
    out.push((
        "TextEdit",
        vec![PathBuf::from("/System/Applications/TextEdit.app")],
    ));
    out
}

#[cfg(target_os = "windows")]
fn known_editors() -> Vec<(&'static str, Vec<PathBuf>)> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let local = env("LOCALAPPDATA");
    let pf = env("ProgramFiles");
    let pf86 = env("ProgramFiles(x86)");
    let under = |base: &Option<PathBuf>, rel: &str| base.as_ref().map(|b| b.join(rel));
    vec![
        (
            "Visual Studio Code",
            [
                under(&local, r"Programs\Microsoft VS Code\Code.exe"),
                under(&pf, r"Microsoft VS Code\Code.exe"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "Cursor",
            [under(&local, r"Programs\cursor\Cursor.exe")]
                .into_iter()
                .flatten()
                .collect(),
        ),
        (
            "Zed",
            [under(&local, r"Programs\Zed\Zed.exe")]
                .into_iter()
                .flatten()
                .collect(),
        ),
        (
            "Sublime Text",
            [under(&pf, r"Sublime Text\sublime_text.exe")]
                .into_iter()
                .flatten()
                .collect(),
        ),
        (
            "Notepad++",
            [
                under(&pf, r"Notepad++\notepad++.exe"),
                under(&pf86, r"Notepad++\notepad++.exe"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "Notepad",
            [env("SystemRoot").map(|r| r.join("notepad.exe"))]
                .into_iter()
                .flatten()
                .collect(),
        ),
    ]
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn known_editors() -> Vec<(&'static str, Vec<PathBuf>)> {
    let bins = [
        ("Visual Studio Code", "code"),
        ("VSCodium", "codium"),
        ("Cursor", "cursor"),
        ("Zed", "zed"),
        ("Sublime Text", "subl"),
        ("Kate", "kate"),
        ("GNOME Text Editor", "gnome-text-editor"),
        ("gedit", "gedit"),
        ("Mousepad", "mousepad"),
        ("Xed", "xed"),
        ("Pluma", "pluma"),
    ];
    let dirs = search_dirs();
    bins.into_iter()
        .map(|(name, bin)| (name, dirs.iter().map(|d| d.join(bin)).collect()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_replaces_the_placeholder_or_is_appended() {
        let file = Path::new("/tmp/my notes.txt");
        assert_eq!(
            command_argv("code --wait {file}", file).unwrap(),
            ["code", "--wait", "/tmp/my notes.txt"]
        );
        assert_eq!(
            command_argv("subl", file).unwrap(),
            ["subl", "/tmp/my notes.txt"]
        );
        assert_eq!(
            command_argv("emacsclient -n +1 --file={file}", file).unwrap(),
            ["emacsclient", "-n", "+1", "--file=/tmp/my notes.txt"]
        );
    }

    #[test]
    fn quoting_keeps_spaces_and_never_expands() {
        let file = Path::new("/f");
        assert_eq!(
            command_argv(r#""/Applications/My Editor/bin/ed" '$HOME' {file}"#, file).unwrap(),
            ["/Applications/My Editor/bin/ed", "$HOME", "/f"]
        );
        assert!(command_argv("   ", file).is_err());
        assert!(command_argv("code 'oops", file).is_err());
    }

    #[test]
    fn a_file_name_is_never_split_or_interpreted() {
        // The path is substituted after splitting, so shell syntax in it stays inert.
        let file = Path::new("/tmp/a; rm -rf ~ $(x).txt");
        assert_eq!(
            command_argv("code {file}", file).unwrap(),
            ["code", "/tmp/a; rm -rf ~ $(x).txt"]
        );
    }
}
