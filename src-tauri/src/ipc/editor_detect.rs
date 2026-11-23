//! Text editors installed on this computer, for Settings' choice of the
//! program the full-pane file manager's Edit (F4) opens files in.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// An editor that was found, and how to start it on a file: `program`,
/// then `args`, then the file. A terminal editor runs in a terminal tab.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditorInfo {
    pub id: String,
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub terminal: bool,
}

/// id, name, program names to look for in PATH, arguments, terminal.
type Known = (&'static str, &'static str, &'static [&'static str], &'static [&'static str], bool);

const KNOWN: &[Known] = &[
    ("codium", "VSCodium", &["codium", "com.vscodium.codium"], &[], false),
    ("code", "Visual Studio Code", &["code", "com.visualstudio.code"], &[], false),
    ("subl", "Sublime Text", &["subl", "com.sublimetext.three"], &[], false),
    ("zed", "Zed", &["zed", "zeditor", "dev.zed.Zed"], &[], false),
    ("kate", "Kate", &["kate", "org.kde.kate"], &[], false),
    ("kwrite", "KWrite", &["kwrite"], &[], false),
    ("gnome-text-editor", "GNOME Text Editor", &["gnome-text-editor", "org.gnome.TextEditor"], &[], false),
    ("gedit", "gedit", &["gedit", "org.gnome.gedit"], &[], false),
    ("mousepad", "Mousepad", &["mousepad"], &[], false),
    ("pluma", "Pluma", &["pluma"], &[], false),
    ("xed", "Xed", &["xed"], &[], false),
    ("notepad++", "Notepad++", &["notepad++"], &[], false),
    ("emacs", "Emacs", &["emacs"], &[], false),
    ("emacs-nw", "Emacs (terminal)", &["emacs"], &["-nw"], true),
    ("nvim", "Neovim", &["nvim"], &[], true),
    ("vim", "Vim", &["vim"], &[], true),
    ("vi", "vi", &["vi"], &[], true),
    ("nano", "nano", &["nano"], &[], true),
    ("micro", "micro", &["micro"], &[], true),
    ("hx", "Helix", &["hx", "helix"], &[], true),
    ("mcedit", "mcedit", &["mcedit"], &[], true),
];

/// Where to look: PATH, plus the usual places a program started from the
/// desktop (with a short PATH) would miss.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut extra: Vec<PathBuf> = vec![
        "/usr/local/bin".into(),
        "/usr/bin".into(),
        "/bin".into(),
        "/opt/homebrew/bin".into(),
        "/snap/bin".into(),
        "/var/lib/flatpak/exports/bin".into(),
    ];
    if let Some(home) = dirs::home_dir() {
        extra.push(home.join(".local/bin"));
        extra.push(home.join(".local/share/flatpak/exports/bin"));
    }
    #[cfg(windows)]
    {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var) {
                extra.push(PathBuf::from(&pf).join("Notepad++"));
                extra.push(PathBuf::from(&pf).join("Microsoft VS Code").join("bin"));
                extra.push(PathBuf::from(&pf).join("VSCodium").join("bin"));
                extra.push(PathBuf::from(&pf).join("Sublime Text"));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            extra.push(PathBuf::from(&local).join("Programs").join("Microsoft VS Code").join("bin"));
            extra.push(PathBuf::from(&local).join("Programs").join("VSCodium").join("bin"));
        }
    }
    for d in extra {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

pub(crate) fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"].iter().map(|ext| format!("{name}.{ext}")).collect()
    } else {
        vec![name.to_string()]
    };
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| is_program(p))
}

#[cfg(unix)]
pub(crate) fn is_program(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
pub(crate) fn is_program(p: &Path) -> bool {
    p.is_file()
}

pub(crate) fn detect_in(dirs: &[PathBuf]) -> Vec<EditorInfo> {
    let mut found = Vec::new();
    for (id, name, programs, args, terminal) in KNOWN {
        if let Some(path) = programs.iter().find_map(|p| find_in(dirs, p)) {
            found.push(EditorInfo {
                id: id.to_string(),
                name: name.to_string(),
                program: path.to_string_lossy().into_owned(),
                args: args.iter().map(|a| a.to_string()).collect(),
                terminal: *terminal,
            });
        }
    }
    #[cfg(target_os = "macos")]
    for (id, name, app) in [
        ("codium", "VSCodium", "VSCodium"),
        ("code", "Visual Studio Code", "Visual Studio Code"),
        ("subl", "Sublime Text", "Sublime Text"),
        ("zed", "Zed", "Zed"),
        ("bbedit", "BBEdit", "BBEdit"),
        ("coteditor", "CotEditor", "CotEditor"),
    ] {
        let installed = Path::new("/Applications").join(format!("{app}.app")).exists()
            || dirs::home_dir().is_some_and(|h| h.join("Applications").join(format!("{app}.app")).exists());
        if installed && !found.iter().any(|e: &EditorInfo| e.id == id) {
            found.push(EditorInfo {
                id: id.to_string(),
                name: name.to_string(),
                program: "/usr/bin/open".to_string(),
                args: vec!["-a".to_string(), app.to_string()],
                terminal: false,
            });
        }
    }
    found
}

/// The editors installed here, in a fixed order (graphical ones first).
#[tauri::command]
pub async fn local_detect_editors() -> Result<Vec<EditorInfo>, String> {
    tokio::task::spawn_blocking(|| detect_in(&search_dirs())).await.map_err(|e| e.to_string())
}

/// Start a graphical editor on a local file and leave it running.
#[tauri::command]
pub async fn local_open_with(program: String, args: Vec<String>, path: String) -> Result<(), String> {
    if !Path::new(&path).is_file() {
        return Err(format!("{path}: not a file"));
    }
    std::process::Command::new(&program)
        .args(&args)
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{program}: {e}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn finds_executables_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        for (name, mode) in [("vim", 0o755), ("codium", 0o755), ("nano", 0o644)] {
            let p = tmp.path().join(name);
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let found = detect_in(&[tmp.path().to_path_buf()]);
        let ids: Vec<_> = found.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.starts_with(&["codium"]), "{ids:?}");
        assert!(ids.contains(&"vim"));
        assert!(!ids.contains(&"nano"), "a file that cannot run is not an editor");
        let vim = found.iter().find(|e| e.id == "vim").unwrap();
        assert!(vim.terminal);
    }
}
