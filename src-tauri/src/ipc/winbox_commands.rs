//! WinBox 4 sessions: MikroTik's own management app, started as a program of
//! its own with the session's address and login. Desktop only.
//!
//! WinBox takes `WinBox <address> [<login> [<password>]]`, the address being
//! an IPv4 or MAC address, a name, or `[IPv6]`, with `:port` when it is not
//! 8291. It reads the password nowhere else (no stdin, file or environment
//! variable), so the session says how to hand it over: as an argument (it then
//! shows in the process list while WinBox runs), on the clipboard for a short
//! while, or not at all.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::ipc::editor_detect::{find_in, is_program};
use crate::state::AppState;

/// WinBox's own port, left out of the address.
pub const DEFAULT_PORT: u16 = 8291;

/// How long a password copied for WinBox stays on the clipboard.
const CLIPBOARD_FOR: Duration = Duration::from_secs(45);

/// How the password reaches WinBox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PasswordMode {
    /// On WinBox's command line: logs in by itself.
    Argument,
    /// Copied to the clipboard, cleared after a short while; pasted by hand.
    Clipboard,
    /// Not passed: typed in WinBox.
    #[default]
    None,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinboxLaunchParams {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// A password manager entry used instead of `password`.
    #[serde(default)]
    pub password_ref: Option<crate::pwmanager::SecretRef>,
    #[serde(default)]
    pub password_mode: PasswordMode,
    /// Settings' WinBox path; empty: look for it.
    #[serde(default)]
    pub program: Option<String>,
}

/// What a launch did, for the toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinboxLaunched {
    pub program: String,
    /// The password was copied to the clipboard.
    pub copied: bool,
}

/// The address as WinBox reads it: IPv6 in brackets, the port only when it
/// is not WinBox's own. A MAC address takes no port.
pub fn address(host: &str, port: u16) -> String {
    let host = host.trim();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let is_mac = is_mac(bare);
    let is_v6 = !is_mac && bare.contains(':');
    let host = if is_v6 { format!("[{bare}]") } else { bare.to_string() };
    if is_mac || port == 0 || port == DEFAULT_PORT {
        host
    } else {
        format!("{host}:{port}")
    }
}

fn is_mac(s: &str) -> bool {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The arguments after the program.
pub fn arguments(address: &str, username: &str, password: Option<&str>) -> Vec<String> {
    let mut args = vec![address.to_string()];
    if !username.is_empty() || password.is_some() {
        args.push(username.to_string());
    }
    if let Some(p) = password {
        args.push(p.to_string());
    }
    args
}

/// Where WinBox is usually put, besides PATH: it ships as a bare program to
/// unpack anywhere (Windows, Linux) or an app bundle (macOS).
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home = dirs::home_dir();
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            dirs.push(local.join("Programs").join("WinBox"));
            dirs.push(local.join("WinBox"));
            // winget's portable apps
            dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var).map(PathBuf::from) {
                dirs.push(pf.join("WinBox"));
                dirs.push(pf.join("MikroTik").join("WinBox"));
                dirs.push(pf.join("Mikrotik").join("Winbox"));
            }
        }
        if let Some(h) = &home {
            dirs.push(h.join("scoop").join("shims"));
            dirs.push(h.join("Desktop"));
            dirs.push(h.join("Downloads"));
            dirs.push(h.join("Downloads").join("WinBox_Windows"));
        }
    } else {
        for d in ["/usr/local/bin", "/usr/bin", "/opt/homebrew/bin", "/home/linuxbrew/.linuxbrew/bin", "/opt/winbox", "/opt/WinBox", "/opt/mikrotik/winbox", "/snap/bin", "/var/lib/flatpak/exports/bin"] {
            dirs.push(d.into());
        }
        if let Some(h) = &home {
            for d in [".local/bin", "bin", "WinBox", "winbox", "Applications/WinBox", ".local/share/WinBox", ".local/share/flatpak/exports/bin", "Downloads/WinBox_Linux"] {
                dirs.push(h.join(d));
            }
        }
    }
    let mut seen = Vec::new();
    dirs.retain(|d| {
        let new = !seen.contains(d);
        seen.push(d.clone());
        new
    });
    dirs
}

/// The program names WinBox goes by: the download's, a package's, Flathub's.
const NAMES: &[&str] = &["WinBox", "winbox", "winbox4", "WinBox4", "winbox64", "com.mikrotik.WinBox"];

/// WinBox found in `dirs`, or on macOS among the applications.
pub fn find_in_dirs(dirs: &[PathBuf]) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut apps = vec![PathBuf::from("/Applications/WinBox.app")];
        if let Some(h) = dirs::home_dir() {
            apps.push(h.join("Applications/WinBox.app"));
        }
        if let Some(app) = apps.into_iter().find(|a| a.is_dir()) {
            return Some(app);
        }
    }
    // Case-insensitive file systems would find "WinBox" twice; it does not matter.
    NAMES.iter().find_map(|n| find_in(dirs, n))
}

/// The program to start: Settings' path when it is one, else WinBox found.
fn program(custom: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = custom.map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(p);
        let usable = is_program(&path) || (cfg!(target_os = "macos") && is_app(&path));
        return if usable {
            Ok(path)
        } else {
            Err(format!("WinBox was not found at {p}. Check the path in Settings > Sessions."))
        };
    }
    find_in_dirs(&search_dirs()).ok_or_else(|| {
        "WinBox 4 was not found. Install it from mikrotik.com/download/winbox, or set its path in Settings > Sessions.".to_string()
    })
}

fn is_app(p: &Path) -> bool {
    p.is_dir() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))
}

/// The command that starts `program` with `args`. An app bundle (macOS)
/// starts through `open`, in a new instance so a second router gets its
/// own window.
pub fn command(program: &Path, args: &[String]) -> Command {
    let mut cmd = if is_app(program) {
        let mut c = Command::new("/usr/bin/open");
        c.arg("-n").arg("-a").arg(program).arg("--args");
        c
    } else {
        let mut c = Command::new(program);
        if let Some(dir) = program.parent().filter(|d| !d.as_os_str().is_empty()) {
            // WinBox looks for its assets next to itself.
            c.current_dir(dir);
        }
        c
    };
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd
}

/// Put `text` on the clipboard and take it off again after a while, unless
/// something else was copied since.
fn copy_for_a_while(text: String) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut clip = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                return;
            }
        };
        if let Err(e) = clip.set_text(text.clone()) {
            let _ = tx.send(Err(e.to_string()));
            return;
        }
        let _ = tx.send(Ok(()));
        // Kept open meanwhile: on Linux the text goes when its owner does.
        std::thread::sleep(CLIPBOARD_FOR);
        if clip.get_text().is_ok_and(|now| now == text) {
            let _ = clip.clear();
        }
    });
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "The clipboard did not answer.".to_string())?
        .map_err(|e| format!("Could not copy the password: {e}"))
}

/// Where WinBox is, for Settings: Settings' path if it is set and usable,
/// else the one found here; None when there is none.
#[tauri::command]
pub async fn winbox_find(program: Option<String>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || self::program(program.as_deref()).ok().map(|p| p.to_string_lossy().into_owned()))
        .await
        .map_err(|e| e.to_string())
}

/// Start WinBox on a saved session.
#[tauri::command]
pub async fn winbox_launch(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    params: WinboxLaunchParams,
    session_id: Option<String>,
) -> Result<WinboxLaunched, String> {
    let mut params = params;
    let wants_password = params.password_mode != PasswordMode::None;
    // A password held by a password manager, fetched now, and only when it
    // is passed on (see `telnet_connect`).
    if params.password_ref.take().is_some() && wants_password {
        let (stored, _) = crate::ipc::session_commands::stored_session_for_desktop(
            &state,
            session_id.as_deref(),
            crate::state::SessionKind::Winbox,
            &params.host,
            params.port,
            None,
            None,
        )
        .await?;
        let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
        let entry = crate::ipc::session_commands::desktop_ref(&stored)?;
        params.password = Some(
            crate::pwmanager::with_prompt(&app_handle, reason, crate::pwmanager::resolve_to_string(&state, entry))
                .await
                .inspect_err(|e| tracing::warn!("WinBox: {e}"))?,
        );
    }
    let password = params.password.filter(|p| !p.is_empty() && wants_password);
    let program = {
        let custom = params.program.clone();
        tauri::async_runtime::spawn_blocking(move || self::program(custom.as_deref()))
            .await
            .map_err(|e| e.to_string())??
    };
    let address = address(&params.host, params.port);
    let username = params.username.unwrap_or_default();
    let on_command_line = if params.password_mode == PasswordMode::Argument { password.as_deref() } else { None };
    let args = arguments(&address, username.trim(), on_command_line);
    // Never the arguments: they may hold the password.
    tracing::info!("WinBox: starting {} for {address}", program.display());
    let mut child = command(&program, &args)
        .spawn()
        .map_err(|e| format!("Could not start WinBox ({}): {e}", program.display()))?;
    // Reaped when it ends, so no zombie stays behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let copied = match (params.password_mode, password) {
        (PasswordMode::Clipboard, Some(p)) => {
            copy_for_a_while(p)?;
            true
        }
        _ => false,
    };
    Ok(WinboxLaunched { program: program.to_string_lossy().into_owned(), copied })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(address("192.0.2.1", 8291), "192.0.2.1");
        assert_eq!(address("192.0.2.1", 0), "192.0.2.1");
        assert_eq!(address(" router.example.com ", 8299), "router.example.com:8299");
        assert_eq!(address("2001:db8::1", 8291), "[2001:db8::1]");
        assert_eq!(address("[2001:db8::1]", 9000), "[2001:db8::1]:9000");
        assert_eq!(address("fe80::1%2", 8291), "[fe80::1%2]");
        assert_eq!(address("00:0C:42:AA:BB:CC", 8291), "00:0C:42:AA:BB:CC");
        assert_eq!(address("00-0c-42-aa-bb-cc", 9000), "00-0c-42-aa-bb-cc", "a MAC address takes no port");
    }

    #[test]
    fn argument_lists() {
        assert_eq!(arguments("192.0.2.1", "", None), vec!["192.0.2.1"]);
        assert_eq!(arguments("192.0.2.1", "admin", None), vec!["192.0.2.1", "admin"]);
        assert_eq!(arguments("192.0.2.1", "admin", Some("pw")), vec!["192.0.2.1", "admin", "pw"]);
        assert_eq!(arguments("192.0.2.1", "", Some("pw")), vec!["192.0.2.1", "", "pw"], "the password stays third");
    }

    #[test]
    fn password_mode_reads_lowercase() {
        let p: WinboxLaunchParams =
            serde_json::from_str(r#"{"host":"192.0.2.1","port":8291,"passwordMode":"clipboard"}"#).unwrap();
        assert_eq!(p.password_mode, PasswordMode::Clipboard);
        let p: WinboxLaunchParams = serde_json::from_str(r#"{"host":"192.0.2.1","port":8291}"#).unwrap();
        assert_eq!(p.password_mode, PasswordMode::None);
    }

    #[cfg(unix)]
    #[test]
    fn finds_winbox_by_its_names() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        assert!(NAMES.iter().all(|n| find_in(&[tmp.path().to_path_buf()], n).is_none()));
        let p = tmp.path().join("winbox4");
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        #[cfg(not(target_os = "macos"))]
        assert_eq!(find_in_dirs(&[tmp.path().to_path_buf()]), Some(p.clone()));
        assert_eq!(program(Some(p.to_str().unwrap())).unwrap(), p);
        assert!(program(Some("/nonexistent/WinBox")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn starts_next_to_its_assets() {
        let cmd = command(Path::new("/opt/winbox/WinBox"), &["192.0.2.1".into()]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/opt/winbox")));
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), vec!["192.0.2.1"]);
    }
}
