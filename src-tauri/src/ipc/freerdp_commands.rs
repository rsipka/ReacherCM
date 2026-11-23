//! Launch the locally installed FreeRDP 3 client in its own window.

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use tauri::ipc::Channel;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use zeroize::Zeroizing;

use crate::state::{AppState, SessionKind};

use super::editor_detect::is_program;
use super::session_commands::{desktop_ref, stored_session_for_desktop};

fn executable(path: &Path) -> bool {
    // Windows batch files are interpreted by cmd.exe. Launch native clients only.
    path.is_absolute()
        && (!cfg!(windows)
            || path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")))
        && is_program(path)
}

fn find_program(custom: Option<&str>) -> Option<PathBuf> {
    if let Some(custom) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(custom);
        return executable(&path).then_some(path);
    }

    let mut dirs: Vec<_> = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .collect();
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            dirs.push(local.join("Microsoft/WinGet/Links"));
        }
        dirs.extend(
            ["ProgramFiles", "ProgramFiles(x86)"]
                .into_iter()
                .filter_map(std::env::var_os)
                .map(|dir| PathBuf::from(dir).join("FreeRDP")),
        );
    } else {
        dirs.extend(
            [
                "/usr/local/bin",
                "/usr/bin",
                "/opt/homebrew/bin",
                "/opt/local/bin",
            ]
            .map(PathBuf::from),
        );
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".local/bin"));
        }
    }

    // Prefer the SDL client, which can show certificate prompts in its GUI.
    let sdl = [
        "sdl-freerdp3",
        "sdl-freerdp",
        "sdl3-freerdp",
        "sdl2-freerdp",
    ];
    let native: &[&str] = if cfg!(windows) {
        &["wfreerdp"]
    } else {
        &["xfreerdp3", "xfreerdp", "wlfreerdp3", "wlfreerdp"]
    };
    sdl.into_iter()
        .chain(native.iter().copied())
        .find_map(|name| {
            let file_name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
            dirs.iter()
                .map(|dir| dir.join(&file_name))
                .find(|path| executable(path))
        })
}

#[tauri::command]
pub fn freerdp_find(program: Option<String>) -> Option<String> {
    find_program(program.as_deref()).map(|path| path.to_string_lossy().into_owned())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreerdpLaunchParams {
    host: String,
    port: u16,
    username: String,
    domain: Option<String>,
    password: Option<String>,
    use_password_ref: bool,
    clipboard: Option<String>,
    graphics_pipeline: bool,
    share_path: Option<String>,
    program: Option<String>,
    /// The session's own RDP settings; the ones FreeRDP has a switch for are passed on.
    #[serde(default)]
    rdp: Option<crate::rdp::options::RdpOptions>,
}

impl FreerdpLaunchParams {
    fn input(
        &self,
        password: &str,
        certificate: Option<&str>,
    ) -> Result<Zeroizing<String>, String> {
        let host = self
            .host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']');
        if host.is_empty() || self.port == 0 {
            return Err("FreeRDP requires a host and port.".into());
        }
        let address = if host.contains(':') {
            format!("[{host}]:{}", self.port)
        } else {
            format!("{host}:{}", self.port)
        };

        // FreeRDP accepts one argument per line. Reject line and NUL injection
        // even for inputs from imported and shared sessions.
        let fields = [
            host,
            &self.username,
            self.domain.as_deref().unwrap_or(""),
            password,
            self.share_path.as_deref().unwrap_or(""),
            self.rdp.as_ref().and_then(|o| o.program.as_deref()).unwrap_or(""),
            self.rdp.as_ref().and_then(|o| o.work_dir.as_deref()).unwrap_or(""),
        ];
        if fields.iter().any(|s| s.contains(['\r', '\n', '\0'])) {
            return Err(
                "FreeRDP cannot pass connection fields containing a line break or NUL.".into(),
            );
        }
        let options = self.rdp.clone().unwrap_or_default();
        let size = match options.resolution.as_deref().and_then(crate::rdp::options::parse_resolution) {
            Some((w, h)) => format!("/size:{w}x{h}\n/smart-sizing\n"),
            None => "/dynamic-resolution\n".to_owned(),
        };
        let mut input = Zeroizing::new(format!("/v:{address}\n/u:{}\n{size}", self.username));
        input.push_str(&freerdp_option_args(&options));
        if let Some(domain) = self.domain.as_deref().filter(|s| !s.is_empty()) {
            input.push_str("/d:");
            input.push_str(domain);
            input.push('\n');
        }
        if self.graphics_pipeline {
            input.push_str("/gfx\n");
        }
        if let Some(certificate) = certificate {
            input.push_str(certificate);
            input.push('\n');
        }
        input.push_str(match self.clipboard.as_deref() {
            Some("off") => "-clipboard\n",
            Some("text") => "/clipboard:files-to:off\n",
            _ => "+clipboard\n",
        });
        if let Some(path) = self.share_path.as_deref() {
            if path.contains(',') || !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
                return Err("FreeRDP drive sharing requires an existing absolute folder path without commas.".into());
            }
            input.push_str("/drive:Reacher,");
            input.push_str(path);
            input.push('\n');
        }

        input.reserve("/p:".len() + password.len() + 1);
        input.push_str("/p:");
        input.push_str(password);
        input.push('\n');
        Ok(input)
    }
}

/// FreeRDP switches for the session's own settings, one per line. Only those
/// FreeRDP has; keep-alive, the jiggler and sound quality are IronRDP's alone.
fn freerdp_option_args(o: &crate::rdp::options::RdpOptions) -> String {
    let mut args = String::new();
    let mut line = |arg: &str| {
        args.push_str(arg);
        args.push('\n');
    };
    if let Some(scale) = o.scale.filter(|s| (100..=500).contains(s)) {
        line(&format!("/scale-desktop:{scale}"));
    }
    if let Some(depth) = o.color_depth.filter(|d| *d == 16 || *d == 32) {
        line(&format!("/bpp:{depth}"));
    }
    if let Some(layout) = o.keyboard_layout.filter(|l| *l != 0) {
        line(&format!("/kbd:layout:0x{layout:08X}"));
    }
    if o.sound.as_deref() == Some("off") {
        line("/audio-mode:2");
    }
    match o.performance.as_deref() {
        Some("fast") => ["-wallpaper", "-themes", "-menu-anims", "-window-drag", "-fonts"].into_iter().for_each(&mut line),
        Some("best") => ["+wallpaper", "+themes", "+menu-anims", "+window-drag", "+fonts", "+aero"].into_iter().for_each(&mut line),
        _ => {}
    }
    if o.compression == Some(false) {
        line("-compression");
    }
    if o.admin == Some(true) {
        line("/admin");
    }
    if let Some(program) = o.program.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        line(&format!("/shell:{program}"));
        if let Some(dir) = o.work_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            line(&format!("/shell-dir:{dir}"));
        }
    }
    args
}

/// xfreerdp prompts on stdin, which the launcher closes after writing the
/// arguments. Let it validate trusted certificates without a console prompt.
/// An RDP certificate the user approved in IronRDP can also be used here.
fn x11_certificate_arg(saved: Option<&str>) -> String {
    let mut arg = "/cert:deny".to_owned();
    if let Some(digest) = saved
        .and_then(|saved| saved.strip_prefix("SHA256:"))
        .and_then(|digest| {
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(digest)
                .ok()
        })
        .filter(|digest| digest.len() == 32)
    {
        arg.push_str(",fingerprint:sha256:");
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        arg.push_str(&hex);
    }
    arg
}

fn check_exit(status: std::io::Result<ExitStatus>, x11_client: bool) -> Result<(), String> {
    let status = status.map_err(|err| format!("Could not wait for FreeRDP: {err}"))?;
    // xfreerdp returns RDP disconnect and logoff reasons as exit codes.
    if status.success() || (x11_client && matches!(status.code(), Some(1 | 2 | 11 | 12))) {
        return Ok(());
    }
    tracing::warn!(exit_code = ?status.code(), x11_client, "FreeRDP session failed");
    // XF_EXIT_CONN_TLS_CONNECT_FAILED in the X11 client.
    if x11_client && status.code() == Some(143) {
        return Err("FreeRDP could not establish TLS. If the server uses a self-signed certificate, verify and save it in an IronRDP session, or select the FreeRDP SDL executable for graphical certificate prompts.".into());
    }
    Err(format!("FreeRDP exited with {status}. Check the server and certificate. For certificate prompts in a window, select the SDL client in Settings > Remote desktop."))
}

#[tauri::command]
pub async fn freerdp_launch(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    mut params: FreerdpLaunchParams,
    session_id: String,
    on_failure: Channel<String>,
) -> Result<(), String> {
    let program = find_program(params.program.as_deref()).ok_or_else(|| {
        "FreeRDP was not found. Install a FreeRDP 3 desktop client or set its executable in Settings > Remote desktop.".to_string()
    })?;

    let password = Zeroizing::new(if params.use_password_ref {
        let (stored, _) = stored_session_for_desktop(
            &state,
            Some(&session_id),
            SessionKind::Rdp,
            &params.host,
            params.port,
            Some(&params.username),
            None,
        )
        .await?;
        if stored.domain != params.domain {
            return Err("The domain changed. Reload the session list and connect again.".into());
        }
        let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
        crate::pwmanager::with_prompt(
            &app_handle,
            reason,
            crate::pwmanager::resolve_to_string(&state, desktop_ref(&stored)?),
        )
        .await?
    } else {
        params
            .password
            .take()
            .ok_or_else(|| "FreeRDP requires a password.".to_string())?
    });
    let x11_client = program
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "xfreerdp" | "xfreerdp3"));
    let saved_fingerprint = x11_client
        .then(|| {
            crate::ssh::client::on_device_fingerprint(&format!(
                "rdp:{}:{}",
                params.host, params.port
            ))
        })
        .flatten();
    let certificate = x11_client.then(|| x11_certificate_arg(saved_fingerprint.as_deref()));
    let input = params.input(&password, certificate.as_deref())?;
    drop(password);

    tracing::info!(
        x11_client,
        saved_certificate = saved_fingerprint.is_some(),
        "Starting FreeRDP"
    );
    // Use an anonymous pipe. The OS process arguments contain only the flag
    // requesting stdin, and FreeRDP's output can include connection arguments.
    let mut child = Command::new(program)
        .arg("/args-from:stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            tracing::warn!(os_error = ?err.raw_os_error(), "Could not start FreeRDP");
            format!("Could not start FreeRDP: {err}")
        })?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or("Could not open the FreeRDP input pipe.")?;
    let written =
        tokio::time::timeout(Duration::from_secs(5), stdin.write_all(input.as_bytes())).await;
    drop(stdin); // The client waits for EOF before parsing its arguments.
    drop(input);
    if !matches!(written, Ok(Ok(()))) {
        let _ = child.kill().await;
        tracing::warn!("Could not write to the FreeRDP connection pipe");
        return Err("Could not pass connection details to FreeRDP. Select a client that supports /args-from:stdin.".into());
    }

    // Report startup failures to the caller. Reap longer-lived sessions and
    // report any later failures without holding up the connection dialog.
    match tokio::time::timeout(Duration::from_millis(300), child.wait()).await {
        Ok(status) => check_exit(status, x11_client),
        Err(_) => {
            tauri::async_runtime::spawn(async move {
                if let Err(error) = check_exit(child.wait().await, x11_client) {
                    let _ = on_failure.send(error);
                }
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> FreerdpLaunchParams {
        serde_json::from_value(serde_json::json!({
            "host": "::1",
            "port": 3390,
            "username": "Example User",
            "domain": "EXAMPLE",
            "usePasswordRef": false,
            "clipboard": "text",
            "graphicsPipeline": true
        }))
        .unwrap()
    }

    #[test]
    fn prepares_one_argument_per_line_with_ipv6_and_clipboard_policy() {
        let input = params().input("example secret", None).unwrap();
        assert_eq!(
            input.lines().collect::<Vec<_>>(),
            [
                "/v:[::1]:3390",
                "/u:Example User",
                "/dynamic-resolution",
                "/d:EXAMPLE",
                "/gfx",
                "/clipboard:files-to:off",
                "/p:example secret",
            ],
        );
    }

    #[test]
    fn passes_session_rdp_settings() {
        let mut session = params();
        session.graphics_pipeline = false;
        session.domain = None;
        session.rdp = Some(crate::rdp::options::RdpOptions {
            resolution: Some("1280x720".into()),
            color_depth: Some(16),
            keyboard_layout: Some(0x0405),
            performance: Some("fast".into()),
            admin: Some(true),
            program: Some("notepad.exe".into()),
            ..Default::default()
        });
        let input = session.input("example", None).unwrap();
        let lines: Vec<_> = input.lines().collect();
        assert_eq!(&lines[2..4], ["/size:1280x720", "/smart-sizing"]);
        for arg in ["/bpp:16", "/kbd:layout:0x00000405", "-wallpaper", "/admin", "/shell:notepad.exe"] {
            assert!(lines.contains(&arg), "{arg} missing");
        }
        assert!(!lines.contains(&"/dynamic-resolution"));

        session.rdp.as_mut().unwrap().program = Some("x\n/cert:ignore".into());
        assert!(session.input("example", None).is_err());
    }

    #[test]
    fn rejects_line_breaks_and_nul_in_connection_fields() {
        assert!(params().input("example\r\n/cert:ignore", None).is_err());
        assert!(params().input("example\0", None).is_err());
        let mut session = params();
        session.username = "user\n+drives".into();
        assert!(session.input("example", None).is_err());
    }

    #[test]
    fn x11_accepts_saved_sha256_certificates_and_disables_console_prompts() {
        assert_eq!(x11_certificate_arg(None), "/cert:deny");
        assert_eq!(x11_certificate_arg(Some("SHA256:YQ")), "/cert:deny");
        let pin = format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode([0xca; 32])
        );
        let policy = x11_certificate_arg(Some(&pin));
        assert_eq!(
            policy,
            format!("/cert:deny,fingerprint:sha256:{}", "ca".repeat(32))
        );
        let input = params().input("dummy", Some(&policy)).unwrap();
        assert!(input.lines().any(|argument| argument == policy));
    }
}
