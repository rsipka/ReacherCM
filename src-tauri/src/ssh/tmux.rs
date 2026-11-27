//! tmux on the server: an SSH session can start inside a named tmux
//! session, so a dropped connection or a closed app loses nothing and the
//! next connect lands where the last one left off.
//!
//! The session is started as the channel's command, the way the login
//! shell field is, through a small `sh -c` script:
//!
//! - no tmux on the server: the login shell, with one line saying so;
//! - somebody already attached (another tab, another computer): a new
//!   session grouped with it, so each sees its own window, removed again
//!   when it is left;
//! - otherwise `new-session -A`: attach, or create it.
//!
//! An SSH tab instead joins as a view of its own (`TmuxStart::view`): the
//! session is created detached if it is not there, and the tab gets a
//! session grouped with it, named `NAME-VIEW`. Grouped sessions share their
//! windows but each shows its own, so every tab can be one tmux window; the
//! view goes away with its tab (destroy-unattached), the windows stay. The
//! view sets the terminal's title to `@ID NAME` of its window, which is how
//! the tab learns which window it shows.
//!
//! Byobu (on its tmux backend) is tmux with its own config and status
//! line, on the same tmux server: the start script runs `byobu-tmux`
//! where it starts or attaches, and everything else (views, windows, the
//! list, ending a session) is plain tmux. Zellij has no grouped sessions
//! and no way to steer one client from outside, so a Zellij tab attaches
//! (`zellij attach -c`) and nothing more; several tabs on one Zellij
//! session each pick their own Zellij tab there.
//!
//! Only names made of letters, digits, `-` and `_` are accepted, and a
//! login shell is passed on only when it is made of plain words, so
//! nothing from a session can break out of the script.

use serde::{Deserialize, Serialize};

/// The longest name accepted. tmux has no limit, but a status line does.
const MAX_NAME: usize = 64;

/// The multiplexer a session runs in on the server.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    #[default]
    Tmux,
    /// tmux through `byobu-tmux`: Byobu's config and status line.
    Byobu,
    Zellij,
}

impl Tool {
    pub fn is_tmux(&self) -> bool {
        *self == Tool::Tmux
    }
}

/// What a connect asks of tmux.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TmuxStart {
    /// The tmux session to attach to or create.
    pub name: String,
    /// Ask first: the server's tmux sessions are listed, and the user picks
    /// one, a new one (`name`), or no tmux.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask: bool,
    /// Turn tmux's mouse mode on for this session, so the wheel scrolls
    /// tmux's history (the session's own option; .tmux.conf stays as it is).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mouse: bool,
    /// The tab's own view of the session: a session grouped with it, named
    /// `{name}-{view}` (see `view_name`). Unset: the tab attaches to the
    /// session itself, or a view tmux names, as a WSL tab does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// The window a view opens on: `new` for a new one, or a window id
    /// (`@3`). Unset: the window with the latest activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Turn tmux's status line off in the view.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hide_status: bool,
    /// The name this app gave the same session before (`reachercm-…` from
    /// the session's name, now `reachercm-tmux` for all): used
    /// instead of `name` while a session by it is on the server and none
    /// by `name` is, so an upgrade does not leave the old one behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<String>,
    /// tmux, Byobu or Zellij. Zellij takes only `name` and `ask`.
    #[serde(default, skip_serializing_if = "Tool::is_tmux")]
    pub tool: Tool,
}

/// A view's tag: lowercase letters and digits, at most 16.
fn valid_view(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= 16 && tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A window id as tmux writes it: `@` and digits.
fn valid_window_id(id: &str) -> bool {
    id.len() > 1 && id.len() <= 12 && id.starts_with('@') && id[1..].bytes().all(|b| b.is_ascii_digit())
}

/// The session a view tab is attached to: `{name}-{view}`.
pub fn view_name(name: &str, view: &str) -> Result<String, String> {
    if !valid_name(name) || !valid_view(view) {
        return Err(format!("'{name}-{view}' is not a tmux view this app uses"));
    }
    Ok(format!("{name}-{view}"))
}

/// A session's own tmux choice, as saved with it. `mode` is `off`,
/// `attach` (attach or create), `ask` (pick from the server's sessions) or
/// `inherit` (as its folder says, with a name of its own); kept as text so a mode a newer version adds
/// does not make the session unreadable here. Unset on the session: its
/// folder's choice, else off.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TmuxSetting {
    pub mode: String,
    /// The tmux session's name; unset: one made from the session's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// tmux's mouse mode for this session (see `TmuxStart::mouse`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mouse: bool,
    /// `tmux` (unset), `byobu` or `zellij`; text, as `mode` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// A name tmux takes as it is, which the script can hold unquoted.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A login shell value the script can hold unquoted: words of letters,
/// digits and `-_./=+`. Anything else is left out (the window then runs
/// tmux's own default shell).
fn plain_shell(shell: Option<&str>) -> Option<String> {
    let s = shell.map(str::trim).filter(|s| !s.is_empty())?;
    let ok = s.bytes().all(|b| b.is_ascii_alphanumeric() || b" -_./=+".contains(&b));
    if !ok {
        tracing::warn!("tmux: the login shell '{s}' has characters the start script does not pass on; tmux uses its default shell");
        return None;
    }
    // As for a plain login shell: a bare program runs as a login shell.
    Some(if s.split_whitespace().nth(1).is_some() { s.to_string() } else { format!("{s} -l") })
}

/// One tmux session on the server, as the picker lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub name: String,
    pub windows: u32,
    /// Clients attached now.
    pub attached: u32,
    /// Last activity, Unix seconds; 0: not known (Zellij).
    pub activity: u64,
    /// A Zellij session that has ended and is restored when attached to.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exited: bool,
}

/// Lists the server's tmux sessions, one per line, fields split by `:`,
/// which tmux never allows in a session name (a tab would come out as `_`).
pub const LIST_COMMAND: &str =
    "tmux list-sessions -F '#{session_name}:#{session_windows}:#{session_attached}:#{session_activity}:#{session_group}:#{session_group_attached}' 2>/dev/null";

/// The sessions in `tmux list-sessions` output (see `LIST_COMMAND`), most
/// recently used first. Sessions another session was grouped with (a
/// second tab's view, `NAME-1`) are not listed: their group is.
pub fn parse_list(out: &str) -> Vec<Listed> {
    let mut list: Vec<Listed> = out
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() < 4 || f[0].is_empty() {
                return None;
            }
            let group = f.get(4).copied().unwrap_or("");
            if !group.is_empty() && group != f[0] {
                return None;
            }
            Some(Listed {
                name: f[0].to_string(),
                windows: f[1].parse().unwrap_or(0),
                // Tabs attach to views grouped with it (see `view_name`):
                // the group's count is what is attached to it.
                attached: f[2].parse::<u32>().unwrap_or(0).max(f.get(5).and_then(|g| g.parse().ok()).unwrap_or(0)),
                activity: f[3].parse().unwrap_or(0),
                exited: false,
            })
        })
        .collect();
    list.sort_by_key(|l| std::cmp::Reverse(l.activity));
    list
}

/// Lists the server's Zellij sessions, as `zellij list-sessions -n` writes
/// them: `NAME [Created 3m 2s ago] (EXITED - attach to resurrect)`.
pub const ZELLIJ_LIST_COMMAND: &str = "zellij list-sessions -n 2>/dev/null";

/// The sessions in `ZELLIJ_LIST_COMMAND`'s output, running ones first.
/// Zellij says nothing of windows, clients or activity.
pub fn parse_zellij_list(out: &str) -> Vec<Listed> {
    let mut list: Vec<Listed> = out
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            // "No active zellij sessions found." and the like.
            if line.starts_with("No ") {
                return None;
            }
            Some(Listed { name: name.to_string(), windows: 0, attached: 0, activity: 0, exited: line.contains("EXITED") })
        })
        .collect();
    list.sort_by_key(|l| l.exited);
    list
}

/// The list command for `tool`.
pub fn list_command(tool: Tool) -> &'static str {
    if tool == Tool::Zellij { ZELLIJ_LIST_COMMAND } else { LIST_COMMAND }
}

/// The sessions in `list_command(tool)`'s output.
pub fn parse_list_of(tool: Tool, out: &str) -> Vec<Listed> {
    if tool == Tool::Zellij { parse_zellij_list(out) } else { parse_list(out) }
}

/// What the picker answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Attach to this tmux session, or create it.
    Session(String),
    /// The session's own shell, no tmux.
    NoTmux,
}

/// The picker's answer: a session name, or `NO_TMUX`.
pub const NO_TMUX_ANSWER: &str = "\u{1}none";

pub fn read_choice(answer: &str, default_name: &str) -> Result<Choice, String> {
    if answer == NO_TMUX_ANSWER {
        return Ok(Choice::NoTmux);
    }
    let name = if answer.trim().is_empty() { default_name } else { answer.trim() };
    if valid_name(name) {
        Ok(Choice::Session(name.to_string()))
    } else {
        // Someone else's session with a name the script cannot hold.
        Err(format!("The tmux session '{name}' has a name this app cannot attach to; only letters, digits, '-' and '_' work"))
    }
}

/// A tmux session to be picked while the connection opens: the picker
/// needs the logged-in connection to list what the server runs.
#[derive(Clone)]
pub struct Ask {
    /// The connection being opened; the answer is kept under it for the
    /// window (`take_chosen`).
    pub id: String,
    pub start: TmuxStart,
    /// The session's login shell.
    pub shell: Option<String>,
    pub app: tauri::AppHandle,
    pub host: String,
    pub port: u16,
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ask").field("id", &self.id).field("start", &self.start).finish()
    }
}

fn chosen() -> &'static std::sync::Mutex<std::collections::HashMap<String, Option<String>>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<String>>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// What the picker chose for connection `id`, once: `Some(Some(name))` a
/// tmux session, `Some(None)` no tmux, `None` nothing was asked.
pub fn take_chosen(id: &str) -> Option<Option<String>> {
    chosen().lock().unwrap().remove(id)
}

impl Ask {
    /// List the server's sessions, ask, and give the command the session
    /// channel runs (`None`: the login shell as usual).
    pub async fn resolve(&self, handle: &russh::client::Handle<super::client::SshClientHandler>) -> Result<Option<String>, super::client::SshError> {
        use super::client::SshError;
        if !self.start.ask {
            // Not asking, only a name to settle: the old one, if only it is there.
            let name = match self.start.legacy.as_deref().filter(|l| valid_name(l) && self.start.tool != Tool::Zellij) {
                Some(old) => {
                    let out = run_on(handle, &legacy_command(&self.start.name, old)).await.unwrap_or_default();
                    pick_legacy(&out, &self.start.name, old).to_string()
                }
                None => self.start.name.clone(),
            };
            let start = TmuxStart { name: name.clone(), legacy: None, ..self.start.clone() };
            let cmd = start_command(&start, self.shell.as_deref()).map_err(SshError::ConnectionFailed)?;
            chosen().lock().unwrap().insert(self.id.clone(), Some(name));
            return Ok(Some(cmd));
        }
        let listed = parse_list_of(self.start.tool, &run_on(handle, list_command(self.start.tool)).await.unwrap_or_default());
        let fields = listed
            .iter()
            .map(|l| super::prompt::Field {
                text: format!("{}\t{}\t{}\t{}\t{}", l.name, l.windows, l.attached, l.activity, u8::from(l.exited)),
                echo: true,
            })
            .collect();
        let answer = super::prompt::ask(
            Some(&self.app),
            &self.host,
            self.port,
            super::prompt::Kind::Tmux,
            &self.start.name,
            "",
            fields,
        )
        .await
        .ok_or_else(|| SshError::ConnectionFailed("Connection cancelled".into()))?;
        let choice = read_choice(answer.first().map(String::as_str).unwrap_or(""), &self.start.name).map_err(SshError::ConnectionFailed)?;
        let (record, command) = match choice {
            Choice::NoTmux => (None, None),
            Choice::Session(name) => {
                let start = TmuxStart { name: name.clone(), ask: false, legacy: None, ..self.start.clone() };
                let cmd = start_command(&start, self.shell.as_deref()).map_err(SshError::ConnectionFailed)?;
                (Some(name), Some(cmd))
            }
        };
        chosen().lock().unwrap().insert(self.id.clone(), record);
        Ok(command.or_else(|| self.shell.clone()))
    }
}

/// Says `old` when the server has a session `old` and none `name`. In
/// `sh -c`, whatever the account's shell is (fish has no `{ }`).
fn legacy_command(name: &str, old: &str) -> String {
    format!("sh -c 'tmux has-session -t ={name} 2>/dev/null || {{ tmux has-session -t ={old} 2>/dev/null && echo old; }}'")
}

/// The name to use, from `legacy_command`'s output.
fn pick_legacy<'a>(out: &str, name: &'a str, old: &'a str) -> &'a str {
    if out.trim() == "old" { old } else { name }
}

/// Runs `command` on a fresh channel of `handle`, giving what it printed.
async fn run_on(handle: &russh::client::Handle<super::client::SshClientHandler>, command: &str) -> Option<String> {
    let mut channel = handle.channel_open_session().await.ok()?;
    channel.exec(true, command).await.ok()?;
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(10), channel.wait()).await {
            Ok(Some(russh::ChannelMsg::Data { data })) => out.extend_from_slice(&data),
            Ok(Some(russh::ChannelMsg::Eof)) | Ok(None) | Err(_) => break,
            _ => {}
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// The message printed when the server has no tmux.
const NO_TMUX: &str = "tmux was not found on this server, so a normal shell was opened.";
/// Printed when Byobu is asked for and only tmux is there.
const NO_BYOBU: &str = "Byobu was not found on this server, so plain tmux was started.";
/// The message printed when the server has no Zellij.
const NO_ZELLIJ: &str = "Zellij was not found on this server, so a normal shell was opened.";

/// The command the channel runs for `start`, given the session's login
/// shell. Run by the account's login shell as `$SHELL -c`, so it is one
/// `sh -c '…'`, valid in sh, bash, zsh, fish and csh alike.
pub fn start_command(start: &TmuxStart, shell: Option<&str>) -> Result<String, String> {
    let script = start_script(start, shell)?;
    // Single quotes hold the script as it is in every shell above; it has
    // none of its own.
    debug_assert!(!script.contains('\''));
    // No `exec` of its own: the channel adds it to every login shell
    // setting (client.rs, session_opts.rs), and `exec exec` fails in bash.
    Ok(format!("sh -c '{script}'"))
}

/// The script itself, for `sh -c` (a WSL tab runs it with `wsl.exe -e`).
pub fn start_script(start: &TmuxStart, shell: Option<&str>) -> Result<String, String> {
    let n = start.name.trim();
    if !valid_name(n) {
        return Err(format!(
            "The tmux session name '{n}' can only use letters, digits, '-' and '_' (at most {MAX_NAME})"
        ));
    }
    if start.tool == Tool::Zellij {
        return Ok(zellij_script(n, shell));
    }
    let window = plain_shell(shell);
    let window_arg = window.as_deref().map(|w| format!(" {w}")).unwrap_or_default();
    let fallback = window.as_deref().map(|w| format!("exec {w}")).unwrap_or_else(|| "exec \"${SHELL:-/bin/sh}\" -l".to_string());
    let mouse = if start.mouse { " \\; set-option mouse on" } else { "" };
    // What starts or attaches: tmux, or `byobu-tmux` (tmux with Byobu's
    // config; on the same tmux server, so the rest is plain tmux).
    let (pick, run) = if start.tool == Tool::Byobu {
        (
            format!("T=tmux; if command -v byobu-tmux >/dev/null 2>&1; then T=byobu-tmux; else printf \"\\033[33m%s\\033[0m\\r\\n\" \"{NO_BYOBU}\"; fi; "),
            "$T",
        )
    } else {
        (String::new(), "tmux")
    };
    if let Some(tag) = start.view.as_deref() {
        let view = view_name(n, tag)?;
        // What the view shows first: a new window (running the login
        // shell, as the first one does), the one asked for, or the one
        // with the latest activity (the base session's own current window
        // never moves, since nobody is attached to it).
        let select = match start.window.as_deref() {
            Some("new") => format!(" \\; new-window{window_arg}"),
            Some(id) if valid_window_id(id) => format!(" \\; select-window -t :{id}"),
            Some(other) => return Err(format!("'{other}' is not a tmux window")),
            None => " \\; select-window -t \"${W:-:}\"".to_string(),
        };
        let latest = if start.window.is_none() {
            "W=$(tmux list-windows -t ={n} -F \"#{window_activity} #{window_id}\" 2>/dev/null | sort -n | tail -n 1 | cut -d\" \" -f2); "
                .replace("{n}", n)
        } else {
            String::new()
        };
        let status = if start.hide_status { " \\; set-option status off" } else { "" };
        let script = format!(
            concat!(
                "if command -v tmux >/dev/null 2>&1; then ",
                "F=; tmux -TRGB -V >/dev/null 2>&1 && F=-TRGB; {pick}",
                // Still there: the connection dropped before the server
                // noticed. Take it over.
                "if tmux has-session -t ={view} 2>/dev/null; then exec {run} $F attach-session -t ={view}{select_again}; fi; ",
                "tmux has-session -t ={n} 2>/dev/null || {run} new-session -d -s {n}{window} || exit 1; ",
                "{latest}",
                "exec {run} $F new-session -t ={n} -s {view} \\; set-option destroy-unattached on",
                " \\; set-option set-titles on \\; set-option set-titles-string \"#{{window_id}} #W\"",
                "{mouse}{status}{select}; ",
                "fi; ",
                "printf \"\\033[33m%s\\033[0m\\r\\n\" \"{msg}\"; ",
                "{fallback}"
            ),
            n = n,
            pick = pick,
            run = run,
            view = view,
            window = window_arg,
            latest = latest,
            select = select,
            select_again = match start.window.as_deref() {
                Some(id) if valid_window_id(id) => format!(" \\; select-window -t :{id}"),
                _ => String::new(),
            },
            mouse = mouse,
            status = status,
            msg = NO_TMUX,
            fallback = fallback,
        );
        return Ok(script);
    }
    // The old name, while only a session by it is there (a WSL tab; an SSH
    // one settles the name before it starts, see `Ask::resolve`).
    let legacy = match start.legacy.as_deref().filter(|l| valid_name(l) && *l != n) {
        Some(old) => format!("tmux has-session -t ={n} 2>/dev/null || {{ tmux has-session -t ={old} 2>/dev/null && N={old}; }}; "),
        None => String::new(),
    };
    let script = format!(
        concat!(
            "if command -v tmux >/dev/null 2>&1; then ",
            // Truecolor, on tmux 3.2 and later; older ones do not know -T.
            "F=; tmux -TRGB -V >/dev/null 2>&1 && F=-TRGB; {pick}",
            "N={n}; {legacy}",
            "if [ -n \"$(tmux list-clients -t =$N 2>/dev/null)\" ]; then ",
            "exec {run} $F new-session -t =$N \\; set-option destroy-unattached on{mouse}; ",
            "fi; ",
            "exec {run} $F new-session -A -s $N{window}{mouse}; ",
            "fi; ",
            "printf \"\\033[33m%s\\033[0m\\r\\n\" \"{msg}\"; ",
            "{fallback}"
        ),
        n = n,
        pick = pick,
        run = run,
        legacy = legacy,
        window = window_arg,
        msg = NO_TMUX,
        fallback = fallback,
        mouse = mouse,
    );
    Ok(script)
}

/// Zellij: attach to session `n`, or create it (an exited one is
/// restored). The login shell, when it is one plain program, is the
/// session's default shell; a new session only, as Zellij keeps the first.
fn zellij_script(n: &str, shell: Option<&str>) -> String {
    let prog = shell
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./+".contains(&b)));
    let options = prog.map(|p| format!(" options --default-shell {p}")).unwrap_or_default();
    let fallback = plain_shell(shell).map(|w| format!("exec {w}")).unwrap_or_else(|| "exec \"${SHELL:-/bin/sh}\" -l".to_string());
    format!(
        "if command -v zellij >/dev/null 2>&1; then exec zellij attach -c {n}{options}; fi; printf \"\\033[33m%s\\033[0m\\r\\n\" \"{NO_ZELLIJ}\"; {fallback}"
    )
}

/// One window of a tab's tmux session.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// tmux's id for it (`@3`), which stays while it lives.
    pub id: String,
    pub index: u32,
    pub name: String,
    /// The program in its active pane.
    pub command: String,
    pub panes: u32,
    /// The window the tab's view shows.
    pub active: bool,
}

/// Lists the windows of view `view`, one per line, the name last (it is the
/// only field that can hold a `:`).
pub fn windows_command(view: &str) -> String {
    format!("tmux list-windows -t ={view} -F '#{{window_id}}:#{{window_index}}:#{{window_active}}:#{{window_panes}}:#{{pane_current_command}}:#{{window_name}}' 2>/dev/null")
}

pub fn parse_windows(out: &str) -> Vec<Window> {
    let mut list: Vec<Window> = out
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(6, ':').collect();
            if f.len() < 6 || !valid_window_id(f[0]) {
                return None;
            }
            Some(Window {
                id: f[0].to_string(),
                index: f[1].parse().unwrap_or(0),
                active: f[2] == "1",
                panes: f[3].parse().unwrap_or(1),
                command: f[4].to_string(),
                name: f[5].to_string(),
            })
        })
        .collect();
    list.sort_by_key(|w| w.index);
    list
}

/// What a tab's Windows menu asks of its view.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WindowOp {
    /// Show this window in the tab.
    Select { window: String },
    Rename { window: String, name: String },
    Close { window: String },
    /// A new window, shown in the tab.
    New,
}

/// Quoted for the account's shell (sh, bash, zsh, fish and csh alike).
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The command for `op` on view `view`; `shell` is the session's login
/// shell, which a new window runs as the first one does.
pub fn window_command(view: &str, op: &WindowOp, shell: Option<&str>) -> Result<String, String> {
    let target = |id: &str| -> Result<String, String> {
        if valid_window_id(id) {
            Ok(format!("={view}:{id}"))
        } else {
            Err(format!("'{id}' is not a tmux window"))
        }
    };
    Ok(match op {
        WindowOp::Select { window } => format!("tmux select-window -t {}", target(window)?),
        WindowOp::Close { window } => format!("tmux kill-window -t {}", target(window)?),
        WindowOp::Rename { window, name } => {
            let name: String = name.chars().filter(|c| !c.is_control()).take(100).collect();
            let name = name.trim();
            if name.is_empty() {
                return Err("A tmux window needs a name".into());
            }
            format!("tmux rename-window -t {} {}", target(window)?, quoted(name))
        }
        WindowOp::New => {
            let shell = plain_shell(shell).map(|w| format!(" {w}")).unwrap_or_default();
            format!("tmux new-window -t ={view}:{shell}")
        }
    })
}

/// Ends the tmux session `name` and every session grouped with it (the
/// ones other tabs joined as), which keep its windows alive otherwise.
pub fn kill_command(name: &str, tool: Tool) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!("'{name}' is not a tmux session name this app uses"));
    }
    if tool == Tool::Zellij {
        // Killed, then deleted so it is not kept to be restored.
        return Ok(format!("sh -c 'zellij kill-session {name} 2>/dev/null; zellij delete-session {name} 2>/dev/null; exit 0'"));
    }
    Ok(format!(
        "sh -c 'for s in $(tmux list-sessions -F \"#{{session_group}}:#{{session_name}}\" 2>/dev/null | sed -n \"s/^{name}://p\"); do tmux kill-session -t \"=$s\"; done; tmux kill-session -t ={name} 2>/dev/null; exit 0'"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(name: &str) -> TmuxStart {
        TmuxStart { name: name.to_string(), ..Default::default() }
    }

    fn view(window: Option<&str>) -> TmuxStart {
        TmuxStart { name: "w".into(), view: Some("ab12".into()), window: window.map(String::from), ..Default::default() }
    }

    #[test]
    fn a_view_joins_the_session_as_its_own() {
        let c = start_script(&view(None), None).unwrap();
        assert!(c.contains("tmux has-session -t =w 2>/dev/null || tmux new-session -d -s w || exit 1;"), "{c}");
        assert!(c.contains("new-session -t =w -s w-ab12 \\; set-option destroy-unattached on"), "{c}");
        assert!(c.contains("set-titles-string \"#{window_id} #W\""), "{c}");
        assert!(c.contains("select-window -t \"${W:-:}\""), "{c}");
        assert!(!c.contains("status off"), "{c}");
        let c = start_script(&TmuxStart { hide_status: true, ..view(Some("new")) }, Some("fish")).unwrap();
        assert!(c.contains("set-option status off \\; new-window fish -l;"), "{c}");
        assert!(!c.contains("W=$("), "{c}");
        let c = start_script(&view(Some("@7")), None).unwrap();
        assert!(c.contains("attach-session -t =w-ab12 \\; select-window -t :@7;"), "{c}");
        assert!(c.contains("\\; select-window -t :@7; fi;"), "{c}");
    }

    #[test]
    fn views_and_windows_are_checked() {
        assert!(start_script(&view(Some("@7; rm")), None).is_err());
        assert!(start_script(&TmuxStart { view: Some("AB".into()), ..view(None) }, None).is_err());
        assert!(view_name("w", "x y").is_err());
        assert_eq!(view_name("w", "ab12").unwrap(), "w-ab12");
        assert!(window_command("w-1", &WindowOp::Select { window: "3".into() }, None).is_err());
        assert_eq!(window_command("w-1", &WindowOp::Close { window: "@3".into() }, None).unwrap(), "tmux kill-window -t =w-1:@3");
        assert_eq!(
            window_command("w-1", &WindowOp::Rename { window: "@3".into(), name: "it's\nlogs".into() }, None).unwrap(),
            "tmux rename-window -t =w-1:@3 'it'\\''slogs'"
        );
        assert!(window_command("w-1", &WindowOp::Rename { window: "@3".into(), name: " ".into() }, None).is_err());
        assert_eq!(window_command("w-1", &WindowOp::New, Some("zsh")).unwrap(), "tmux new-window -t =w-1: zsh -l");
    }

    #[test]
    fn windows_are_listed_in_order_names_may_hold_colons() {
        let out = "@4:2:0:1:vim:notes: todo\n@1:0:1:2:bash:main\nbroken\n";
        let w = parse_windows(out);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0], Window { id: "@1".into(), index: 0, name: "main".into(), command: "bash".into(), panes: 2, active: true });
        assert_eq!(w[1].name, "notes: todo");
    }

    #[test]
    fn mouse_mode_is_set_on_the_session_only() {
        let c = start_command(&TmuxStart { name: "w".into(), mouse: true, ..Default::default() }, None).unwrap();
        assert!(c.contains("new-session -A -s $N \\; set-option mouse on;"), "{c}");
        assert!(c.contains("destroy-unattached on \\; set-option mouse on;"), "{c}");
        assert!(!c.contains("-g"), "{c}");
    }

    #[test]
    fn the_list_is_read_newest_first_without_grouped_views() {
        let out = "old:1:0:100:\nw:3:1:300:w\nw-1:3:1:400:w\nnew:2:0:200:\nbroken\n";
        let names: Vec<String> = parse_list(out).into_iter().map(|l| l.name).collect();
        assert_eq!(names, ["w", "new", "old"]);
        assert_eq!(parse_list(out)[0], Listed { name: "w".into(), windows: 3, attached: 1, activity: 300, exited: false });
    }

    #[test]
    fn a_session_counts_the_tabs_attached_to_its_views() {
        let out = "w:3:0:300:w:2\nw-ab12:3:1:300:w:2\nw-cd34:3:1:300:w:2\nsolo:1:1:100::\n";
        let l = parse_list(out);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].name.as_str(), l[0].attached), ("w", 2));
        assert_eq!((l[1].name.as_str(), l[1].attached), ("solo", 1));
    }

    #[test]
    fn the_list_command_splits_on_colons() {
        assert!(LIST_COMMAND.contains("#{session_name}:#{session_windows}"));
        assert_eq!(LIST_COMMAND.matches('\'').count(), 2);
    }

    #[test]
    fn a_choice_is_a_name_the_default_or_no_tmux() {
        assert_eq!(read_choice("main", "reachercm-w"), Ok(Choice::Session("main".into())));
        assert_eq!(read_choice("", "reachercm-w"), Ok(Choice::Session("reachercm-w".into())));
        assert_eq!(read_choice(NO_TMUX_ANSWER, "reachercm-w"), Ok(Choice::NoTmux));
        assert!(read_choice("my session", "reachercm-w").is_err());
    }

    #[test]
    fn names_are_plain_words() {
        assert!(valid_name("reachercm-web-01"));
        assert!(valid_name("main_2"));
        for bad in ["", "-x", "a b", "a;b", "a'b", "a$b", "a:b", "a.b", "ä", &"x".repeat(65)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_bad_name_is_refused() {
        assert!(start_command(&start("x'; rm -rf ~"), None).is_err());
        assert!(kill_command("a b", Tool::Tmux).is_err());
    }

    #[test]
    fn the_command_is_one_quoted_script() {
        let c = start_command(&start("reachercm-web"), None).unwrap();
        assert!(c.starts_with("sh -c '"), "{c}");
        assert!(c.ends_with('\''));
        assert_eq!(c.matches('\'').count(), 2, "{c}");
        assert!(c.contains("N=reachercm-web; "), "{c}");
        assert!(c.contains("new-session -A -s $N;"), "{c}");
        assert!(c.contains("new-session -t =$N \\; set-option destroy-unattached on"), "{c}");
        assert!(c.contains("exec \"${SHELL:-/bin/sh}\" -l"), "{c}");
    }

    #[test]
    fn the_login_shell_runs_in_the_window_and_as_the_fallback() {
        let c = start_command(&start("w"), Some("fish")).unwrap();
        assert!(c.contains("new-session -A -s $N fish -l;"), "{c}");
        assert!(c.ends_with("exec fish -l'"), "{c}");
        let c = start_command(&start("w"), Some("zsh -i")).unwrap();
        assert!(c.contains("new-session -A -s $N zsh -i;"), "{c}");
    }

    #[test]
    fn a_login_shell_with_quotes_is_left_out() {
        let c = start_command(&start("w"), Some("bash -c 'echo hi'")).unwrap();
        assert!(c.contains("new-session -A -s $N;"), "{c}");
        assert_eq!(c.matches('\'').count(), 2, "{c}");
    }

    /// Runs the script with a fake tmux on PATH: what it was asked to do.
    /// Not on Android, whose shell is not at /bin/sh.
    #[cfg(all(unix, not(target_os = "android")))]
    fn run_with_fake_tmux(attached: bool, have_tmux: bool) -> String {
        run_with_fake_tmux_as(&start("w"), attached, have_tmux, "")
    }

    /// The same for `start`, on a server whose tmux has the sessions `has`
    /// (space separated) for `has-session`.
    #[cfg(all(unix, not(target_os = "android")))]
    fn run_with_fake_tmux_as(start: &TmuxStart, attached: bool, have_tmux: bool, has: &str) -> String {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("reachercm-tmux-test-{}-{attached}-{have_tmux}-{}", std::process::id(), has.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        if have_tmux {
            let fake = dir.join("tmux");
            let clients = if attached { "echo /dev/pts/1: w" } else { "true" };
            std::fs::write(
                &fake,
                format!(
                    "#!/bin/sh\ncase \"$*\" in '-TRGB -V') exit 0;; list-clients*) {clients}; exit 0;; has-session*) for s in {has}; do [ \"$3\" = \"=$s\" ] && exit 0; done; exit 1;; esac\necho \"$@\" >> {}\n",
                    log.display()
                ),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // Only these on PATH, so a real tmux on this machine is never found.
        std::os::unix::fs::symlink("/bin/sh", dir.join("sh")).unwrap();
        let shim = dir.join("say");
        std::fs::write(&shim, "#!/bin/sh\necho \"$@\"\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cmd = start_command(start, Some("say")).unwrap();
        // As the channel sends it: every login shell setting gets an `exec`.
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("exec {cmd}"))
            .env("PATH", &dir)
            .output()
            .unwrap();
        let logged = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        format!("{}{}", logged, String::from_utf8_lossy(&out.stdout))
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn the_script_attaches_or_creates() {
        assert_eq!(run_with_fake_tmux(false, true).trim(), "-TRGB new-session -A -s w say -l");
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn the_script_joins_as_a_group_when_someone_is_attached() {
        assert_eq!(run_with_fake_tmux(true, true).trim(), "-TRGB new-session -t =w ; set-option destroy-unattached on");
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn the_script_keeps_an_old_name_only_while_it_alone_is_there() {
        let s = TmuxStart { legacy: Some("old".into()), ..start("w") };
        assert_eq!(run_with_fake_tmux_as(&s, false, true, "old").trim(), "-TRGB new-session -A -s old say -l");
        assert_eq!(run_with_fake_tmux_as(&s, false, true, "old w").trim(), "-TRGB new-session -A -s w say -l");
        assert_eq!(run_with_fake_tmux_as(&s, false, true, "").trim(), "-TRGB new-session -A -s w say -l");
    }

    #[test]
    fn an_old_name_is_picked_only_when_the_server_says_so() {
        assert_eq!(legacy_command("reachercm-tmux", "reachercm-w"), "sh -c 'tmux has-session -t =reachercm-tmux 2>/dev/null || { tmux has-session -t =reachercm-w 2>/dev/null && echo old; }'");
        assert_eq!(pick_legacy("old\n", "new", "old"), "old");
        assert_eq!(pick_legacy("", "new", "old"), "new");
    }

    #[test]
    fn byobu_starts_and_attaches_through_byobu_tmux() {
        let s = TmuxStart { tool: Tool::Byobu, ..start("w") };
        let c = start_command(&s, None).unwrap();
        assert_eq!(c.matches('\'').count(), 2, "{c}");
        assert!(c.contains("then T=byobu-tmux; else printf"), "{c}");
        assert!(c.contains("exec $T $F new-session -A -s $N;"), "{c}");
        assert!(c.contains("exec $T $F new-session -t =$N"), "{c}");
        // Listing and the rest stay plain tmux: the same server.
        assert!(c.contains("tmux list-clients -t =$N"), "{c}");
        let v = start_script(&TmuxStart { tool: Tool::Byobu, ..view(None) }, None).unwrap();
        assert!(v.contains("|| $T new-session -d -s w || exit 1;"), "{v}");
        assert!(v.contains("exec $T $F new-session -t =w -s w-ab12"), "{v}");
        // Plain tmux is as it was.
        assert!(!start_command(&start("w"), None).unwrap().contains("byobu"));
    }

    #[test]
    fn zellij_attaches_or_creates_and_nothing_more() {
        let z = TmuxStart { tool: Tool::Zellij, mouse: true, view: Some("ab12".into()), hide_status: true, ..start("w") };
        let c = start_command(&z, Some("fish")).unwrap();
        assert_eq!(c.matches('\'').count(), 2, "{c}");
        assert!(c.contains("exec zellij attach -c w options --default-shell fish;"), "{c}");
        assert!(c.contains(NO_ZELLIJ), "{c}");
        assert!(c.ends_with("exec fish -l'"), "{c}");
        assert!(!c.contains("tmux"), "{c}");
        // A shell with arguments is not a default shell for Zellij.
        let c = start_command(&TmuxStart { tool: Tool::Zellij, ..start("w") }, Some("zsh -i")).unwrap();
        assert!(c.contains("exec zellij attach -c w;"), "{c}");
        assert!(start_command(&TmuxStart { tool: Tool::Zellij, ..start("a;b") }, None).is_err());
    }

    #[test]
    fn zellij_sessions_are_listed_running_first() {
        let out = "old [Created 2days ago] (EXITED - attach to resurrect)\nwork [Created 3m 2s ago] (current)\n\nreachercm-zellij [Created 1h ago]\n";
        let l = parse_zellij_list(out);
        let names: Vec<&str> = l.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["work", "reachercm-zellij", "old"]);
        assert!(l[2].exited && !l[0].exited);
        assert!(parse_zellij_list("No active zellij sessions found.\n").is_empty());
        assert_eq!(list_command(Tool::Byobu), LIST_COMMAND);
        assert_eq!(list_command(Tool::Zellij), ZELLIJ_LIST_COMMAND);
    }

    #[test]
    fn a_zellij_session_is_killed_and_deleted() {
        assert_eq!(
            kill_command("w", Tool::Zellij).unwrap(),
            "sh -c 'zellij kill-session w 2>/dev/null; zellij delete-session w 2>/dev/null; exit 0'"
        );
        assert!(kill_command("w", Tool::Byobu).unwrap().contains("tmux kill-session"));
    }

    #[test]
    fn the_tool_is_tmux_unless_said() {
        let s: TmuxStart = serde_json::from_str(r#"{"name":"w"}"#).unwrap();
        assert_eq!(s.tool, Tool::Tmux);
        let s: TmuxStart = serde_json::from_str(r#"{"name":"w","tool":"zellij"}"#).unwrap();
        assert_eq!(s.tool, Tool::Zellij);
        assert!(!serde_json::to_string(&start("w")).unwrap().contains("tool"));
        let saved: TmuxSetting = serde_json::from_str(r#"{"mode":"attach","tool":"something-newer"}"#).unwrap();
        assert_eq!(saved.tool.as_deref(), Some("something-newer"));
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn without_tmux_the_script_says_so_and_runs_the_shell() {
        let out = run_with_fake_tmux(false, false);
        assert!(out.contains(NO_TMUX), "{out}");
        // `say -l` is the stand-in login shell: it prints its arguments.
        assert!(out.trim_end().ends_with("-l"), "{out}");
    }
}
