use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
#[cfg(desktop)]
use std::sync::Mutex;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::mcp::McpState;
use crate::monitoring::collector::MonitoringCollector;
#[cfg(desktop)]
use crate::pty::manager::PtyManager;
#[cfg(desktop)]
use crate::serial::port::SerialManager;
use crate::rdp::RdpManager;
use crate::vnc::VncManager;
use crate::spice::SpiceManager;
use crate::ssh::client::SshManager;
use crate::tofu::types::SchemaCache;
use crate::tunnel::manager::TunnelManager;
use crate::vault::VaultManager;
use crate::plugin::manager::PluginManager;

/// Configuration for a jump host in a ProxyJump chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JumpHostConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    /// A saved credential this hop logs in with, from the session's vault.
    /// Its username and login win over the hop's own, which stay stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    /// Read only: the saved credential the hop's login was filled in from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialUse>,
}

/// What a saved session connects with. Sessions saved before this existed
/// have no `kind` on disk and read back as SSH, which is what they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Ssh,
    Rdp,
    Vnc,
    Wsl,
    Telnet,
    /// A local serial port (`host` is the port name). Desktop only.
    Serial,
    /// MikroTik WinBox 4, started as a program of its own. Desktop only.
    Winbox,
    /// A SPICE display (KVM/QEMU, Proxmox, libvirt).
    Spice,
    /// A web management page (iDRAC, iLO, Proxmox VE...), shown in a window
    /// of its own that cannot reach the app; see `crate::web`.
    Web,
    /// FTP or FTPS, shown in the full-pane file manager. Desktop only.
    Ftp,
    /// A server's BMC (iDRAC, iLO...) over Redfish: health, sensors, logs,
    /// power and virtual media; see `crate::bmc`.
    Bmc,
    /// A Proxmox VE node or cluster over its API: guests, power, snapshots,
    /// tasks and consoles; see `crate::proxmox`.
    Proxmox,
    /// An MQTT broker: its topics as they arrive, publishing; see `crate::mqtt`.
    Mqtt,
}

impl SessionKind {
    fn is_ssh(&self) -> bool {
        *self == SessionKind::Ssh
    }
}

/// Configuration for a saved session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    /// SSH unless said otherwise; omitted from disk when it is SSH so the
    /// file stays readable by older builds.
    #[serde(default, skip_serializing_if = "SessionKind::is_ssh")]
    pub kind: SessionKind,
    /// Windows logon domain, RDP only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// A local folder shown inside the remote desktop as a drive, RDP only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_path: Option<String>,
    /// What the RDP clipboard lets through: unset for text, pictures and
    /// files, `text` for no files, `off` for nothing. RDP only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdp_clipboard: Option<String>,
    /// Open this RDP session in the installed FreeRDP client on desktop.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rdp_use_freerdp: bool,
    /// Forward lock key presses to the remote desktop. Enabled when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdp_forward_lock_keys: Option<bool>,
    /// RDP only: display, keyboard, sound, performance and program settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdp: Option<crate::rdp::options::RdpOptions>,
    /// Key sessions only: offer the SSH agent's keys if this session's own key
    /// is refused. Off unless the user turned it on; see `build_auth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub try_agent_keys: Option<bool>,
    /// VNC and SPICE only: a saved SSH session to reach the server through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_session_id: Option<String>,
    /// WSL only: distribution name (e.g. "Ubuntu")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsl_distro: Option<String>,
    /// Telnet only: how it talks to the device (mode, Enter, Backspace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telnet: Option<crate::telnet::protocol::TelnetOptions>,
    /// SPICE only: TLS port and how the server's certificate is checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spice: Option<crate::spice::SpiceOptions>,
    /// VNC only: encryption (VeNCrypt, or TLS from the first byte) and the
    /// certificates it trusts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vnc: Option<crate::vnc::VncOptions>,
    /// Serial only: speed, framing, flow control and keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<crate::serial::options::SerialOptions>,
    pub folder_id: Option<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_os: Option<String>,
    /// `detected_os` was picked by hand in the session editor, so connecting
    /// never replaces it. False: found on connect (SSH) or not set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub os_manual: bool,
    /// Which vault this session belongs to (None = private __sessions__ vault)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<String>,
    /// A WireGuard profile (`ipc::wireguard_commands`) whose tunnel carries
    /// the connection to the main address, or to the first jump host. SSH
    /// and Telnet only for now. The tunnel is brought up on connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wireguard_id: Option<String>,
    /// ProxyJump chain: ordered list of jump hosts to tunnel through
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_chain: Option<Vec<JumpHostConfig>>,
    /// Proxy configuration (SOCKS5, SOCKS4, HTTP CONNECT)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
    /// Optional per-session login shell run instead of the remote default
    /// (e.g. "fish" or "fish -l"). Also drives shell-aware color init.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// ssh_config settings: the files it was imported from, lines set in
    /// Reach, and the commands and weakenings the user allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_options: Option<crate::ssh::sshconf::session::SshOptions>,
    /// A saved credential this session logs in with. Its username, domain
    /// and login win over the session's own, which stay stored as they were
    /// so an older Reach, or a machine the credential is missing on, still
    /// has a login to use. Unset: the folder's credential, if any applies;
    /// "own": the session's own login even then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    /// Pinned to the start screen's Favorites. Kept on the session so it
    /// syncs with it; omitted from disk when off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub favorite: bool,
    /// Where the session stands among the others in its folder (or among
    /// the sessions outside any folder), lowest first; set by moving it in
    /// the list or sorting the folder. Kept on the session so it syncs with
    /// it. Unset: after the ordered ones, as read. Only `session_set_order`
    /// changes it; a move to another folder clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    /// Read only: the saved credential this session's login was filled
    /// from when it was read, if one applies. Never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialUse>,
    /// Free text about the server (RDM's Description and Notes). Part of
    /// the session record, so it is encrypted and synced with the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Named values kept with the session (owner, asset tag, contract...),
    /// in the order the user put them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_fields: Option<Vec<CustomField>>,
    /// The terminal colours this session's tabs use, by terminal theme id
    /// (see src/lib/state/terminal-themes.svelte.ts); unset: the default
    /// chosen in Settings. Kept on the session, so it syncs with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_theme: Option<String>,
    /// The terminal cursor's shape in this session's tabs: `block`,
    /// `underline` or `bar`; unset: the default chosen in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_style: Option<String>,
    /// Whether that cursor blinks; unset: the default chosen in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_blink: Option<bool>,
    /// Server monitoring (CPU, memory, disk, users in the bar under the
    /// terminal) for this session's tabs: on or off; unset: the default
    /// chosen in Settings. Kept on the session, so it syncs with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitoring: Option<bool>,
    /// What this session's tabs record into the session history: `off`,
    /// `commands` or `output`; unset: the default chosen in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<String>,
    /// Start this session's terminals in a tmux session on the server (see
    /// `ssh::tmux`); unset: as its folder says, else not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux: Option<crate::ssh::tmux::TmuxSetting>,
    /// SSH only: start a whole desktop over X11 forwarding, shown in a
    /// window of its own here, instead of a shell (see `ssh::x11_desktop`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x11_desktop: Option<crate::ssh::x11_desktop::X11Desktop>,
    /// SSH only: terminal type, what Backspace sends and the server's
    /// character encoding (see `ssh::terminal`); unset: xterm-256color,
    /// DEL and UTF-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::ssh::terminal::TerminalOptions>,
    /// Approvals of the password manager entries a saved credential brings
    /// to this session, by slot (`password`, `jump:0:passphrase`). An entry
    /// in a credential is approved per session and person like one of the
    /// session's own, but the credential's login is not stored in the
    /// session, so its approvals are kept here. Signed like the others;
    /// see `ipc::saved_credential_commands`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub credential_ref_approvals: std::collections::BTreeMap<String, Vec<String>>,
    /// The server keys this session trusts, one per server it goes through
    /// (see `ssh::session_hostkeys`). Kept on the session so it syncs and is
    /// shared with it. Changed only by connecting (Accept and save) and the
    /// `session_host_key_*` commands; an editor's save keeps the stored ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_keys: Option<Vec<crate::ssh::session_hostkeys::SavedHostKey>>,
    /// Servers whose key was removed from this session, and when, so that
    /// sync does not bring the key back from a device that still has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_keys_removed: Option<Vec<crate::ssh::session_hostkeys::RemovedHostKey>>,
    /// A name for the main address (`host`), shown when the session has
    /// other addresses ("Office").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_label: Option<String>,
    /// Other addresses the same server answers on, by priority after the
    /// main one: an internal and an external one, IPv4 and IPv6, other
    /// names. `host` stays the main address, so older builds, importers and
    /// everything else that reads it keep working. SSH only for now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_addresses: Option<Vec<SessionAddress>>,
    /// How a session with other addresses picks one: `auto` (by priority,
    /// skipping what does not answer), `ask` (a picker on every connect) or
    /// `main` (the main address only). Unset: `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_mode: Option<String>,
    /// Check which addresses answer before connecting (Auto, and the
    /// picker of Ask): on or off; unset: as Settings says. Off: Auto tries
    /// them in order without checking first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_check: Option<bool>,
    /// Passwords this session logged in with before, newest first (see
    /// `ipc::password_history`). Part of the record, so encrypted and
    /// synced with it. Kept only by saving a changed password; never sent
    /// to the window with the session, and what the window sends is ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_history: Option<Vec<PastPassword>>,
    /// Web only: address, storage, sign-in and trusted certificates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<crate::web::WebOptions>,
    /// Wake-on-LAN for the machine: its MAC address and where the magic
    /// packet goes (Wake in the session's menu, Network tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wol: Option<WakeOnLan>,
    /// SSH only: open in the full-pane file manager rather than the
    /// terminal, over `sftp` or `scp` (see `sftp::backend`). Unset: a terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<FilesProtocol>,
    /// FTP only: encryption and the certificates it trusts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ftp: Option<crate::ftp::FtpOptions>,
    /// Server management (BMC) only: the certificates it trusts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmc: Option<crate::bmc::BmcOptions>,
    /// Proxmox only: realm, API token and the certificates it trusts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxmox: Option<crate::proxmox::ProxmoxOptions>,
    /// MQTT only: transport, client ID, topic filters, client certificate
    /// and the certificates it trusts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mqtt: Option<crate::mqtt::MqttOptions>,
}

/// How to wake a session's machine over the network.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WakeOnLan {
    /// Its network card's MAC address, as typed.
    pub mac: String,
    /// Where the packet goes: a broadcast address (10.0.0.255 for another
    /// subnet) or a name; unset: 255.255.255.255.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<String>,
    /// UDP port; unset: 9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// How an SSH session that opens as files reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilesProtocol {
    Sftp,
    /// The scp program for copies, shell commands for the rest: for
    /// servers without the SFTP subsystem.
    Scp,
}

/// A password a session or saved credential used before it was changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PastPassword {
    pub password: String,
    /// When it was replaced, in Unix seconds.
    pub replaced_at: i64,
}

/// One more address of a session's server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionAddress {
    /// What it is, as the picker shows it ("Internet", "IPv6").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// A host name, IPv4 or IPv6 address.
    pub host: String,
    /// Unset: the session's port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Reached straight, without the session's jump hosts or proxy (the
    /// office address of a server reached from outside through a gateway).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub direct: bool,
    /// Reached through another saved SSH session instead: its own jump
    /// hosts, then that session's server as the last hop. Wins over
    /// `direct` and the session's route. Each address can go its own way:
    /// the office one straight, the internet one through a gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_session_id: Option<String>,
    /// Reached through this WireGuard profile's tunnel instead, straight
    /// (without the session's jump hosts or proxy). Wins over the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_wireguard: Option<String>,
}

/// One named value of a session's documentation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomField {
    pub name: String,
    pub value: String,
    /// Shown masked until revealed, like a password. Stored the same way
    /// as every other field: the whole session is encrypted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl SessionConfig {
    /// Notes with nothing in them and fields with neither a name nor a
    /// value are not kept, so an untouched session stores neither.
    pub fn tidy_documentation(&mut self) {
        if self.notes.as_deref().is_some_and(|n| n.trim().is_empty()) {
            self.notes = None;
        }
        if let Some(fields) = self.custom_fields.as_mut() {
            for f in fields.iter_mut() {
                f.name = f.name.trim().to_string();
            }
            fields.retain(|f| !f.name.is_empty() || !f.value.trim().is_empty());
        }
        if self.custom_fields.as_ref().is_some_and(|f| f.is_empty()) {
            self.custom_fields = None;
        }
        if self.terminal_theme.as_deref().is_some_and(|t| t.trim().is_empty()) {
            self.terminal_theme = None;
        }
        if !matches!(self.cursor_style.as_deref(), None | Some("block" | "underline" | "bar")) {
            self.cursor_style = None;
        }
        if !matches!(self.history.as_deref(), None | Some("off" | "commands" | "output")) {
            self.history = None;
        }
        if self.kind != SessionKind::Ssh || self.x11_desktop.as_ref().is_some_and(|d| d.check().is_err()) {
            self.x11_desktop = None;
        }
        self.terminal = self.terminal.take().filter(|_| self.kind == SessionKind::Ssh).and_then(|t| t.tidy());
        if self.kind != SessionKind::Spice || self.spice.as_ref().is_some_and(|o| o.is_empty()) {
            self.spice = None;
        }
        self.tidy_addresses();
        if let Some(w) = self.wol.as_mut() {
            w.mac = w.mac.trim().to_string();
            w.broadcast = w.broadcast.take().map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
            w.port = w.port.filter(|p| *p != 0);
        }
        if self.wol.as_ref().is_some_and(|w| w.mac.is_empty()) {
            self.wol = None;
        }
        if self.kind == SessionKind::Web {
            if let Some(w) = self.web.as_mut() {
                w.tidy();
            }
        } else {
            self.web = None;
        }
    }

    /// Other addresses without a host are dropped, as is a mode nobody
    /// knows; a session left with none stores neither.
    pub fn tidy_addresses(&mut self) {
        if let Some(list) = self.alt_addresses.as_mut() {
            for a in list.iter_mut() {
                // An IPv6 address as typed in a URL: the brackets go.
                a.host = a.host.trim().trim_start_matches('[').trim_end_matches(']').to_string();
                a.label = a.label.trim().to_string();
            }
            let own = self.id.clone();
            for a in list.iter_mut() {
                // Through itself is no way in; an empty choice is none.
                if a.via_session_id.as_deref().is_some_and(|v| v.trim().is_empty() || v == own) {
                    a.via_session_id = None;
                }
                if a.via_wireguard.as_deref().is_some_and(|v| v.trim().is_empty()) {
                    a.via_wireguard = None;
                }
                if a.via_wireguard.is_some() {
                    a.via_session_id = None;
                }
                if a.via_session_id.is_some() || a.via_wireguard.is_some() {
                    a.direct = false;
                }
            }
            list.retain(|a| !a.host.is_empty());
        }
        if self.alt_addresses.as_ref().is_some_and(|l| l.is_empty()) {
            self.alt_addresses = None;
        }
        if self.wireguard_id.as_deref().is_some_and(|v| v.trim().is_empty()) {
            self.wireguard_id = None;
        }
        if self.host_label.as_deref().is_some_and(|l| l.trim().is_empty()) {
            self.host_label = None;
        }
        if !matches!(self.connect_mode.as_deref(), None | Some("auto" | "ask" | "main")) || self.alt_addresses.is_none() {
            self.connect_mode = None;
        }
        if self.alt_addresses.is_none() {
            self.address_check = None;
        }
    }

    /// Every `host:port` the session's server is reached on: the main
    /// address first, then the others, as host keys are kept under.
    pub fn address_ids(&self) -> Vec<String> {
        let mut ids = vec![format!("{}:{}", self.host, self.port)];
        for a in self.alt_addresses.iter().flatten() {
            let id = format!("{}:{}", a.host, a.port.unwrap_or(self.port));
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    /// The other address `host:port` is, if it is one of this session's
    /// (`None` for the main address, or one it does not have).
    pub fn alt_address(&self, host: &str, port: u16) -> Option<&SessionAddress> {
        self.alt_addresses.iter().flatten().find(|a| a.host == host && a.port.unwrap_or(self.port) == port)
    }
}

/// A login kept once and used by any number of sessions in the same vault:
/// a session links it by id, or a folder does for the sessions inside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCredential {
    pub id: String,
    pub name: String,
    pub username: String,
    /// Windows logon domain, used by RDP sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub auth_method: AuthMethod,
    /// The vault it is kept in, as for sessions (None = this device's
    /// private sessions vault). Only sessions in the same vault can use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<String>,
    /// Passwords it held before, newest first; as for sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_history: Option<Vec<PastPassword>>,
    /// The credential folder it is filed in, in the same vault. None, or a
    /// folder that is gone or in another vault: the top of the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
}

/// A folder in the credentials list, only for keeping credentials in order.
/// It lives in the vault of the credentials it holds (this device's own
/// sessions vault, or a user vault, where it syncs with them). It is a
/// record of its own (`custom:credential_folder`), so an older Reach never
/// reads it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CredentialFolder {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// As for credentials: None for this device's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<String>,
    /// Icon and colour, as for session folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Which saved credential a session's login came from, as read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CredentialUse {
    pub id: String,
    /// The credential's name; empty when it is missing.
    pub name: String,
    /// The folder it was inherited from; None when the session links it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// The credential is not in the session's vault (deleted, or not synced
    /// yet): the session's own stored login was used.
    #[serde(default)]
    pub missing: bool,
    /// Set when the session or its folder says "each person's own login"
    /// (`credential_id` = `personal`): the session or folder id the reading
    /// person's choice is kept under, in their own vault. `missing` then
    /// means they have not chosen one yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_target: Option<String>,
    /// With `personal_target`: the session's own user name, which the
    /// reader's choice may have replaced. The editor shows and keeps this
    /// one, so saving never writes someone's own user name to the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_username: Option<String>,
}

/// Proxy configuration for SSH connections (Tor, SOCKS5, HTTP CONNECT).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Proxy type: "socks5", "socks4", "http"
    pub proxy_type: String,
    /// Proxy host (e.g. "127.0.0.1" for Tor)
    pub host: String,
    /// Proxy port (e.g. 9050 for Tor)
    pub port: u16,
    /// Optional proxy username
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Optional proxy password
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The proxy password from a password manager, fetched at connect time.
    /// Wins over `password` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_ref: Option<crate::pwmanager::SecretRef>,
}

/// Authentication method for an SSH session.
/// Credentials (password, passphrase, key_content) are stored encrypted in the vault.
/// A `*_ref` points at a password manager entry instead; it is resolved at
/// connect time and wins over the plaintext field (see `crate::pwmanager`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthMethod {
    Password {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password_ref: Option<crate::pwmanager::SecretRef>,
    },
    Key {
        /// A key file on this machine. Set when the user browsed to one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passphrase: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_content: Option<String>, // Embedded key for sharing
        /// A key imported into the vault, referenced by id. Set instead of
        /// `path` when the user picked an imported key, so the session works
        /// on a machine that has no copy of the key file (issue #46).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_id: Option<String>,
        /// The key's passphrase from a password manager.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passphrase_ref: Option<crate::pwmanager::SecretRef>,
    },
    Agent,
}

/// A saved command snippet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A folder used to organize sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<String>,
    /// A saved credential the sessions inside (at any depth) log in with,
    /// unless a session links its own or a nearer folder names another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    /// Start the terminals of the sessions inside (at any depth) in tmux,
    /// unless a session or a nearer folder says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux: Option<bool>,
    /// The icon shown for the folder in the session list: `fa:<name>` for
    /// one of the app's own symbols, `os:<id>` for an OS or vendor logo.
    /// `None`: the plain folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The colour of that icon, by name (`red`, `blue`…). `None`: the
    /// theme's folder colour, or a logo's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Configuration for a port-forwarding tunnel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub id: String,
    pub tunnel_type: TunnelType,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub session_id: String,
    pub status: TunnelStatus,
}

/// The type of SSH tunnel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TunnelType {
    Local,
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStatus {
    Stopped,
    Starting,
    Running,
}

/// System statistics collected from a remote host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStats {
    pub cpu: f64,
    pub ram: f64,
    pub ram_total: u64,
    pub ram_used: u64,
    pub disk: f64,
    pub users: Vec<String>,
    /// Network upload bytes per second
    #[serde(default)]
    pub net_up: u64,
    /// Network download bytes per second
    #[serde(default)]
    pub net_down: u64,
}

/// Main application state.
///
/// All collections are behind `Arc<RwLock<_>>` for safe concurrent access
/// from multiple Tauri command handlers.
///
/// Note: Sessions, folders, credentials, and settings are all
/// stored encrypted in the vault (SQLite + XChaCha20-Poly1305).
pub struct AppState {
    pub ssh_manager: Arc<tokio::sync::Mutex<SshManager>>,
    pub monitoring: Arc<RwLock<HashMap<String, SystemStats>>>,
    #[cfg(desktop)]
    pub pty_manager: Arc<Mutex<PtyManager>>,
    pub monitoring_collector: Arc<tokio::sync::Mutex<MonitoringCollector>>,
    pub tunnel_manager: Arc<tokio::sync::Mutex<TunnelManager>>,
    #[cfg(desktop)]
    pub serial_manager: Arc<tokio::sync::Mutex<SerialManager>>,
    pub rdp_manager: Arc<tokio::sync::Mutex<RdpManager>>,
    pub telnet_manager: Arc<tokio::sync::Mutex<crate::telnet::TelnetManager>>,
    /// Open FTP sessions; their ids are answered by the `sftp_*` commands.
    pub ftp_manager: Arc<crate::ftp::FtpManager>,
    /// Open server management (Redfish) sessions.
    pub bmc_manager: Arc<crate::bmc::BmcManager>,
    /// Open Proxmox sessions.
    pub proxmox_manager: Arc<crate::proxmox::ProxmoxManager>,
    /// Open MQTT sessions.
    pub mqtt_manager: Arc<crate::mqtt::MqttManager>,
    pub vnc_manager: Arc<tokio::sync::Mutex<VncManager>>,
    pub spice_manager: Arc<tokio::sync::Mutex<SpiceManager>>,
    pub vault_manager: Arc<tokio::sync::Mutex<VaultManager>>,
    pub plugin_manager: Arc<tokio::sync::Mutex<PluginManager>>,
    /// URL of the marketplace registry JSON file.
    pub marketplace_index_url: Arc<RwLock<String>>,
    pub tofu_schema_cache: Arc<tokio::sync::Mutex<SchemaCache>>,
    /// Saved and open database connections.
    pub db: Arc<tokio::sync::Mutex<crate::db::DbManager>>,
    /// Open container engines (Docker, Podman) and their log streams.
    pub containers: Arc<tokio::sync::Mutex<crate::container::ContainerManager>>,
    /// Open Kubernetes clusters and their log streams.
    pub k8s: Arc<tokio::sync::Mutex<crate::k8s::K8sManager>>,
    /// Saved container hosts and clusters, from the `__devops__` vault.
    pub devops_store: Arc<tokio::sync::Mutex<crate::devops_store::DevopsStore>>,
    pub close_to_tray: AtomicBool,
    /// Pending file for the editor window to pick up on mount
    pub pending_editor_file: Arc<tokio::sync::Mutex<Option<serde_json::Value>>>,
    /// MCP server state. Constructed disabled, with no token and no shared
    /// sessions: the listener does not exist until the user asks for it.
    pub mcp: Arc<McpState>,
    /// Peer-to-peer sharing configuration. Off by default; the webview
    /// loads none of the sharing code until this says otherwise.
    pub share: Arc<RwLock<crate::share::ShareConfig>>,
    /// Uploads and downloads in flight, by transfer id: whose connection
    /// they run on and how to stop them.
    pub transfers: Arc<std::sync::Mutex<HashMap<String, (String, crate::sftp::transfer::CancellationToken)>>>,
}

impl AppState {
    pub fn new() -> Self {
        // Writable app data dir, resolved from Tauri's path API at startup.
        // NOT `dirs::data_dir()` directly: on Android/iOS the OS sandbox isn't
        // an XDG/known dir, so `dirs` yields a read-only path and vault writes
        // fail with "Read-only file system (os error 30)". See crate::app_data_dir.
        let app_dir = crate::app_data_dir();

        Self {
            ssh_manager: Arc::new(tokio::sync::Mutex::new(SshManager::new())),
            monitoring: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(desktop)]
            pty_manager: Arc::new(Mutex::new(PtyManager::new())),
            monitoring_collector: Arc::new(tokio::sync::Mutex::new(MonitoringCollector::new())),
            tunnel_manager: Arc::new(tokio::sync::Mutex::new(TunnelManager::new())),
            #[cfg(desktop)]
            serial_manager: Arc::new(tokio::sync::Mutex::new(SerialManager::new())),
            rdp_manager: Arc::new(tokio::sync::Mutex::new(RdpManager::new())),
            telnet_manager: Arc::new(tokio::sync::Mutex::new(crate::telnet::TelnetManager::new())),
            ftp_manager: Arc::new(crate::ftp::FtpManager::new()),
            bmc_manager: Arc::new(crate::bmc::BmcManager::new()),
            proxmox_manager: Arc::new(crate::proxmox::ProxmoxManager::new()),
            mqtt_manager: Arc::new(crate::mqtt::MqttManager::new()),
            vnc_manager: Arc::new(tokio::sync::Mutex::new(VncManager::new())),
            spice_manager: Arc::new(tokio::sync::Mutex::new(SpiceManager::new())),
            vault_manager: Arc::new(tokio::sync::Mutex::new(VaultManager::new(app_dir.clone()))),
            plugin_manager: Arc::new(tokio::sync::Mutex::new(PluginManager::new(app_dir.join("plugins")))),
            marketplace_index_url: Arc::new(RwLock::new(
                crate::plugin::marketplace::DEFAULT_INDEX_URL.to_string(),
            )),
            tofu_schema_cache: Arc::new(tokio::sync::Mutex::new(SchemaCache::default())),
            containers: Arc::new(tokio::sync::Mutex::new(crate::container::ContainerManager::default())),
            k8s: Arc::new(tokio::sync::Mutex::new(crate::k8s::K8sManager::default())),
            devops_store: Arc::new(tokio::sync::Mutex::new(crate::devops_store::DevopsStore::default())),
            db: Arc::new(tokio::sync::Mutex::new(crate::db::DbManager::new())),
            close_to_tray: AtomicBool::new(false),
            pending_editor_file: Arc::new(tokio::sync::Mutex::new(None)),
            mcp: Arc::new(McpState::default()),
            share: Arc::new(RwLock::new(crate::share::ShareConfig::default())),
            transfers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
