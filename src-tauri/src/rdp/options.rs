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
}

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
