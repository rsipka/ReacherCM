//! ZMODEM over an SSH exec channel of its own, beside the terminal's.
//!
//! tmux (like any terminal multiplexer) draws the program's output on its
//! own screen instead of passing the bytes on, so `rz` or `sz` typed in a
//! tmux pane can never reach the app. For a tmux tab the Send and Download
//! buttons run the program here instead, on a channel with no terminal,
//! in the folder the tmux pane is in.

use std::time::Duration;

use russh::ChannelMsg;
use tauri::Emitter;

use super::{Direction, Tap, ZmodemEvent, WINDOW_NETWORK};
use crate::ssh::client::SharedHandle;

/// What to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecKind {
    /// `rz`: the files the user picked go up.
    Receive,
    /// Every file under the folder, through `sz -f`.
    SendFolder,
}

/// `'…'` for sh.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command for `kind`, run by sh in the folder tmux `target`'s pane is
/// in (when there is one). `target` must already be a name this app uses.
pub fn command(kind: ExecKind, target: Option<&str>) -> String {
    let cd = match target {
        Some(t) => format!("cd \"$(tmux display-message -p -t {t} '#{{pane_current_path}}' 2>/dev/null)\" 2>/dev/null; "),
        None => String::new(),
    };
    let body = match kind {
        ExecKind::Receive => "exec rz",
        ExecKind::SendFolder => {
            r#"d="${PWD##*/}"; cd .. && exec find "$d" -type f -exec sz -f {} +"#
        }
    };
    format!("sh -c {}", quote(&format!("{cd}{body}")))
}

/// Run `command` on a new channel of `handle` and drive its transfer,
/// telling the terminal as `zmodem-{id}`. Returns once the program ends.
pub async fn run(
    app: tauri::AppHandle,
    handle: SharedHandle,
    id: String,
    direction: Direction,
    command: String,
) {
    let event = format!("zmodem-{id}");
    let fail = |error: String| {
        let _ = app.emit(&event, ZmodemEvent::Failed { direction, error });
    };
    let channel = {
        let guard = handle.lock().await;
        guard.channel_open_session().await
    };
    let mut channel = match channel {
        Ok(c) => c,
        Err(e) => return fail(format!("Could not open a channel: {e}")),
    };
    if let Err(e) = channel.exec(true, command.as_bytes()).await {
        return fail(format!("Could not start the transfer: {e}"));
    }
    let mut tap = Tap::for_app(&app, &id, WINDOW_NETWORK);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stderr = String::new();
    let mut started = false;
    let mut exit: Option<u32> = None;
    loop {
        tokio::select! {
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { ref data }) => {
                    let z = tap.input(data);
                    started |= tap.busy();
                    if !z.wire.is_empty() && channel.data(&z.wire[..]).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                    if stderr.len() < 4096 {
                        stderr.push_str(&String::from_utf8_lossy(data));
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(exit_status),
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    if !tap.busy() {
                        break;
                    }
                    // Let the transfer see its last bytes through.
                    if exit.is_some() || matches!(msg, None | Some(ChannelMsg::Close)) {
                        break;
                    }
                }
                _ => {}
            },
            _ = tick.tick(), if tap.busy() => {
                let z = tap.poll();
                if !z.wire.is_empty() && channel.data(&z.wire[..]).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = channel.close().await;
    if !started {
        // rz or sz never began: say why, from what it printed.
        let said = stderr
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty() && !l.starts_with("rz waiting"))
            .unwrap_or("")
            .to_string();
        let error = match exit {
            Some(127) => "rz or sz is not installed on the server (package lrzsz)".to_string(),
            _ if !said.is_empty() => said,
            Some(code) => format!("The transfer program ended with code {code}"),
            None => "The transfer program ended".to_string(),
        };
        fail(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_run_in_the_pane_folder() {
        assert_eq!(command(ExecKind::Receive, None), "sh -c 'exec rz'");
        let c = command(ExecKind::Receive, Some("reachercm-web-a1"));
        assert_eq!(
            c,
            r#"sh -c 'cd "$(tmux display-message -p -t reachercm-web-a1 '\''#{pane_current_path}'\'' 2>/dev/null)" 2>/dev/null; exec rz'"#
        );
        assert!(
            command(ExecKind::SendFolder, None).contains(r#"find "$d" -type f -exec sz -f {} +"#)
        );
    }

    /// The commands are what sh runs, quotes and all.
    #[test]
    fn sh_reads_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let c = command(ExecKind::SendFolder, None)
            .replace("exec find", "find")
            .replace("sz -f", "echo");
        let sub = dir.path().join("proj");
        std::fs::create_dir_all(sub.join("a b")).unwrap();
        std::fs::write(sub.join("a b/x.txt"), b"x").unwrap();
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&c)
            .current_dir(&sub)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "proj/a b/x.txt"
        );
    }
}
