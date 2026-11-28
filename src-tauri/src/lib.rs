pub mod ansible;
pub mod bmc;
pub mod proxmox;
pub mod mqtt;
pub mod plugin;
pub mod pwmanager;
pub mod ipc;
pub mod mcp;
pub mod monitoring;
pub mod net;
#[cfg(desktop)]
pub mod pty;
#[cfg(desktop)]
pub mod localserver;
pub mod rdp;
pub mod vnc;
pub mod spice;
pub mod container;
pub mod db;
pub mod devops;
mod devops_projects;
pub mod devops_store;
pub mod k8s;
pub mod logfiles;
mod local_paths;
pub mod dragout;
pub mod hardening;
pub mod history;
pub mod system_lock;
pub mod http;
pub mod recipe;
pub mod report;
pub mod share;
pub mod sound;
pub mod serial;
pub mod session;
pub mod sftp;
pub mod ftp;
pub mod ssh;
pub mod state;
pub mod text;
pub mod theme;
pub mod telnet;
pub mod tofu;
#[cfg(desktop)]
pub mod tray;
pub mod toolchain;
pub mod tunnel;
pub mod vault;
pub mod web;
pub mod wireguard;
pub mod wsl;
pub mod zmodem;

use std::sync::atomic::Ordering;

use state::AppState;
use tracing_subscriber::EnvFilter;

use ipc::ansible_commands::*;
use ipc::db_commands::*;
use ipc::ai_commands::*;
use ipc::update_commands::*;
use ipc::container_commands::*;
use ipc::k8s_commands::*;
use ipc::plugin_commands::*;
use ipc::marketplace_commands::*;
use ipc::registry_url_commands::*;
use ipc::credential_commands::*;
use ipc::settings_commands::*;
use ipc::sync_account_commands::*;
use ipc::history_commands::*;
use ipc::font_commands::*;
use ipc::mcp_commands::*;
#[cfg(desktop)]
use ipc::freerdp_commands::*;
use ipc::rdp_commands::*;
use ipc::vnc_commands::*;
use ipc::spice_commands::*;
use ipc::monitoring_commands::*;
#[cfg(desktop)]
use ipc::pty_commands::*;
#[cfg(desktop)]
use ipc::serial_commands::*;
#[cfg(desktop)]
use ipc::winbox_commands::*;
#[cfg(desktop)]
use ipc::screenshot_commands::*;
#[cfg(desktop)]
use ipc::local_server_commands::*;
use ipc::telnet_commands::*;
use ipc::zmodem_commands::*;
use ipc::saved_credential_commands::*;
use ipc::credential_folder_commands::*;
use ipc::personal_login_commands::*;
use ipc::session_commands::*;
use ipc::sftp_commands::*;
use ipc::local_fs_commands::*;
#[cfg(desktop)]
use ipc::editor_detect::*;
use ipc::dragout_commands::*;
use ipc::webview_commands::*;
use ipc::ssh_commands::*;
use ipc::sshconfig_commands::*;
use ipc::import_commands::*;
use ipc::x11server_commands::*;
use ipc::sshkey_commands::*;
use ipc::tofu_commands::*;
use ipc::theme_commands::*;
use ipc::toolchain_commands::*;
use ipc::tunnel_commands::*;
use ipc::vault_commands::*;
use ipc::editor_commands::*;
#[cfg(desktop)]
use ipc::window_commands::*;
use ipc::snippet_commands::*;
use ipc::wireguard_commands::*;
use ipc::recipe_commands::*;
use ipc::share_commands::*;
use ipc::pwmanager_commands::*;

use std::path::PathBuf;
use std::sync::OnceLock;

/// Process-wide writable app data directory, resolved once at startup from
/// Tauri's path API. Using a global (instead of `dirs::data_dir()`) is what
/// makes mobile work: on Android/iOS the OS sandbox isn't an XDG/known dir, so
/// `dirs` returns a non-writable path and every file write fails with
/// "Read-only file system (os error 30)". `AppState` and the SSH known-hosts
/// store read from this instead.
static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The product name the fork shows (Reacher CM, a fork of Reach). Identifiers
/// that existing data depends on (the app id, keychain services, vault file
/// names) keep their Reach names on purpose.
pub const APP_NAME: &str = "Reacher CM";
/// Sent as the User-Agent on downloads from GitHub (VcXsrv, OpenTofu).
pub const USER_AGENT: &str = "ReacherCM (https://github.com/rsipka/ReacherCM)";

/// Reacher CM's own app id and desktop data folder name, so it installs and
/// runs next to Reach (`com.reach.desktop`, data in `com.reach.app`) without
/// sharing its data, keychain entries or single-instance lock.
pub const APP_ID: &str = "com.reachercm.client";

/// The app, for code with no handle of its own that needs a mobile plugin:
/// the keychain functions, which on Android go through the Keystore.
#[cfg(target_os = "android")]
static ANDROID_APP: OnceLock<tauri::AppHandle> = OnceLock::new();

#[cfg(target_os = "android")]
pub fn android_app() -> Option<&'static tauri::AppHandle> {
    ANDROID_APP.get()
}

/// Record the resolved app data directory. Call once, early in `setup()`.
pub fn set_app_data_dir(dir: PathBuf) {
    let _ = APP_DATA_DIR.set(dir);
}

/// The writable app data directory. Falls back to the desktop `dirs` location
/// when not yet set (tests / very early startup before `setup()` runs).
pub fn app_data_dir() -> PathBuf {
    APP_DATA_DIR.get().cloned().unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_ID)
    })
}

/// A folder for short-lived files: copies fetched to be dragged out, files
/// pasted from a remote desktop. It sits under the app's own data folder and
/// is owner-only, not in the system temp folder, which on Linux every user of
/// the machine shares: there another user could read the files, or create the
/// folder first and plant links in it.
pub fn private_tmp_dir(name: &str) -> std::io::Result<PathBuf> {
    let base = app_data_dir().join("tmp");
    vault::private_fs::private_dir(&base)?;
    let dir = base.join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[tauri::command]
fn set_close_to_tray(state: tauri::State<'_, AppState>, enabled: bool) {
    state.close_to_tray.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn get_close_to_tray(state: tauri::State<'_, AppState>) -> bool {
    state.close_to_tray.load(Ordering::Relaxed)
}

/// Exit the whole app. The frontend calls this after confirming there are no
/// active SSH sessions (or the user accepted terminating them) — see the tray
/// "Quit" → `app-quit-requested` flow in `AppShell.svelte`.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    // A password manager passphrase kept in memory goes before the process
    // does (it is encrypted there, but there is no reason to leave it).
    pwmanager::forget_passphrase();
    ssh::unlocked::forget();
    app.exit(0);
}

/// Turn WebKitGTK's WebRTC setting on for the main window. Its default is
/// off, which removes `RTCPeerConnection` from the page altogether. Where the
/// system's GStreamer lacks the WebRTC elements the setting stays inert and
/// the share dialog says what to install.
#[cfg(target_os = "linux")]
fn enable_webrtc(app: &tauri::App) {
    use tauri::Manager;
    use webkit2gtk::{SettingsExt, WebViewExt};
    for (_, window) in app.webview_windows() {
        let _ = window.with_webview(|webview| {
            if let Some(settings) = webview.inner().settings() {
                settings.set_enable_webrtc(true);
            }
        });
    }
}

/// Let a held key repeat instead of offering accented characters.
///
/// macOS ships `ApplePressAndHoldEnabled` on, and WebKit honours it: hold a key
/// over an editable element — which is what xterm.js focuses — and AppKit opens
/// the accent picker rather than sending key repeats. In a terminal that means
/// holding an arrow, Backspace or any character does *nothing at all*, which is
/// exactly how it was reported (issue #47).
///
/// It has to be done here, at runtime. `ApplePressAndHoldEnabled` is read from
/// `NSUserDefaults`, and `Info.plist` is not one of its domains, so the key that
/// several projects put in their plist is never read by anything. Registering it
/// puts the value in the *registration* domain: the lowest priority there is, so
/// a user who has deliberately set the key either way still wins, and nothing is
/// written to their preferences file.
#[cfg(target_os = "macos")]
fn disable_press_and_hold() {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use std::ffi::c_char;

    // SAFETY: plain Foundation constructors and one setter, each null-checked
    // before it is used. Nothing here is kept past the call.
    unsafe {
        let name = c"ApplePressAndHoldEnabled".as_ptr() as *const c_char;
        let key: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: name];
        let value: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: Bool::NO];
        if key.is_null() || value.is_null() {
            return;
        }
        let dict: *mut AnyObject =
            msg_send![class!(NSDictionary), dictionaryWithObject: value, forKey: key];
        if dict.is_null() {
            return;
        }
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults.is_null() {
            return;
        }
        let _: () = msg_send![defaults, registerDefaults: dict];
    }
}
/// Log to the console as before, and on desktop also to
/// `<app data>/logs/reach.log`, so a user who hits a problem has something to
/// send (issue #77 came with a screenshot and nothing else). The file starts
/// over past 5 MB, keeping the one before as `reach.1.log`.
fn init_logging() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let file = log_file().map(|f| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
    });
    use tracing_subscriber::Layer;
    // Each output has its own filter, so a connection's LogLevel can see
    // debug messages that the console and the file leave out.
    let file_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .with(file.map(|f| f.with_filter(file_filter)))
        .with(crate::ssh::connlog::ConnLogLayer.with_filter(crate::ssh::connlog::ConnLogFilter))
        .init();

    // A panic is printed to stderr, which nobody sees when Reach is started
    // from a menu: a crash left nothing in the log (issue #87). Put it there.
    let print = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!("Panic on thread {}: {info}", thread.name().unwrap_or("unnamed"));
        print(info);
    }));
}

#[cfg(desktop)]
fn log_file() -> Option<std::fs::File> {
    const LIMIT: u64 = 5 * 1024 * 1024;
    let dir = app_data_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    purge_logs_with_secrets(&dir);
    let path = dir.join("reach.log");
    if std::fs::metadata(&path).map(|m| m.len() > LIMIT).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("reach.1.log"));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Up to 0.8.0, creating or editing a session wrote its login (password, key
/// passphrase, embedded key, proxy password) into reach.log, and opening a
/// synced vault its token. Those logs are deleted once, before anything new
/// is written; the marker keeps later starts from deleting new logs.
#[cfg(desktop)]
fn purge_logs_with_secrets(dir: &std::path::Path) {
    let marker = dir.join(".secrets-purged");
    if marker.exists() {
        return;
    }
    let mut cleared = true;
    for name in ["reach.log", "reach.1.log"] {
        let path = dir.join(name);
        if path.exists() && std::fs::remove_file(&path).is_err() {
            cleared = false;
        }
    }
    // Only marked done when both are gone, so a file that was locked is
    // tried again next start.
    if cleared {
        let _ = std::fs::write(&marker, b"");
    }
}

/// iOS keeps its trusted certificate authorities in the keychain, where
/// rustls-native-certs (RDP's TLS upgrade, the database drivers) does not
/// look: on iOS it reads files only, finds none, and every connection that
/// asks for the platform's roots fails with "no usable roots". It honours
/// SSL_CERT_FILE, so write Mozilla's roots to one in the app's own temporary
/// folder and point it there. Android's equivalent is SSL_CERT_DIR in run().
#[cfg(target_os = "ios")]
fn ios_cert_file() {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    if std::env::var_os("SSL_CERT_FILE").is_some() {
        return;
    }
    let mut pem = String::new();
    for cert in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        pem.push_str("-----BEGIN CERTIFICATE-----\n");
        let b64 = BASE64.encode(cert.as_ref());
        for line in b64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap_or_default());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
    }
    let path = std::env::temp_dir().join("reach-roots.pem");
    match std::fs::write(&path, pem) {
        Ok(()) => std::env::set_var("SSL_CERT_FILE", &path),
        Err(e) => tracing::warn!("Could not write the root certificates for TLS: {e}"),
    }
}

/// Mobile platforms keep their own system log (logcat on Android).
#[cfg(mobile)]
fn log_file() -> Option<std::fs::File> {
    None
}

/// Build and run the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // A development build can be pointed at a data folder of its own, so
    // testing never runs against the user's real vault. First, before the
    // log file opens. Compiled out of release builds: what an installed
    // Reach opens is never decided by an environment variable.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("REACH_DEV_DATA_DIR").filter(|d| !d.is_empty()) {
        set_app_data_dir(PathBuf::from(dir));
    }
    init_logging();

    // TLS for HTTPS (reqwest) and every other rustls user: ring, which the
    // database drivers already build in. reqwest is compiled without a
    // provider of its own and asks for this one at its first request.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Before any secret is in memory: keep other programs out of it.
    hardening::protect_process();

    // Android keeps its trusted certificate authorities in a directory, not in
    // a file or store rustls-native-certs knows, so it found none and every
    // TLS user that asks for the platform's roots failed there: RDP's TLS
    // upgrade stopped with "the platform certificate store contains no usable
    // roots". That crate honours SSL_CERT_DIR, so point it at Android's own
    // (Android 14 moved it into the Conscrypt APEX).
    #[cfg(target_os = "android")]
    if std::env::var_os("SSL_CERT_DIR").is_none() {
        let dir = ["/apex/com.android.conscrypt/cacerts", "/system/etc/security/cacerts"]
            .into_iter()
            .find(|d| std::path::Path::new(d).is_dir());
        if let Some(dir) = dir {
            std::env::set_var("SSL_CERT_DIR", dir);
            tracing::info!("Trusted certificates from {}", dir);
        }
    }

    #[cfg(target_os = "ios")]
    ios_cert_file();

    tracing::info!("Starting Reach application");

    // Work around WebKitGTK DMA-BUF protocol errors on Wayland (KDE Plasma, etc.)
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
            tracing::info!("Set WEBKIT_DISABLE_DMABUF_RENDERER=1 for Wayland compatibility");
        }
    }

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_reach_unlock::init());
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    }
    // NOTE: AppState is managed inside `setup()` (not here) so it can be rooted
    // at the Tauri-resolved, writable app data dir — required on Android/iOS.

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                use tauri::Manager;
                // When a second instance is launched, focus the existing window
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }))
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                None,
            ))
            // Dragging a file out of the window onto the desktop. The OS never
            // reports where a drag was dropped, so this can only hand it a
            // file that already exists locally — see src/lib/explorer/drag-out.
            .plugin(tauri_plugin_drag::init());
    }

    #[cfg(desktop)]
    {
        builder = builder.invoke_handler(refuse_web_pages(tauri::generate_handler![
            vault_enable_account_sync,
            vault_account_sync_enabled,
            vault_organizations,
            vault_organization_list,
            vault_organization_create,
            vault_organization_open,
            vault_organization_grants,
            vault_organization_grant,
            vault_organization_finish_rotation,
            sync_account_status,
            sync_account_sign_in,
            sync_account_cancel,
            sync_account_check,
            sync_account_sign_out,
            sync_account_manage,
            sync_account_preferences,
            sync_account_managed_removal,
            // MCP server. Off until the user enables it; nothing here starts
            // a listener or shares a session on its own.
            mcp_start,
            telnet_connect,
            telnet_send,
            telnet_resize,
            telnet_disconnect,
            // ZMODEM in SSH, Telnet and serial sessions
            zmodem_upload,
            zmodem_download,
            zmodem_cancel,
            ssh_zmodem_exec,
            rdp_connect,
            rdp_disconnect,
            rdp_mouse,
            rdp_key,
            rdp_unicode,
            rdp_sync_locks,
            rdp_local_numlock,
            rdp_resize,
            rdp_rail_windows,
            rdp_rail_command,
            rdp_view_attach,
            rdp_view_detach,
            rdp_view_ack,
            rdp_ack,
            rdp_disconnect_all,
            rdp_clipboard_sync,
            rdp_clipboard_send_files,
            rdp_clipboard_fetch,
            rdp_clipboard_cancel,
            rdp_clipboard_pick_files,
            rdp_clipboard_save_received,
            rdp_window_fullscreen,
            vnc_connect,
            vnc_disconnect,
            vnc_disconnect_all,
            vnc_ack,
            vnc_security,
            vnc_mouse,
            vnc_key,
            vnc_resize,
            vnc_clipboard_sync,
            spice_connect,
            spice_disconnect,
            spice_disconnect_all,
            spice_ack,
            spice_mouse,
            spice_key,
            spice_type,
            spice_resize,
            spice_clipboard_sync,
            spice_read_vv,
            mcp_stop,
            mcp_status,
            mcp_restore,
            mcp_share_session,
            mcp_unshare_session,
            mcp_list_agents,
            mcp_set_agent,
            mcp_set_mode,
            mcp_set_session_agent,
            mcp_set_session_mode,
            mcp_regenerate_token,
            mcp_confirm_response,
            // SSH commands
            ssh_connect,
            crate::ipc::ssh_commands::ssh_probe_addresses,
            crate::ipc::net_commands::net_resolve,
            crate::ipc::net_commands::net_ping,
            crate::ipc::net_commands::net_scan_ports,
            crate::ipc::net_commands::net_scan_cancel,
            crate::ipc::net_commands::net_wake,
            ssh_disconnect,
            ssh_files_scp,
            ssh_send,
            ssh_ready,
            ssh_hostkey_response,
            ssh_auth_prompt_response,
            session_log_default_dir,
            session_log_folder_exists,
            session_log_status,
            session_log_pause,
            session_log_stop,
            history_start,
            history_stop,
            history_pause,
            history_status,
            history_search,
            history_transcript,
            history_forget,
            history_configure,
            history_stats,
            app_update_check,
            ctr_open,
            ctr_open_live,
            ctr_hosts,
            ctr_host_save,
            ctr_host_delete,
            ctr_close,
            ctr_containers,
            ctr_images,
            ctr_volumes,
            ctr_networks,
            ctr_projects,
            ctr_act,
            ctr_remove,
            ctr_inspect,
            ctr_compose,
            ctr_logs,
            ctr_logs_stop,
            ctr_log_files,
            ctr_logs_files,
            devops_log_rules,
            devops_log_rules_save,
            ctr_shell_command,
            k8s_contexts,
            k8s_clusters,
            k8s_cluster_save,
            k8s_cluster_delete,
            k8s_open,
            k8s_close,
            k8s_namespaces,
            k8s_pods,
            k8s_workloads,
            k8s_objects,
            k8s_events,
            k8s_get_yaml,
            k8s_replace_yaml,
            k8s_delete,
            k8s_scale,
            k8s_restart,
            k8s_logs,
            k8s_logs_stop,
            k8s_log_files,
            k8s_logs_files,
            k8s_helm_releases,
            k8s_helm_history,
            k8s_helm_detail,
            k8s_helm_run,
            ssh_resize,
            ssh_list_connections,
            ssh_connection_algorithms,
            ssh_detect_os,
            ssh_tmux_kill,
            ssh_tmux_chosen,
            tmux_start_script,
            ssh_tmux_list,
            ssh_tmux_windows,
            ssh_tmux_window,
            inspect_key_file,
            // SSH Config commands
            sshconfig_scan,
            x11server_status,
            x11server_install,
            x11server_cancel,
            x11server_remove,
            sshconfig_exists,
            import_read_local,
            import_yaml_to_json,
            ssh_options_report,
            ssh_algorithm_catalog,
            ssh_key_import,
            ssh_key_list,
            ssh_key_update,
            ssh_key_delete,
            ssh_key_public,
            // SFTP commands
            sftp_list_dir,
            sftp_upload,
            sftp_download,
            sftp_delete,
            sftp_rename,
            sftp_mkdir,
            sftp_touch,
            sftp_read_file,
            sftp_write_file,
            sftp_archive_tools,
            sftp_archive_create,
            sftp_archive_extract,
            sftp_archive_cancel,
            sftp_transfer_cancel,
            sftp_terminal_cwd,
            sftp_home,
            ftp_connect,
            ftp_disconnect,
            ftp_session_cert_trust,
            vnc_session_cert_trust,
            bmc_connect,
            bmc_disconnect,
            bmc_session_cert_trust,
            bmc_session_vnc_save,
            bmc_overview,
            bmc_sensors,
            bmc_hardware,
            bmc_log_services,
            bmc_log_entries,
            bmc_log_clear,
            bmc_jobs,
            bmc_virtual_media,
            bmc_power,
            bmc_boot_once,
            bmc_identify,
            bmc_media_insert,
            bmc_media_eject,
            bmc_reset_bmc,
            proxmox_connect,
            proxmox_console_ready,
            proxmox_console_login,
            proxmox_disconnect,
            proxmox_session_cert_trust,
            proxmox_overview,
            proxmox_resources,
            proxmox_guest,
            proxmox_guest_action,
            proxmox_snapshot_create,
            proxmox_snapshot_rollback,
            proxmox_snapshot_delete,
            proxmox_node_action,
            proxmox_tasks,
            proxmox_task_log,
            proxmox_task_stop,
            proxmox_pbs_overview,
            proxmox_pbs_namespaces,
            proxmox_pbs_groups,
            proxmox_pbs_snapshots,
            proxmox_pbs_gc,
            proxmox_pbs_verify,
            proxmox_pbs_protect,
            proxmox_pbs_forget,
            mqtt_connect,
            mqtt_disconnect,
            mqtt_publish,
            mqtt_subscribe,
            mqtt_unsubscribe,
            mqtt_session_cert_trust,
            // This computer's files, for the full-pane file manager
            local_list_dir,
            local_mkdir,
            local_prepare_download_dir,
            local_rename,
            local_delete,
            local_join,
            local_roots,
            local_read_text,
            local_stat,
            local_edit_copy_path,
            local_open,
            local_touch,
            local_reveal,
            local_detect_editors,
            local_open_with,
            drag_preview_icon,
            drag_cache_dir,
            dragout_start,
            webview_refocus,
            report::report_prepare,
            report::report_reveal,
            sound::play_notification_sound,
            sound::play_bell_sound,
            sound::set_audio_muted,
            // Session commands
            session_list,
            session_get,
            session_create,
            wsl_list_distros,
            session_update,
            session_untrusted_secret_refs,
            session_approve_secret_refs,
            session_delete,
            session_move_to_folder,
            session_set_favorite,
            session_host_key_remove,
            session_host_key_replace,
            host_keys_remove_all,
            session_password_history,
            session_password_history_clear,
            saved_credential_password_history,
            saved_credential_password_history_clear,
            password_history_clear_all,
            session_set_order,
            session_bulk_update,
            session_bulk_undo,
            session_list_folders,
            session_create_folder,
            session_set_folder_credential,
            session_set_folder_tmux,
            session_set_folder_icon,
            session_rename_folder,
            saved_credential_list,
            personal_login_list,
            personal_login_set,
            credential_keys_unlock,
            credential_keys_unlock_enabled,
            credential_keys_biometric_forget,
            saved_credential_create,
            saved_credential_update,
            saved_credential_delete,
            saved_credential_move,
            credential_folder_list,
            credential_folder_create,
            credential_folder_rename,
            credential_folder_move,
            credential_folder_set_icon,
            credential_folder_delete,
            session_update_folder_parent,
            session_delete_folder,
            session_share,
            // Tunnel commands
            tunnel_create,
            tunnel_start,
            tunnel_stop,
            tunnel_remove,
            tunnel_list,
            // Local TFTP, FTP, SFTP and HTTP servers
            local_server_interfaces,
            local_server_check_port,
            local_server_start,
            local_server_stop,
            local_server_status,
            local_server_clear_log,
            // PTY commands
            pty_spawn,
            pty_write,
            pty_resize,
            pty_close,
            // Serial commands
            serial_list_ports,
            serial_connect,
            serial_send,
            serial_break,
            serial_disconnect,
            // FreeRDP, started as its own program
            freerdp_find,
            freerdp_launch,
            // WinBox 4, started as its own program
            winbox_find,
            winbox_launch,
            // A picture of a session pane, to a file or the clipboard
            screenshot_save,
            screenshot_copy,
            // Monitoring commands
            monitoring_start,
            monitoring_start_wsl,
            monitoring_stop,
            monitoring_get_stats,
            // AI commands
            ai_chat,
            ai_fetch_models,
            // Credential commands
            credential_set_master_password,
            credential_verify_master_password,
            credential_is_locked,
            credential_lock,
            credential_has_master_password,
            // Settings commands
            settings_get_all,
            settings_get,
            settings_set,
            settings_delete,
            settings_save_all,
            fonts_list_installed,
            // Vault commands
            vault_init_identity,
            vault_unlock,
            vault_auto_unlock,
            vault_reset,
            vault_export_identity,
            vault_import_identity,
            vault_export_device_link,
            vault_link_device,
            vault_local_link_recovery_available,
            vault_restore_local_before_link,
            qr_svg,
            vault_lock,
            vault_is_held,
            vault_resume,
            vault_unlock_methods,
            vault_biometric_enable,
            vault_biometric_unlock,
            vault_security_key_add,
            vault_security_key_unlock,
            vault_unlock_method_remove,
            vault_is_locked,
            vault_has_identity,
            vault_get_public_key,
            vault_get_user_uuid,
            vault_create,
            vault_open,
            vault_close,
            vault_list,
            vault_duplicates,
            vault_unlock_vault,
            vault_lock_vault,
            vault_sync,
            vault_sync_status,
            vault_personal_restore_review,
            vault_personal_restore_confirm,
            vault_sync_test,
            vault_sync_upgrade,
            vault_sync_clear_conflicts,
            vault_secret_create,
            vault_secret_read,
            vault_secret_update,
            vault_secret_delete,
            vault_secret_list,
            vault_invite_member,
            vault_accept_invite,
            vault_remove_member,
            vault_list_members,
            vault_delete,
            // Vault sharing individual items
            vault_share_item,
            vault_list_shared_items,
            vault_revoke_shared_item,
            vault_accept_shared_item,
            vault_list_received_shares,
            // Vault settings
            vault_get_settings,
            vault_save_settings,
            vault_get_turso_config,
            vault_set_turso_config,
            // Turso Platform API
            turso_create_database,
            turso_create_database_token,
            turso_list_personal_databases,
            // Personal sync config
            vault_set_personal_sync,
            vault_get_personal_sync,
            // Full backup
            vault_export_backup,
            vault_preview_backup,
            vault_import_backup,
            // Toolchain commands
            toolchain_check,
            toolchain_install,
            // Ansible commands
            ansible_list_projects,
            ansible_create_project,
            ansible_delete_project,
            ansible_open_project,
            ansible_update_inventory,
            ansible_list_files,
            ansible_read_file,
            ansible_write_file,
            ansible_run_command,
            ansible_engines,
            ansible_remote_engine,
            ansible_generate_inventory,
            ansible_write_inventory,
            ansible_list_roles,
            ansible_list_collections,
            ansible_vault_view,
            // OpenTofu commands
            tofu_list_projects,
            tofu_create_project,
            tofu_delete_project,
            tofu_open_project,
            tofu_run_command,
            tofu_binary_status,
            tofu_binary_install,
            tofu_binary_pin,
            tofu_binary_remove,
            tofu_read_file,
            tofu_write_file,
            tofu_list_files,
            tofu_get_provider_catalog,
            tofu_update_providers,
            tofu_update_variables,
            tofu_update_environments,
            tofu_generate_hcl,
            tofu_write_generated_files,
            tofu_get_resource_catalog,
            tofu_update_resources,
            tofu_state_list_resources,
            tofu_update_outputs,
            tofu_get_output_values,
            tofu_get_dependency_graph,
            tofu_get_templates,
            tofu_apply_template,
            tofu_update_backend,
            tofu_update_encryption,
            tofu_discard_plan,
            tofu_update_data_sources,
            tofu_get_data_source_catalog,
            tofu_update_locals,
            tofu_update_modules,
            tofu_get_backend_catalog,
            tofu_workspace_list,
            tofu_workspace_new,
            tofu_workspace_select,
            tofu_workspace_delete,
            tofu_fmt,
            tofu_show_plan_json,
            tofu_fetch_schema,
            tofu_get_cached_schema,
            tofu_get_schema_resource_fields,
            // Plugin commands
            plugin_discover,
            plugin_load,
            plugin_unload,
            plugin_reload,
            plugin_list,
            plugin_call_action,
            plugin_get_ui,
            plugin_get_config,
            plugin_set_config,
            plugin_get_dir,
            plugin_set_dir,
            plugin_dispatch_hook,
            // Marketplace commands
            marketplace_fetch,
            marketplace_install,
            marketplace_uninstall,
            marketplace_get_url,
            marketplace_set_url,
            marketplace_load_url,
            marketplace_reset_url,
            theme_fetch_registry,
            theme_install,
            theme_list_installed,
            theme_uninstall,
            theme_default_registry_url,
            terminal_scheme_download,
            // Editor commands
            editor_open_file,
            editor_get_pending_file,
            editor_hide_window,
            // Floating session windows
            float_window_open,
            rail_windows_sync,
            float_window_take,
            float_window_list,
            float_window_focus,
            // Web sessions (see `web`)
            web_session_open,
            web_session_cert_answer,
            web_session_cert_add,
            web_session_cert_remove,
            web_session_clear_data,
            bmc_remote_console,
            // WireGuard profiles and tunnels (see `wireguard`)
            wireguard_list,
            wireguard_get,
            wireguard_check,
            wireguard_save,
            wireguard_delete,
            wireguard_up,
            wireguard_down,
            wireguard_running,
            wireguard_generate_key,
            wireguard_session_route,
            // Snippet commands
            snippet_list,
            snippet_create,
            snippet_update,
            snippet_delete,
            recipe_list,
            recipe_get,
            recipe_preview,
            recipe_save,
            recipe_delete,
            recipe_template,
            recipe_prepare,
            recipe_fetch_registry,
            recipe_install,
            recipe_registry_url,
            registry_url_get,
            registry_url_set,
            registry_url_reset,
            share_get_config,
            share_set_enabled,
            share_set_ice_servers,
            share_reset_ice_servers,
            share_load,
            // Password managers. Off until the user turns them on; nothing
            // here returns a secret.
            pwmanager_get_config,
            pwmanager_save_config,
            pwmanager_status,
            pwmanager_test,
            pwmanager_list,
            pwmanager_passphrase_response,
            pwmanager_forget_passphrase,
            devops::devops_set_enabled,
            devops::devops_save_text,
            // Databases
            db_list_connections,
            db_save_connection,
            db_delete_connection,
            db_test_connection,
            db_route_preview,
            db_connect,
            db_disconnect,
            db_databases,
            db_schemas,
            db_objects,
            db_completion,
            db_definition,
            db_analyze,
            db_execute,
            db_cancel,
            db_close_session,
            db_table_rows,
            db_table_design,
            db_preview_edits,
            db_apply_edits,
            db_preview_design,
            db_apply_design,
            db_server_info,
            db_sessions,
            db_kill,
            db_cancel_job,
            db_backup,
            db_restore,
            db_csv_preview,
            db_import_csv,
            db_export,
            db_detect,
            db_redis_databases,
            db_redis_scan,
            db_redis_get,
            db_redis_command,
            db_redis_info,
            db_redis_clients,
            // Tray commands
            set_close_to_tray,
            get_close_to_tray,
            tray::tray_update,
            tray::tray_sync,
            quit_app,
        ]));
    }

    #[cfg(not(desktop))]
    {
        builder = builder.invoke_handler(refuse_web_pages(tauri::generate_handler![
            vault_account_sync_enabled,
            vault_organizations,
            vault_organization_list,
            vault_organization_create,
            vault_organization_open,
            vault_organization_grants,
            vault_organization_grant,
            vault_organization_finish_rotation,
            sync_account_status,
            sync_account_pair_origin,
            sync_account_pair,
            sync_account_cancel,
            sync_account_check,
            sync_account_sign_out,
            sync_account_manage,
            sync_account_preferences,
            sync_account_managed_removal,
            // MCP server. Off until the user enables it; nothing here starts
            // a listener or shares a session on its own.
            mcp_start,
            telnet_connect,
            telnet_send,
            telnet_resize,
            telnet_disconnect,
            // ZMODEM in SSH, Telnet and serial sessions
            zmodem_upload,
            zmodem_download,
            zmodem_cancel,
            ssh_zmodem_exec,
            rdp_connect,
            rdp_disconnect,
            rdp_mouse,
            rdp_key,
            rdp_unicode,
            rdp_sync_locks,
            rdp_local_numlock,
            rdp_resize,
            rdp_rail_windows,
            rdp_rail_command,
            rdp_view_attach,
            rdp_view_detach,
            rdp_view_ack,
            rdp_ack,
            rdp_disconnect_all,
            rdp_clipboard_sync,
            rdp_clipboard_send_files,
            rdp_clipboard_fetch,
            rdp_clipboard_cancel,
            rdp_clipboard_pick_files,
            rdp_clipboard_save_received,
            rdp_window_fullscreen,
            vnc_connect,
            vnc_disconnect,
            vnc_disconnect_all,
            vnc_ack,
            vnc_security,
            vnc_mouse,
            vnc_key,
            vnc_resize,
            vnc_clipboard_sync,
            spice_connect,
            spice_disconnect,
            spice_disconnect_all,
            spice_ack,
            spice_mouse,
            spice_key,
            spice_type,
            spice_resize,
            spice_clipboard_sync,
            spice_read_vv,
            mcp_stop,
            mcp_status,
            mcp_restore,
            mcp_share_session,
            mcp_unshare_session,
            mcp_list_agents,
            mcp_set_agent,
            mcp_set_mode,
            mcp_set_session_agent,
            mcp_set_session_mode,
            mcp_regenerate_token,
            mcp_confirm_response,
            // SSH commands
            ssh_connect,
            crate::ipc::ssh_commands::ssh_probe_addresses,
            crate::ipc::net_commands::net_resolve,
            crate::ipc::net_commands::net_ping,
            crate::ipc::net_commands::net_scan_ports,
            crate::ipc::net_commands::net_scan_cancel,
            crate::ipc::net_commands::net_wake,
            ssh_disconnect,
            ssh_files_scp,
            ssh_send,
            ssh_ready,
            ssh_hostkey_response,
            ssh_auth_prompt_response,
            session_log_default_dir,
            session_log_folder_exists,
            session_log_status,
            session_log_pause,
            session_log_stop,
            history_start,
            history_stop,
            history_pause,
            history_status,
            history_search,
            history_transcript,
            history_forget,
            history_configure,
            history_stats,
            app_update_check,
            ctr_open,
            ctr_open_live,
            ctr_hosts,
            ctr_host_save,
            ctr_host_delete,
            ctr_close,
            ctr_containers,
            ctr_images,
            ctr_volumes,
            ctr_networks,
            ctr_projects,
            ctr_act,
            ctr_remove,
            ctr_inspect,
            ctr_compose,
            ctr_logs,
            ctr_logs_stop,
            ctr_log_files,
            ctr_logs_files,
            devops_log_rules,
            devops_log_rules_save,
            ctr_shell_command,
            k8s_contexts,
            k8s_clusters,
            k8s_cluster_save,
            k8s_cluster_delete,
            k8s_open,
            k8s_close,
            k8s_namespaces,
            k8s_pods,
            k8s_workloads,
            k8s_objects,
            k8s_events,
            k8s_get_yaml,
            k8s_replace_yaml,
            k8s_delete,
            k8s_scale,
            k8s_restart,
            k8s_logs,
            k8s_logs_stop,
            k8s_log_files,
            k8s_logs_files,
            k8s_helm_releases,
            k8s_helm_history,
            k8s_helm_detail,
            k8s_helm_run,
            ssh_resize,
            ssh_list_connections,
            ssh_connection_algorithms,
            ssh_detect_os,
            ssh_tmux_kill,
            ssh_tmux_chosen,
            tmux_start_script,
            ssh_tmux_list,
            ssh_tmux_windows,
            ssh_tmux_window,
            inspect_key_file,
            // SSH Config commands
            sshconfig_scan,
            x11server_status,
            x11server_install,
            x11server_cancel,
            x11server_remove,
            sshconfig_exists,
            import_read_local,
            import_yaml_to_json,
            ssh_options_report,
            ssh_algorithm_catalog,
            ssh_key_import,
            ssh_key_list,
            ssh_key_update,
            ssh_key_delete,
            ssh_key_public,
            // SFTP commands
            sftp_list_dir,
            sftp_upload,
            sftp_download,
            sftp_delete,
            sftp_rename,
            sftp_mkdir,
            sftp_touch,
            sftp_read_file,
            sftp_write_file,
            sftp_archive_tools,
            sftp_archive_create,
            sftp_archive_extract,
            sftp_archive_cancel,
            sftp_transfer_cancel,
            sftp_terminal_cwd,
            sftp_home,
            ftp_connect,
            ftp_disconnect,
            ftp_session_cert_trust,
            vnc_session_cert_trust,
            // The phone's own files, for the full-pane file manager
            local_list_dir,
            local_mkdir,
            local_prepare_download_dir,
            local_rename,
            local_delete,
            local_join,
            local_roots,
            local_read_text,
            local_stat,
            local_edit_copy_path,
            local_touch,
            bmc_connect,
            bmc_disconnect,
            bmc_session_cert_trust,
            bmc_session_vnc_save,
            bmc_overview,
            bmc_sensors,
            bmc_hardware,
            bmc_log_services,
            bmc_log_entries,
            bmc_log_clear,
            bmc_jobs,
            bmc_virtual_media,
            bmc_power,
            bmc_boot_once,
            bmc_identify,
            bmc_media_insert,
            bmc_media_eject,
            bmc_reset_bmc,
            proxmox_connect,
            proxmox_console_ready,
            proxmox_console_login,
            proxmox_disconnect,
            proxmox_session_cert_trust,
            proxmox_overview,
            proxmox_resources,
            proxmox_guest,
            proxmox_guest_action,
            proxmox_snapshot_create,
            proxmox_snapshot_rollback,
            proxmox_snapshot_delete,
            proxmox_node_action,
            proxmox_tasks,
            proxmox_task_log,
            proxmox_task_stop,
            proxmox_pbs_overview,
            proxmox_pbs_namespaces,
            proxmox_pbs_groups,
            proxmox_pbs_snapshots,
            proxmox_pbs_gc,
            proxmox_pbs_verify,
            proxmox_pbs_protect,
            proxmox_pbs_forget,
            mqtt_connect,
            mqtt_disconnect,
            mqtt_publish,
            mqtt_subscribe,
            mqtt_unsubscribe,
            mqtt_session_cert_trust,
            drag_preview_icon,
            drag_cache_dir,
            dragout_start,
            webview_refocus,
            report::report_prepare,
            report::report_reveal,
            sound::play_notification_sound,
            sound::play_bell_sound,
            sound::set_audio_muted,
            // Session commands
            session_list,
            session_get,
            session_create,
            wsl_list_distros,
            session_update,
            session_untrusted_secret_refs,
            session_approve_secret_refs,
            session_delete,
            session_move_to_folder,
            session_set_favorite,
            web_session_cert_add,
            web_session_cert_remove,
            session_host_key_remove,
            session_host_key_replace,
            host_keys_remove_all,
            session_password_history,
            session_password_history_clear,
            saved_credential_password_history,
            saved_credential_password_history_clear,
            password_history_clear_all,
            session_set_order,
            session_bulk_update,
            session_bulk_undo,
            session_list_folders,
            session_create_folder,
            session_set_folder_credential,
            session_set_folder_tmux,
            session_set_folder_icon,
            session_rename_folder,
            saved_credential_list,
            personal_login_list,
            personal_login_set,
            credential_keys_unlock,
            credential_keys_unlock_enabled,
            credential_keys_biometric_forget,
            saved_credential_create,
            saved_credential_update,
            saved_credential_delete,
            saved_credential_move,
            credential_folder_list,
            credential_folder_create,
            credential_folder_rename,
            credential_folder_move,
            credential_folder_set_icon,
            credential_folder_delete,
            session_update_folder_parent,
            session_delete_folder,
            session_share,
            // Tunnel commands
            tunnel_create,
            tunnel_start,
            tunnel_stop,
            tunnel_remove,
            tunnel_list,
            // Monitoring commands
            monitoring_start,
            monitoring_start_wsl,
            monitoring_stop,
            monitoring_get_stats,

            // AI commands
            ai_chat,
            ai_fetch_models,
            // Credential commands
            credential_set_master_password,
            credential_verify_master_password,
            credential_is_locked,
            credential_lock,
            credential_has_master_password,
            // Settings commands
            settings_get_all,
            settings_get,
            settings_set,
            settings_delete,
            settings_save_all,
            fonts_list_installed,
            // Vault commands
            vault_init_identity,
            vault_unlock,
            vault_auto_unlock,
            vault_reset,
            vault_export_identity,
            vault_import_identity,
            vault_export_device_link,
            vault_link_device,
            vault_local_link_recovery_available,
            vault_restore_local_before_link,
            qr_svg,
            vault_lock,
            vault_is_held,
            vault_resume,
            vault_unlock_methods,
            vault_biometric_enable,
            vault_biometric_unlock,
            vault_security_key_add,
            vault_security_key_unlock,
            vault_unlock_method_remove,
            vault_is_locked,
            vault_has_identity,
            vault_get_public_key,
            vault_get_user_uuid,
            vault_create,
            vault_open,
            vault_close,
            vault_list,
            vault_duplicates,
            vault_unlock_vault,
            vault_lock_vault,
            vault_sync,
            vault_sync_status,
            vault_personal_restore_review,
            vault_personal_restore_confirm,
            vault_sync_test,
            vault_sync_upgrade,
            vault_sync_clear_conflicts,
            vault_secret_create,
            vault_secret_read,
            vault_secret_update,
            vault_secret_delete,
            vault_secret_list,
            vault_invite_member,
            vault_accept_invite,
            vault_remove_member,
            vault_list_members,
            vault_delete,
            // Vault sharing individual items
            vault_share_item,
            vault_list_shared_items,
            vault_revoke_shared_item,
            vault_accept_shared_item,
            vault_list_received_shares,
            // Vault settings
            vault_get_settings,
            vault_save_settings,
            vault_get_turso_config,
            vault_set_turso_config,
            // Turso Platform API
            turso_create_database,
            turso_create_database_token,
            turso_list_personal_databases,
            // Personal sync config
            vault_set_personal_sync,
            vault_get_personal_sync,
            // Full backup
            vault_export_backup,
            vault_preview_backup,
            vault_import_backup,
            // Toolchain commands
            toolchain_check,
            toolchain_install,
            // Ansible commands
            ansible_list_projects,
            ansible_create_project,
            ansible_delete_project,
            ansible_open_project,
            ansible_update_inventory,
            ansible_list_files,
            ansible_read_file,
            ansible_write_file,
            ansible_run_command,
            ansible_engines,
            ansible_remote_engine,
            ansible_generate_inventory,
            ansible_write_inventory,
            ansible_list_roles,
            ansible_list_collections,
            ansible_vault_view,
            // OpenTofu commands
            tofu_list_projects,
            tofu_create_project,
            tofu_delete_project,
            tofu_open_project,
            tofu_run_command,
            tofu_binary_status,
            tofu_binary_install,
            tofu_binary_pin,
            tofu_binary_remove,
            tofu_read_file,
            tofu_write_file,
            tofu_list_files,
            tofu_get_provider_catalog,
            tofu_update_providers,
            tofu_update_variables,
            tofu_update_environments,
            tofu_generate_hcl,
            tofu_write_generated_files,
            tofu_get_resource_catalog,
            tofu_update_resources,
            tofu_state_list_resources,
            tofu_update_outputs,
            tofu_get_output_values,
            tofu_get_dependency_graph,
            tofu_get_templates,
            tofu_apply_template,
            tofu_update_backend,
            tofu_update_encryption,
            tofu_discard_plan,
            tofu_update_data_sources,
            tofu_get_data_source_catalog,
            tofu_update_locals,
            tofu_update_modules,
            tofu_get_backend_catalog,
            tofu_workspace_list,
            tofu_workspace_new,
            tofu_workspace_select,
            tofu_workspace_delete,
            tofu_fmt,
            tofu_show_plan_json,
            tofu_fetch_schema,
            tofu_get_cached_schema,
            tofu_get_schema_resource_fields,
            // Plugin commands
            plugin_discover,
            plugin_load,
            plugin_unload,
            plugin_reload,
            plugin_list,
            plugin_call_action,
            plugin_get_ui,
            plugin_get_config,
            plugin_set_config,
            plugin_get_dir,
            plugin_set_dir,
            plugin_dispatch_hook,
            // Marketplace commands
            marketplace_fetch,
            marketplace_install,
            marketplace_uninstall,
            marketplace_get_url,
            marketplace_set_url,
            marketplace_load_url,
            marketplace_reset_url,
            theme_fetch_registry,
            theme_install,
            theme_list_installed,
            theme_uninstall,
            theme_default_registry_url,
            terminal_scheme_download,
            // Editor commands
            editor_open_file,
            editor_get_pending_file,
            editor_hide_window,
            // WireGuard profiles and tunnels (see `wireguard`)
            wireguard_list,
            wireguard_get,
            wireguard_check,
            wireguard_save,
            wireguard_delete,
            wireguard_up,
            wireguard_down,
            wireguard_running,
            wireguard_generate_key,
            wireguard_session_route,
            // Snippet commands
            snippet_list,
            snippet_create,
            snippet_update,
            snippet_delete,
            recipe_list,
            recipe_get,
            recipe_preview,
            recipe_save,
            recipe_delete,
            recipe_template,
            recipe_prepare,
            recipe_fetch_registry,
            recipe_install,
            recipe_registry_url,
            registry_url_get,
            registry_url_set,
            registry_url_reset,
            share_get_config,
            share_set_enabled,
            share_set_ice_servers,
            share_reset_ice_servers,
            share_load,
            // Password managers. Off until the user turns them on; nothing
            // here returns a secret.
            pwmanager_get_config,
            pwmanager_save_config,
            pwmanager_status,
            pwmanager_test,
            pwmanager_list,
            pwmanager_passphrase_response,
            pwmanager_forget_passphrase,
            devops::devops_set_enabled,
            devops::devops_save_text,
            // Databases
            db_list_connections,
            db_save_connection,
            db_delete_connection,
            db_test_connection,
            db_route_preview,
            db_connect,
            db_disconnect,
            db_databases,
            db_schemas,
            db_objects,
            db_completion,
            db_definition,
            db_analyze,
            db_execute,
            db_cancel,
            db_close_session,
            db_table_rows,
            db_table_design,
            db_preview_edits,
            db_apply_edits,
            db_preview_design,
            db_apply_design,
            db_server_info,
            db_sessions,
            db_kill,
            db_cancel_job,
            db_backup,
            db_restore,
            db_csv_preview,
            db_import_csv,
            db_export,
            db_detect,
            db_redis_databases,
            db_redis_scan,
            db_redis_get,
            db_redis_command,
            db_redis_info,
            db_redis_clients,
            // Tray commands
            set_close_to_tray,
            get_close_to_tray,
            quit_app,
        ]));
    }


    builder
        .setup(|app| {
            crate::history::spawn_writer(app.handle().clone());
            use tauri::Manager;

            // Before anything can take a keystroke: a held key must repeat.
            #[cfg(target_os = "macos")]
            disable_press_and_hold();

            // Session sharing needs `RTCPeerConnection`, and WebKitGTK ships
            // with WebRTC switched off: the constructor does not exist, so
            // every Start or Join failed before it began (see src/share).
            // Switching it on contacts nothing; the first packet still waits
            // for the user to click Share. The window already exists here and
            // its page has not loaded yet, so the setting reaches the first
            // document.
            #[cfg(target_os = "linux")]
            enable_webrtc(app);

            // Files fetched so they could be dragged to the desktop do not
            // survive a restart: they were copies made for one gesture, and
            // left alone they would accumulate at up to two gigabytes each.
            // Failure is ignored — a temp directory we cannot clear is not a
            // reason to refuse to start.
            {
                let stale = std::env::temp_dir().join("reachercm-drag");
                if stale.is_dir() {
                    let _ = std::fs::remove_dir_all(&stale);
                }
            }

            // Root all storage at a writable data dir, then manage AppState here
            // (not before build) so it picks up that dir.
            //
            // Desktop keeps its data in `dirs::data_dir()/APP_ID` (upstream Reach
            // uses `com.reach.app`). Reacher CM has its own folder so it can run
            // next to Reach.
            //
            // Mobile MUST use the Tauri-resolved OS sandbox: there
            // `dirs::data_dir()` is read-only, so vault writes fail with
            // "Read-only file system (os error 30)".
            #[cfg(not(desktop))]
            match app.path().app_data_dir() {
                Ok(dir) => set_app_data_dir(dir),
                Err(e) => tracing::error!("Failed to resolve app_data_dir: {}", e),
            }
            #[cfg(target_os = "android")]
            let _ = ANDROID_APP.set(app.handle().clone());
            // The phone's own folder for the file manager's local side.
            #[cfg(mobile)]
            ipc::local_fs_commands::set_device_files_dir(app.handle());
            let data_dir = app_data_dir();
            let _ = std::fs::create_dir_all(&data_dir);
            // The same for where those copies are kept now (`private_tmp_dir`),
            // and for files pasted from a remote desktop that a crash left.
            // Only one Reacher CM runs at a time, so nothing here is in use.
            let _ = std::fs::remove_dir_all(data_dir.join("tmp"));
            tracing::info!("App data dir: {:?}", data_dir);
            app.manage(AppState::new());
            #[cfg(desktop)]
            app.manage(LocalServers::default());

            // Build application menu (macOS menu bar).
            //
            // Copy and Paste differ by platform, and the difference is the
            // whole of issue #47's second report. On macOS, Cmd+C and Cmd+V
            // are menu key equivalents: the system looks for a menu item
            // bound to them and runs its selector, and if no item carries the
            // native copy:/paste: selector the keystroke does nothing — in
            // every text field in the app. Cut and Select All were native and
            // worked; Copy and Paste were custom and did not. So on macOS they
            // are the predefined items, and the terminal catches the native
            // paste as a DOM event and routes it through its own confirmation.
            //
            // On Windows and Linux the predefined Paste is implemented by
            // synthesising a Ctrl+V keystroke, which xterm's key handler
            // already owns, so there the items stay custom and emit an event
            // the terminal listens for.
            #[cfg(desktop)]
            {
                use tauri::menu::{MenuBuilder, PredefinedMenuItem, SubmenuBuilder};
                // Only the Windows and Linux edit menu builds its own items.
                #[cfg(not(target_os = "macos"))]
                use tauri::menu::MenuItemBuilder;
                use tauri::Emitter;

                let app_name = APP_NAME;

                // --- Reach menu ---
                let about = PredefinedMenuItem::about(app, Some(&format!("About {APP_NAME}")), None)?;
                let services = PredefinedMenuItem::services(app, None)?;
                let separator1 = PredefinedMenuItem::separator(app)?;
                let hide = PredefinedMenuItem::hide(app, Some(&format!("Hide {APP_NAME}")))?;
                let hide_others = PredefinedMenuItem::hide_others(app, None)?;
                let show_all = PredefinedMenuItem::show_all(app, None)?;
                let separator2 = PredefinedMenuItem::separator(app)?;
                let quit = PredefinedMenuItem::quit(app, Some(&format!("Quit {APP_NAME}")))?;
                let reach_menu = SubmenuBuilder::new(app, app_name)
                    .item(&about)
                    .item(&services)
                    .item(&separator1)
                    .item(&hide)
                    .item(&hide_others)
                    .item(&show_all)
                    .item(&separator2)
                    .item(&quit)
                    .build()?;

                // --- File menu ---
                let file_menu = SubmenuBuilder::new(app, "File")
                    .item(&PredefinedMenuItem::close_window(app, Some("Close Window"))?)
                    .build()?;

                // --- Edit menu ---
                #[cfg(target_os = "macos")]
                let (copy_item, paste_item) = (
                    PredefinedMenuItem::copy(app, None)?,
                    PredefinedMenuItem::paste(app, None)?,
                );
                #[cfg(not(target_os = "macos"))]
                let (copy_item, paste_item) = (
                    MenuItemBuilder::with_id("copy", "Copy").build(app)?,
                    MenuItemBuilder::with_id("paste", "Paste").build(app)?,
                );
                let cut_item = PredefinedMenuItem::cut(app, None)?;
                let select_all = PredefinedMenuItem::select_all(app, None)?;
                let edit_menu = SubmenuBuilder::new(app, "Edit")
                    .item(&copy_item)
                    .item(&paste_item)
                    .item(&cut_item)
                    .separator()
                    .item(&select_all)
                    .build()?;

                // --- View menu ---
                let view_menu = SubmenuBuilder::new(app, "View")
                    .item(&PredefinedMenuItem::fullscreen(app, None)?)
                    .build()?;

                // --- Window menu ---
                let minimize = PredefinedMenuItem::minimize(app, None)?;
                let zoom = PredefinedMenuItem::maximize(app, None)?;
                let window_menu = SubmenuBuilder::new(app, "Window")
                    .item(&minimize)
                    .item(&zoom)
                    .build()?;

                // --- Help menu ---
                let help_menu = SubmenuBuilder::new(app, "Help")
                    .build()?;

                let app_menu = MenuBuilder::new(app)
                    .item(&reach_menu)
                    .item(&file_menu)
                    .item(&edit_menu)
                    .item(&view_menu)
                    .item(&window_menu)
                    .item(&help_menu)
                    .build()?;

                // GTK draws a menu bar inside the window, above Reach's own
                // title bar: a strip of macOS items (Services, Hide Others)
                // whose menus did not even open. Reach's title bar and the
                // terminal's own keys cover everything it offered there.
                #[cfg(not(target_os = "linux"))]
                app.set_menu(app_menu)?;
                #[cfg(target_os = "linux")]
                drop(app_menu);

                // Route menu-bar Copy/Paste clicks to the webview so the active
                // terminal can handle them.
                app.on_menu_event(move |app_handle, event| {
                    match event.id().as_ref() {
                        "copy" => {
                            if let Some(w) = app_handle.get_webview_window("main") {
                                let _ = w.emit("menu-copy", ());
                            }
                        }
                        "paste" => {
                            if let Some(w) = app_handle.get_webview_window("main") {
                                let _ = w.emit("menu-paste", ());
                            }
                        }
                        _ => {}
                    }
                });
            }

            // Build system tray
            #[cfg(desktop)]
            {
                use tauri::menu::{MenuBuilder, MenuItemBuilder};
                use tauri::tray::TrayIconBuilder;
                use tauri::image::Image;

                // The menu starts as Show/Quit and is rebuilt in the
                // interface's language once it reports what is open (see
                // `tray`). Left click brings the window up; the menu is on
                // right click (Linux tray hosts open it on any click).
                let tray_menu = MenuBuilder::new(app)
                    .item(&MenuItemBuilder::with_id("show", "Show").build(app)?)
                    .separator()
                    .item(&MenuItemBuilder::with_id("quit", "Quit").build(app)?)
                    .build()?;

                let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))
                    .expect("failed to load tray icon");

                TrayIconBuilder::with_id(tray::TRAY_ID)
                    .icon(icon)
                    .tooltip(APP_NAME)
                    .menu(&tray_menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app_handle, event| {
                        tray::handle_menu_event(app_handle, event.id().as_ref());
                    })
                    .on_tray_icon_event(|tray_icon, event| {
                        use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
                        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                            tray::show_main(tray_icon.app_handle());
                        }
                    })
                    .build(app)?;
            }

            // Prepend tools dir to PATH so installed tools are found
            {
                let tools_dir = app_data_dir().join("tools");
                let _ = std::fs::create_dir_all(&tools_dir);
                let current_path = std::env::var("PATH").unwrap_or_default();
                let sep = if cfg!(windows) { ";" } else { ":" };
                std::env::set_var(
                    "PATH",
                    format!("{}{}{}", tools_dir.display(), sep, current_path),
                );
                tracing::info!("Tools directory added to PATH: {:?}", tools_dir);
            }

            // When the computer locks or sleeps, tell the interface, which locks
            // Reach too if the user asked for that (Settings → Security). A
            // password manager passphrase kept in memory goes in any case,
            // whatever that setting says, and any passphrase question still
            // open is cancelled: nobody is at the keyboard to have typed it.
            {
                let handle = app.handle().clone();
                system_lock::watch(std::sync::Arc::new(move || {
                    pwmanager::forget_passphrase();
                    let _ = tauri::Emitter::emit(&handle, "system-locked", ());
                }));
            }

            // Every vault is a local file; synced ones exchange changes with
            // their backend once a vault unlocks, a few seconds after each
            // change, and every minute. When a sync brought something in,
            // the interface is told so it reloads what it shows.
            {
                let vaults = app.state::<AppState>().vault_manager.clone();
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut every_minute = tokio::time::interval(std::time::Duration::from_secs(60));
                    every_minute.tick().await;
                    loop {
                        let timed = tokio::select! {
                            _ = every_minute.tick() => true,
                            _ = vault::manager::sync_requested() => {
                                // Let a burst of changes go out together.
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                false
                            }
                        };
                        let (changed, _failed) = vault::manager::sync_vaults(&vaults, None, timed).await;
                        for vault_id in changed {
                            let _ = tauri::Emitter::emit(&handle, "vault-synced", vault_id);
                        }
                        let _ = tauri::Emitter::emit(&handle, "vault-sync-status", ());
                    }
                });
            }

            let handle = app.handle().clone();

            // Clone state arcs for plugin auto-loading
            let app_state = app.state::<AppState>();
            let ssh_mgr = app_state.ssh_manager.clone();
            let tunnel_mgr = app_state.tunnel_manager.clone();
            let vault_mgr = app_state.vault_manager.clone();
            let plugin_mgr = app_state.plugin_manager.clone();

            tauri::async_runtime::spawn(async move {
                let app_data_dir = match handle.path().app_data_dir() {
                    Ok(dir) => dir,
                    Err(e) => {
                        tracing::warn!("Failed to get app data dir: {}", e);
                        return;
                    }
                };

                if let Err(e) = std::fs::create_dir_all(&app_data_dir) {
                    tracing::warn!("Failed to create app data dir: {}", e);
                    return;
                }

                tracing::info!("App data directory ready: {:?}", app_data_dir);

                // Auto-load plugins on startup
                let saved_configs = {
                    let vm = vault_mgr.lock().await;
                    plugin::storage::load_plugin_configs(&vm)
                        .await
                        .unwrap_or_default()
                };

                let config_map: std::collections::HashMap<String, plugin::schema::PluginConfig> =
                    saved_configs
                        .into_iter()
                        .map(|c| (c.id.clone(), c))
                        .collect();

                let mut pm = plugin_mgr.lock().await;
                if let Ok(manifests) = pm.discover_plugins() {
                    for manifest in manifests {
                        let config =
                            config_map.get(&manifest.id).cloned().unwrap_or_else(|| {
                                plugin::schema::PluginConfig {
                                    id: manifest.id.clone(),
                                    enabled: true,
                                    granted_permissions: manifest.permissions.clone(),
                                    version_at_grant: Some(manifest.version.clone()),
                                }
                            });
                        if config.enabled {
                            match pm.load_plugin(
                                &manifest.id,
                                config,
                                ssh_mgr.clone(),
                                tunnel_mgr.clone(),
                                vault_mgr.clone(),
                                Some(handle.clone()),
                            ) {
                                Ok(info) => {
                                    tracing::info!(
                                        "Auto-loaded plugin: {} ({}) [{:?}]",
                                        info.manifest.name,
                                        info.manifest.id,
                                        info.status,
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to auto-load plugin {}: {}",
                                        manifest.id,
                                        e,
                                    );
                                }
                            }
                        }
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // What was dropped on the window, as the OS gave it: the RDP
            // clipboard sends only files that really were dropped.
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                rdp::note_drop(paths);
            }
            // The main window shown, hidden or brought back: the tray's
            // Show/Hide item follows.
            #[cfg(desktop)]
            if let tauri::WindowEvent::Focused(_) = event {
                if window.label() == "main" {
                    tray::sync_visibility(tauri::Manager::app_handle(window));
                }
            }
            // The main window gone (the app closing): its floating windows go too.
            #[cfg(desktop)]
            if let tauri::WindowEvent::Destroyed = event {
                if window.label() == "main" {
                    ipc::window_commands::close_floating(tauri::Manager::app_handle(window));
                    if let Some(servers) = tauri::Manager::try_state::<LocalServers>(window) {
                        if let Ok(mut servers) = servers.0.try_lock() {
                            servers.stop_all();
                        }
                    }
                    web::window::close_all(tauri::Manager::app_handle(window));
                }
                if web::is_web_label(window.label()) {
                    web::window::destroyed(window.label());
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // A web window on an iDRAC logs out first, then closes itself.
                // Web windows are desktop only.
                #[cfg(desktop)]
                if web::is_web_label(window.label()) && web::window::close_requested(tauri::Manager::app_handle(window), window.label()) {
                    api.prevent_close();
                }
                // Editor windows: ALWAYS hide instead of close (prevents WebView2 crash on Windows)
                if window.label().starts_with("editor") {
                    api.prevent_close();
                    #[cfg(desktop)]
                    let _ = window.hide();
                }
                // Main window: the frontend's `onCloseRequested` is the single
                // owner — it always prevents the close, then either hides to tray
                // or runs the active-session guard. Acting here too would race it
                // (e.g. hide the window while the frontend tries to destroy it).
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Reach application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                logout_bmcs(app);
            }
        });
}

/// Log out of every open server management connection as the app quits:
/// controllers allow only a few sessions, and one left behind holds a slot
/// until it times out. Best-effort, at most 3 seconds.
fn logout_bmcs(app: &tauri::AppHandle) {
    use tauri::Manager;
    let Some(state) = app.try_state::<AppState>() else { return };
    let conns = state.bmc_manager.drain();
    if conns.is_empty() {
        return;
    }
    tauri::async_runtime::block_on(async move {
        let all = futures_util::future::join_all(conns.iter().map(|c| c.client.logout()));
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), all).await;
    });
}

/// Messages from a web session's window never reach a command, whatever
/// page it shows and whatever the capabilities say (see `web`).
fn refuse_web_pages<F>(handler: F) -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static
where
    F: Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
{
    move |invoke| {
        let webview = invoke.message.webview_ref();
        if web::is_web_label(webview.label()) || web::is_web_label(webview.window_ref().label()) {
            tracing::warn!("Refused a command from a web session window");
            invoke.resolver.reject("Not available to web pages");
            return true;
        }
        handler(invoke)
    }
}

#[cfg(test)]
mod metainfo_tests {
    /// The Linux metadata (software centres, the AppImage catalog) lists the
    /// version being built, so a release cannot ship with a stale history.
    #[test]
    fn metainfo_lists_this_version() {
        let xml = include_str!("../linux/com.reachercm.client.metainfo.xml");
        let release = format!("<release version=\"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(
            xml.contains(&release),
            "linux/com.reachercm.client.metainfo.xml has no {release} entry: add this version to <releases>"
        );
    }
}

#[cfg(test)]
mod log_secret_tests {
    use std::path::Path;

    /// Argument types that hold a password, a passphrase, a key or a token.
    const SECRET_TYPES: &[&str] = &[
        "AuthMethod",
        "SessionConfig",
        "ProxyConfig",
        "JumpHostConfig",
        "SshOptions",
        "SecretBox",
        "SecretString",
    ];
    /// Argument names that do.
    const SECRET_NAMES: &[&str] = &[
        "password", "passphrase", "token", "secret", "key_content", "private_key", "pin",
    ];

    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// The parameter list of the first fn after `from`, split at top-level commas.
    fn params(source: &str) -> Vec<(String, String)> {
        let Some(fn_at) = source.find("fn ") else { return vec![] };
        let rest = &source[fn_at..];
        let Some(open) = rest.find('(') else { return vec![] };
        let (mut depth, mut end) = (0i32, open);
        for (i, c) in rest[open..].char_indices() {
            match c {
                '(' | '<' | '[' => depth += 1,
                ')' | '>' | ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let (mut parts, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
        for c in rest[open + 1..end].chars() {
            match c {
                '(' | '<' | '[' => depth += 1,
                ')' | '>' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        parts.push(cur);
        parts
            .iter()
            .filter_map(|p| {
                let (name, ty) = p.split_once(':')?;
                let name = name.trim().trim_start_matches("mut ").trim().to_string();
                Some((name, ty.trim().to_string()))
            })
            .collect()
    }

    /// Every argument an instrument span keeps is written to reach.log, so an
    /// argument holding a secret has to be skipped (or the span `skip_all`).
    /// Logs are what people attach to bug reports.
    #[test]
    #[cfg_attr(
        target_os = "android",
        ignore = "reads the crate's source, which is not on the device; the desktop runs check the same code"
    )]
    fn instrument_spans_never_record_secrets() {
        let mut files = Vec::new();
        rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut leaks = Vec::new();
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            for (at, _) in source.match_indices("#[tracing::instrument") {
                let attr_end = at + source[at..].find(")]").map_or(0, |e| e + 2);
                let attr = &source[at..attr_end];
                if attr.contains("skip_all") {
                    continue;
                }
                let skipped: Vec<&str> = attr
                    .split("skip(")
                    .nth(1)
                    .and_then(|s| s.split(')').next())
                    .map(|s| s.split(',').map(str::trim).collect())
                    .unwrap_or_default();
                for (name, ty) in params(&source[attr_end..]) {
                    if name.contains("self") || skipped.contains(&name.as_str()) {
                        continue;
                    }
                    // An id or a yes/no flag names a secret without holding it.
                    if name.ends_with("_id") || ty.trim_start_matches("Option<").starts_with("bool") {
                        continue;
                    }
                    let lower = name.to_lowercase();
                    if SECRET_TYPES.iter().any(|t| ty.contains(t)) || SECRET_NAMES.iter().any(|n| lower.contains(n)) {
                        let line = source[..at].lines().count() + 1;
                        leaks.push(format!("{}:{line}: `{name}: {ty}`", file.display()));
                    }
                }
            }
        }
        assert!(leaks.is_empty(), "instrument spans that log a secret (add it to skip(...)):\n{}", leaks.join("\n"));
    }
}

#[cfg(all(test, desktop))]
mod purge_logs_tests {
    use super::purge_logs_with_secrets;

    #[test]
    fn old_logs_go_once_and_new_ones_stay() {
        let dir = std::env::temp_dir().join(format!("reach-purge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("reach.log"), "auth_method=Password { password: Some(\"x\") }").unwrap();
        std::fs::write(dir.join("reach.1.log"), "older").unwrap();
        std::fs::write(dir.join("other.txt"), "kept").unwrap();

        purge_logs_with_secrets(&dir);
        assert!(!dir.join("reach.log").exists(), "the old log is deleted");
        assert!(!dir.join("reach.1.log").exists(), "the rotated old log is deleted");
        assert!(dir.join("other.txt").exists(), "nothing else in the folder is touched");
        assert!(dir.join(".secrets-purged").exists(), "the purge is remembered");

        std::fs::write(dir.join("reach.log"), "new").unwrap();
        purge_logs_with_secrets(&dir);
        assert!(dir.join("reach.log").exists(), "a log written after the purge is kept");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
