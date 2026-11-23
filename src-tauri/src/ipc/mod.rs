pub mod webview_commands;
pub mod dragout_commands;
pub mod ai_commands;
pub mod plugin_commands;
pub mod marketplace_commands;
pub mod credential_commands;
pub mod settings_commands;
pub mod history_commands;
pub mod font_commands;
pub mod mcp_commands;
pub mod monitoring_commands;
#[cfg(desktop)]
pub mod pty_commands;
#[cfg(desktop)]
pub mod serial_commands;
#[cfg(desktop)]
pub mod freerdp_commands;
pub mod rdp_commands;
pub mod telnet_commands;
pub mod zmodem_commands;
pub mod vnc_commands;
pub mod spice_commands;
pub mod saved_credential_commands;
pub mod credential_folder_commands;
pub(crate) mod password_history;
pub mod session_commands;
pub mod sftp_commands;
#[cfg(desktop)]
pub mod local_fs_commands;
#[cfg(desktop)]
pub mod editor_detect;
#[cfg(desktop)]
pub mod winbox_commands;
pub mod ssh_commands;
pub mod sshconfig_commands;
pub mod import_commands;
pub mod x11server_commands;
pub mod sshkey_commands;
pub mod ansible_commands;
pub mod container_commands;
pub mod db_commands;
pub mod k8s_commands;
pub mod tofu_commands;
pub mod theme_commands;
pub mod update_commands;
pub mod toolchain_commands;
pub mod tunnel_commands;
pub mod vault_commands;
pub mod editor_commands;
#[cfg(desktop)]
pub mod window_commands;
pub mod snippet_commands;
pub mod recipe_commands;
pub mod registry_url_commands;
pub mod share_commands;
pub mod pwmanager_commands;
