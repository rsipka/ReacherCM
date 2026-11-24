//! RemoteApp (RAIL): a session that shows one program's windows rather than
//! a whole desktop.
//!
//! The windows are drawn by the server into the session's picture at their
//! own places, over an empty background, so the panel shows them as it shows
//! a desktop and input goes where it always goes. What this adds is the
//! window list the server keeps the client informed of, which the panel shows
//! as a row of buttons (there is no remote taskbar to bring back a minimised
//! window), and the commands those buttons send.
//!
//! The client does not offer local move and resize, so dragging a window is
//! done by the server, inside the picture, as on a desktop.

use std::collections::BTreeMap;

use ironrdp_client::rdp::RdpInputSender;
use ironrdp_pdu::window::{ShowState, WindowOrderBody, WindowingOrder};
use ironrdp_rail::pdu::{ActivatePdu, ExecutePdu, ExecuteResult, ExecuteResultPdu, SystemCommand, SystemCommandPdu};
use serde::Serialize;

use super::options::RdpOptions;

/// WS_EX_TOOLWINDOW: palettes and floating toolbars, never on a taskbar.
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
/// WS_EX_APPWINDOW: on the taskbar even when owned.
const WS_EX_APPWINDOW: u32 = 0x0004_0000;
/// WS_CHILD: part of another window.
const WS_CHILD: u32 = 0x4000_0000;

/// One window as the panel's window row shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RailWindow {
    pub id: u32,
    pub title: String,
    /// `normal`, `minimized` or `maximized`.
    pub state: &'static str,
}

/// What the server has said about each window so far. Orders carry only the
/// fields that changed, so each window's state is built up from them.
#[derive(Debug, Default)]
pub struct Windows {
    known: BTreeMap<u32, Known>,
}

#[derive(Debug, Default, Clone)]
struct Known {
    title: String,
    owner: u32,
    style: u32,
    ex_style: u32,
    show: Option<ShowState>,
}

impl Windows {
    /// Take in one batch of orders. True when the window row changed.
    pub fn apply(&mut self, orders: &[WindowingOrder]) -> bool {
        let before = self.list();
        for order in orders {
            let WindowingOrder::Window(order) = order else { continue };
            match &order.body {
                WindowOrderBody::Deleted => {
                    self.known.remove(&order.window_id);
                }
                WindowOrderBody::State(state) => {
                    let known = self.known.entry(order.window_id).or_default();
                    if let Some(title) = &state.title {
                        known.title.clone_from(title);
                    }
                    if let Some(owner) = state.owner_window_id {
                        known.owner = owner;
                    }
                    if let Some(style) = &state.style {
                        known.style = style.style;
                        known.ex_style = style.extended_style;
                    }
                    if let Some(show) = state.show_state {
                        known.show = Some(show);
                    }
                }
                WindowOrderBody::Icon(_) => {}
            }
        }
        self.list() != before
    }

    /// The windows a taskbar would show, oldest first: shown or minimised,
    /// not tool windows or children, not owned unless they ask to be listed.
    pub fn list(&self) -> Vec<RailWindow> {
        self.known
            .iter()
            .filter(|(_, w)| {
                let shown = matches!(w.show, Some(ShowState::Show | ShowState::Minimized | ShowState::Maximized));
                let listed = w.ex_style & WS_EX_APPWINDOW != 0 || (w.owner == 0 && w.ex_style & WS_EX_TOOLWINDOW == 0);
                shown && listed && w.style & WS_CHILD == 0
            })
            .map(|(id, w)| RailWindow {
                id: *id,
                title: w.title.clone(),
                state: match w.show {
                    Some(ShowState::Minimized) => "minimized",
                    Some(ShowState::Maximized) => "maximized",
                    _ => "normal",
                },
            })
            .collect()
    }
}

/// The launch request for the session's program, if it is a RemoteApp.
pub fn execute(options: Option<&RdpOptions>) -> Option<ExecutePdu> {
    let o = options.filter(|o| o.is_remote_app())?;
    Some(ExecutePdu {
        flags: 0,
        executable: o.program.as_deref()?.trim().to_string(),
        working_directory: o.work_dir.as_deref().map(str::trim).unwrap_or_default().to_string(),
        arguments: o.app_args.as_deref().map(str::trim).unwrap_or_default().to_string(),
    })
}

/// Why the server would not start the program, in words; None when it did.
pub fn launch_error(result: &ExecuteResultPdu) -> Option<String> {
    let why = match result.result {
        ExecuteResult::Ok => return None,
        ExecuteResult::NotInAllowlist => {
            "the server does not allow this program as a RemoteApp. On Windows Server, publish it in RemoteApp Programs; on Windows 10/11 Pro, set fAllowUnlistedRemotePrograms"
        }
        ExecuteResult::FileNotFound => "the program was not found on the server",
        ExecuteResult::SessionLocked => "the remote session is locked",
        ExecuteResult::HookNotLoaded => "the server's RemoteApp support did not start",
        ExecuteResult::DecodeFailed | ExecuteResult::Fail => "the server could not start it",
    };
    Some(format!("RemoteApp {}: {why}.", result.executable))
}

/// A window button's command.
pub fn send_command(input: &RdpInputSender, window_id: u32, command: &str) -> Result<(), String> {
    use ironrdp_client::rail::RailInputEvent;
    let system = |command| RailInputEvent::SystemCommand(SystemCommandPdu { window_id, command });
    let event = match command {
        "activate" => RailInputEvent::Activate(ActivatePdu { window_id, enabled: true }),
        "close" => system(SystemCommand::Close),
        "minimize" => system(SystemCommand::Minimize),
        "maximize" => system(SystemCommand::Maximize),
        "restore" => system(SystemCommand::Restore),
        other => return Err(format!("unknown window command {other}")),
    };
    input.try_send_rail_input(event).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_pdu::window::{WindowOrder, WindowState, WindowStyle};

    fn state(id: u32, is_new: bool, state: WindowState) -> WindowingOrder {
        WindowingOrder::Window(WindowOrder { window_id: id, is_new, body: WindowOrderBody::State(Box::new(state)) })
    }

    #[test]
    fn builds_the_window_row_from_partial_orders() {
        let mut w = Windows::default();
        let changed = w.apply(&[
            state(1, true, WindowState {
                title: Some("Untitled - Notepad".into()),
                owner_window_id: Some(0),
                style: Some(WindowStyle { style: 0x14CF_0000, extended_style: 0x100 }),
                show_state: Some(ShowState::Show),
                ..Default::default()
            }),
            // A tooltip: owned tool window, not listed.
            state(2, true, WindowState {
                owner_window_id: Some(1),
                style: Some(WindowStyle { style: 0, extended_style: WS_EX_TOOLWINDOW }),
                show_state: Some(ShowState::Show),
                ..Default::default()
            }),
        ]);
        assert!(changed);
        assert_eq!(w.list(), vec![RailWindow { id: 1, title: "Untitled - Notepad".into(), state: "normal" }]);

        // Only the changed field comes again.
        assert!(w.apply(&[state(1, false, WindowState { show_state: Some(ShowState::Minimized), ..Default::default() })]));
        assert_eq!(w.list()[0].state, "minimized");
        assert_eq!(w.list()[0].title, "Untitled - Notepad");
        assert!(!w.apply(&[state(2, false, WindowState { title: Some("tip".into()), ..Default::default() })]));

        assert!(w.apply(&[WindowingOrder::Window(WindowOrder { window_id: 1, is_new: false, body: WindowOrderBody::Deleted })]));
        assert!(w.list().is_empty());
    }

    #[test]
    fn launch_request_only_for_a_remote_app() {
        let plain = RdpOptions { program: Some("notepad".into()), ..Default::default() };
        assert!(execute(Some(&plain)).is_none());
        let app = RdpOptions { remote_app: Some(true), app_args: Some(" a.txt ".into()), ..plain };
        let pdu = execute(Some(&app)).unwrap();
        assert_eq!((pdu.executable.as_str(), pdu.arguments.as_str()), ("notepad", "a.txt"));
    }
}
