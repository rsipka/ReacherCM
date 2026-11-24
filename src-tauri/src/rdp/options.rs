//! A saved RDP session's own connection settings, as in the session editor's
//! RDP section. Every field left unset keeps what Reacher CM did before the
//! setting existed, so sessions saved earlier connect exactly as they did.

use ironrdp_pdu::rdp::client_info::PerformanceFlags;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpOptions {
    /// A fixed desktop size, `WIDTHxHEIGHT`. Unset: the size of the panel,
    /// following it as it is resized. Applied by the panel, which asks for
    /// this size and scales the picture to fit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// The remote's display scaling in percent (100 to 500). Unset: the
    /// local screen's own scaling. Applied by the panel at connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    /// 16 or 32 bits per pixel. Unset: 32.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_depth: Option<u32>,
    /// The keyboard layout the session starts with, as a Windows layout id
    /// (0x0405 Czech). Unset: the server's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard_layout: Option<u32>,
    /// `off` for no remote sound. Unset: played here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
    /// `dynamic`, `medium` or `high`. Unset: high.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_quality: Option<String>,
    /// `fast` (no wallpaper, animations, themes or font smoothing) or `best`
    /// (everything, desktop composition too). Unset: a balance of the two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance: Option<String>,
    /// False turns bulk compression off. Unset: on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<bool>,
    /// Seconds between keep-alive input while idle, so firewalls keep the
    /// connection. Unset or 0: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keepalive: Option<u32>,
    /// Seconds between small mouse moves while idle, so the remote does not
    /// lock or go idle. Unset or 0: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jiggler: Option<u32>,
    /// Ask for the administrative (console) session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<bool>,
    /// A program started instead of the desktop (alternate shell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// The folder that program starts in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    /// Run `program` as a RemoteApp: only its windows, no remote desktop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_app: Option<bool>,
    /// RemoteApp only: the program's command line arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_args: Option<String>,
    /// RemoteApp in the built-in client: `separate` puts each of its windows
    /// in a window of its own on this computer. Unset: inside the tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_windows: Option<String>,
    /// The SSH port the session menu's Open file manager uses on the same
    /// host (SFTP). Unset: 22.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_port: Option<u16>,
    /// More local folders shown inside the desktop as drives, beside the
    /// session's shared folder. Desktops only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drives: Option<Vec<String>>,
    /// Every local drive as well: the drive letters on Windows; on macOS
    /// and Linux the home folder, the root and mounted disks. Desktops only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_drives: Option<bool>,
    /// `kerberos` to log in with Kerberos (falling back to NTLM when no KDC
    /// answers). Unset: NTLM, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    /// Kerberos only: the KDC, `dc.example.com[:88]`, or a KDC proxy URL
    /// (`https://gw.example.com/KdcProxy`). Unset: looked up in DNS and the
    /// system's Kerberos settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdc: Option<String>,
}

/// The most extra drives a session keeps.
pub const MAX_DRIVES: usize = 16;

/// The SSH port Open file manager uses when the session names none.
pub const FILES_PORT: u16 = 22;

/// The largest keep-alive or jiggler interval taken, one day.
const MAX_INTERVAL: u32 = 86_400;

impl RdpOptions {
    /// The options as stored: values out of range or not known are dropped,
    /// text is trimmed. None when nothing is left.
    pub fn tidy(self) -> Option<Self> {
        let text = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty() && !v.contains('\0'));
        let one_of = |v: Option<String>, known: &[&str]| v.filter(|v| known.contains(&v.as_str()));
        let interval = |v: Option<u32>| v.filter(|s| (1..=MAX_INTERVAL).contains(s));
        let tidy = Self {
            resolution: text(self.resolution).filter(|r| parse_resolution(r).is_some()),
            scale: self.scale.filter(|s| (100..=500).contains(s)),
            color_depth: self.color_depth.filter(|d| *d == 16 || *d == 32),
            keyboard_layout: self.keyboard_layout.filter(|l| *l != 0),
            sound: one_of(self.sound, &["off"]),
            audio_quality: one_of(self.audio_quality, &["dynamic", "medium", "high"]),
            performance: one_of(self.performance, &["fast", "best"]),
            compression: self.compression.filter(|c| !c),
            keepalive: interval(self.keepalive),
            jiggler: interval(self.jiggler),
            admin: self.admin.filter(|a| *a),
            remote_app: self.remote_app.filter(|r| *r),
            app_args: text(self.app_args),
            app_windows: one_of(self.app_windows, &["separate"]),
            program: text(self.program),
            work_dir: text(self.work_dir),
            files_port: self.files_port.filter(|p| *p != 0 && *p != FILES_PORT),
            drives: self.drives.map(|list| {
                let mut kept: Vec<String> = Vec::new();
                for d in list.into_iter().filter_map(|d| text(Some(d))) {
                    if !kept.contains(&d) && kept.len() < MAX_DRIVES {
                        kept.push(d);
                    }
                }
                kept
            })
            .filter(|l| !l.is_empty()),
            all_drives: self.all_drives.filter(|a| *a),
            kdc: if self.auth.as_deref() == Some("kerberos") { text(self.kdc).filter(|k| kdc_url(k).is_some()) } else { None },
            auth: one_of(self.auth, &["kerberos"]),
        };
        // A RemoteApp, its arguments and the start folder mean nothing without a program.
        let tidy = if tidy.program.is_none() {
            Self { remote_app: None, app_args: None, app_windows: None, work_dir: None, ..tidy }
        } else if tidy.remote_app.is_none() {
            Self { app_args: None, app_windows: None, ..tidy }
        } else {
            tidy
        };
        (tidy != Self::default()).then_some(tidy)
    }

    /// A RemoteApp whose windows each get a window of their own here.
    pub fn separate_windows(&self) -> bool {
        self.is_remote_app() && self.app_windows.as_deref() == Some("separate")
    }

    /// Whether the session opens a RemoteApp rather than a desktop.
    pub fn is_remote_app(&self) -> bool {
        self.remote_app == Some(true) && self.program.as_deref().is_some_and(|p| !p.trim().is_empty())
    }

    /// The SSH port of the session's file manager.
    pub fn files_port(options: Option<&Self>) -> u16 {
        options.and_then(|o| o.files_port).unwrap_or(FILES_PORT)
    }

    pub fn performance_flags(&self) -> Option<PerformanceFlags> {
        match self.performance.as_deref()? {
            "fast" => Some(
                PerformanceFlags::DISABLE_WALLPAPER
                    | PerformanceFlags::DISABLE_FULLWINDOWDRAG
                    | PerformanceFlags::DISABLE_MENUANIMATIONS
                    | PerformanceFlags::DISABLE_THEMING
                    | PerformanceFlags::DISABLE_CURSOR_SHADOW,
            ),
            "best" => Some(PerformanceFlags::ENABLE_FONT_SMOOTHING | PerformanceFlags::ENABLE_DESKTOP_COMPOSITION),
            _ => None,
        }
    }
}

/// The KDC a session names, as the URL Kerberos takes: a proxy URL as it
/// is, a host with or without a port as `tcp://host:port` (88 unless said).
pub fn kdc_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    if text.starts_with("https://") || text.starts_with("http://") || text.starts_with("tcp://") || text.starts_with("udp://") {
        return url::Url::parse(text).ok().filter(|u| u.host_str().is_some()).map(|_| text.to_string());
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h, p.parse::<u16>().ok().filter(|p| *p != 0)?),
        Some(_) => return None,
        None => (text, 88),
    };
    let ok = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then(|| format!("tcp://{host}:{port}"))
}

/// `1920x1080` as a size the server takes (200 to 8192 each way).
pub fn parse_resolution(text: &str) -> Option<(u16, u16)> {
    let (w, h) = text.trim().split_once(['x', 'X', '×'])?;
    let (w, h) = (w.trim().parse::<u16>().ok()?, h.trim().parse::<u16>().ok()?);
    ((200..=8192).contains(&w) && (200..=8192).contains(&h)).then_some((w, h))
}

/// The device scale factor Windows takes beside a desktop scale factor: it
/// knows only 100, 140 and 180.
pub fn device_scale(desktop: u32) -> u32 {
    match desktop {
        0..125 => 100,
        125..175 => 140,
        _ => 180,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_set_is_stored_as_nothing() {
        assert_eq!(RdpOptions::default().tidy(), None);
        let unset = RdpOptions {
            compression: Some(true),
            admin: Some(false),
            keepalive: Some(0),
            program: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(unset.tidy(), None);
    }

    #[test]
    fn values_out_of_range_are_dropped() {
        let o = RdpOptions {
            resolution: Some("100x100".into()),
            scale: Some(50),
            color_depth: Some(24),
            sound: Some("remote".into()),
            performance: Some("turbo".into()),
            jiggler: Some(60),
            ..Default::default()
        }
        .tidy()
        .unwrap();
        assert_eq!(o, RdpOptions { jiggler: Some(60), ..Default::default() });
    }

    #[test]
    fn remote_app_needs_a_program() {
        let o = RdpOptions { remote_app: Some(true), app_args: Some("x".into()), work_dir: Some("C:\\".into()), ..Default::default() };
        assert_eq!(o.tidy(), None);
        let o = RdpOptions { program: Some("notepad".into()), app_args: Some("a.txt".into()), ..Default::default() }.tidy().unwrap();
        assert_eq!(o.app_args, None);
        let o = RdpOptions { program: Some("notepad".into()), remote_app: Some(true), app_args: Some("a.txt".into()), ..Default::default() }.tidy().unwrap();
        assert!(o.is_remote_app());
        assert_eq!(o.app_args.as_deref(), Some("a.txt"));
    }

    #[test]
    fn kdc_forms() {
        assert_eq!(kdc_url("dc.example.com").as_deref(), Some("tcp://dc.example.com:88"));
        assert_eq!(kdc_url("dc.example.com:8888").as_deref(), Some("tcp://dc.example.com:8888"));
        assert_eq!(kdc_url("https://gw.example.com/KdcProxy").as_deref(), Some("https://gw.example.com/KdcProxy"));
        assert_eq!(kdc_url("dc example"), None);
        assert_eq!(kdc_url("dc.example.com:x"), None);
    }

    #[test]
    fn drives_and_kerberos_tidied() {
        let o = RdpOptions {
            drives: Some(vec![" /srv/a ".into(), "/srv/a".into(), "".into()]),
            kdc: Some("dc.example.com".into()),
            ..Default::default()
        }
        .tidy()
        .unwrap();
        assert_eq!(o.drives, Some(vec!["/srv/a".to_string()]));
        // No KDC without Kerberos.
        assert_eq!(o.kdc, None);
        let o = RdpOptions { auth: Some("kerberos".into()), kdc: Some(" dc.example.com ".into()), ..Default::default() }.tidy().unwrap();
        assert_eq!((o.auth.as_deref(), o.kdc.as_deref()), (Some("kerberos"), Some("dc.example.com")));
    }

    #[test]
    fn resolutions() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution(" 1280 X 720 "), Some((1280, 720)));
        assert_eq!(parse_resolution("1920"), None);
        assert_eq!(parse_resolution("9000x1000"), None);
    }

    #[test]
    fn device_scale_steps() {
        assert_eq!(device_scale(100), 100);
        assert_eq!(device_scale(125), 140);
        assert_eq!(device_scale(150), 140);
        assert_eq!(device_scale(200), 180);
    }
}
