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
use ironrdp_pdu::window::{DesktopOrder, MonitoredDesktop, ShowState, WindowOrderBody, WindowingOrder};
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
    /// Where the window is on the remote desktop, in its pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// A window a taskbar would show (else a menu, tooltip, dialog part...).
    pub listed: bool,
    /// The window that owns it (a dialog's or a menu's), 0 for none.
    pub owner: u32,
    /// The window the server has in front (the one keys go to).
    pub active: bool,
    /// Its place in the server's stacking order, 0 at the top; windows the
    /// server has not ranked come after the ranked ones.
    pub z: u32,
}

/// What the server has said about each window so far. Orders carry only the
/// fields that changed, so each window's state is built up from them.
#[derive(Debug, Default)]
pub struct Windows {
    known: BTreeMap<u32, Known>,
    /// The server's front window, when it says.
    active: Option<u32>,
    /// The server's stacking order, topmost first, when it says.
    z_order: Vec<u32>,
}

#[derive(Debug, Default, Clone)]
struct Known {
    title: String,
    owner: u32,
    style: u32,
    ex_style: u32,
    show: Option<ShowState>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Windows {
    /// Take in one batch of orders. True when the window row changed.
    pub fn apply(&mut self, orders: &[WindowingOrder]) -> bool {
        let before = self.list();
        for order in orders {
            let order = match order {
                WindowingOrder::Window(order) => order,
                WindowingOrder::Desktop(desktop) => {
                    if desktop.resets_windows() {
                        self.known.clear();
                        self.active = None;
                        self.z_order.clear();
                    }
                    if let DesktopOrder::ActivelyMonitored(m) = desktop {
                        if let Some(active) = m.active_window_id {
                            self.active = (active != MonitoredDesktop::NO_ACTIVE_WINDOW).then_some(active);
                        }
                        if let Some(z) = &m.z_order {
                            self.z_order.clone_from(z);
                        }
                    }
                    continue;
                }
                WindowingOrder::NotifyIcon(_) => continue,
            };
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
                    if let Some(at) = &state.window_offset {
                        (known.x, known.y) = (at.x, at.y);
                    }
                    if let Some(size) = &state.window_size {
                        (known.width, known.height) = (size.width, size.height);
                    }
                }
                WindowOrderBody::Icon(_) => {}
            }
        }
        self.list() != before
    }

    /// Every window that is up (shown or minimised, not a child), oldest
    /// first. `listed` marks those a taskbar would show: not tool windows,
    /// not owned unless they ask to be listed.
    pub fn list(&self) -> Vec<RailWindow> {
        self.known
            .iter()
            .filter(|(_, w)| {
                matches!(w.show, Some(ShowState::Show | ShowState::Minimized | ShowState::Maximized)) && w.style & WS_CHILD == 0
            })
            .map(|(id, w)| RailWindow {
                id: *id,
                title: w.title.clone(),
                state: match w.show {
                    Some(ShowState::Minimized) => "minimized",
                    Some(ShowState::Maximized) => "maximized",
                    _ => "normal",
                },
                x: w.x,
                y: w.y,
                width: w.width,
                height: w.height,
                listed: w.ex_style & WS_EX_APPWINDOW != 0 || (w.owner == 0 && w.ex_style & WS_EX_TOOLWINDOW == 0),
                owner: w.owner,
                active: self.active == Some(*id),
                z: self.z_order.iter().position(|z| z == id).map_or(u32::MAX, |p| u32::try_from(p).unwrap_or(u32::MAX)),
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
    use ironrdp_pdu::window::{WindowOrder, WindowPoint, WindowSize, WindowState, WindowStyle};

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
        let listed: Vec<_> = w.list().into_iter().filter(|w| w.listed).collect();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].id, listed[0].title.as_str(), listed[0].state), (1, "Untitled - Notepad", "normal"));
        // The tooltip is up, only not listed.
        assert!(w.list().iter().any(|w| w.id == 2 && !w.listed && w.owner == 1));

        // Only the changed field comes again.
        assert!(w.apply(&[state(1, false, WindowState { show_state: Some(ShowState::Minimized), ..Default::default() })]));
        assert_eq!(w.list()[0].state, "minimized");
        assert_eq!(w.list()[0].title, "Untitled - Notepad");
        assert!(w.apply(&[state(1, false, WindowState { window_offset: Some(WindowPoint { x: 40, y: 30 }), window_size: Some(WindowSize { width: 640, height: 480 }), ..Default::default() })]));
        assert_eq!((w.list()[0].x, w.list()[0].y, w.list()[0].width, w.list()[0].height), (40, 30, 640, 480));

        // The server brings window 1 forward.
        assert!(w.apply(&[WindowingOrder::Desktop(DesktopOrder::ActivelyMonitored(MonitoredDesktop {
            active_window_id: Some(1),
            z_order: Some(vec![1, 2]),
            ..Default::default()
        }))]));
        let one = w.list().into_iter().find(|w| w.id == 1).unwrap();
        assert!(one.active && one.z == 0);

        assert!(w.apply(&[WindowingOrder::Window(WindowOrder { window_id: 1, is_new: false, body: WindowOrderBody::Deleted })]));
        assert!(w.list().iter().all(|w| w.id != 1));
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
