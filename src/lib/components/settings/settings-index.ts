/**
 * Every setting Settings search can find, and the page it is on.
 *
 * A row is found on its page by its label: the result opens the page, finds
 * the row whose label reads the same, opens the Advanced section around it
 * if it is closed, and scrolls it into view. So a row needs no id of its
 * own; its label key is its id (openSettings('settings.term_osc52')).
 *
 * `words` are extra English search terms (what people call a setting when
 * they do not know its name). Labels and descriptions match in the user's
 * own language.
 */

import type { PageId } from './settings-pages.js';

export interface IndexedRow {
	page: PageId;
	/** i18n key of the row's label, as shown on the page. */
	label: string;
	desc?: string;
	words?: string;
	/** Inside the page's collapsed Advanced section. */
	advanced?: boolean;
	/** Not shown on a phone or tablet. */
	desktopOnly?: boolean;
	/** Only shown on a phone or tablet. */
	phoneOnly?: boolean;
}

export const SETTINGS_INDEX: IndexedRow[] = [
	{ page: 'account', label: 'sync_account.server', words: 'self hosted sign in login devices pair QR' },
	{ page: 'ai', label: 'ai_settings.enable', desc: 'ai_settings.enable_desc', words: 'llm openrouter assistant' },
	{ page: 'ai', label: 'ai_settings.api_key', desc: 'ai_settings.api_key_desc' },
	{ page: 'ai', label: 'ai_settings.model_browser' },
	{ page: 'appearance', label: 'settings.theme', words: 'dark light colours colors' },
	{ page: 'appearance', label: 'themes.marketplace' },
	{ page: 'appearance', label: 'settings.ui_zoom', desc: 'settings.ui_zoom_hint', words: 'scale zoom size interface', desktopOnly: true },
	{ page: 'shortcuts', label: 'shortcuts.title', desc: 'shortcuts.desc', words: 'keyboard shortcuts hotkeys keybindings keys bind rebind', desktopOnly: true },
	{ page: 'appearance', label: 'settings.compact_layout', desc: 'settings.compact_layout_desc' },
	{ page: 'appearance', label: 'termbar.setting', desc: 'termbar.setting_desc' },
	{ page: 'terminal', label: 'settings.terminal_font', desc: 'settings.font_desc', words: 'font typeface monospace' },
	{ page: 'terminal', label: 'settings.font_size', desc: 'settings.font_size_hint', words: 'zoom text size' },
	{ page: 'terminal', label: 'cursor.style', desc: 'cursor.style_desc', words: 'caret block underline bar' },
	{ page: 'terminal', label: 'cursor.width', desc: 'cursor.width_desc' },
	{ page: 'terminal', label: 'cursor.blink', desc: 'cursor.blink_desc' },
	{ page: 'terminal', label: 'cursor.inactive', desc: 'cursor.inactive_desc' },
	{ page: 'terminal', label: 'settings.scrollback', desc: 'settings.scrollback_desc', words: 'history buffer lines scroll back' },
	{ page: 'terminal', label: 'settings.line_numbers', desc: 'settings.line_numbers_desc', words: 'gutter row numbers' },
	{ page: 'terminal', label: 'settings.timestamps', desc: 'settings.timestamps_desc', words: 'time clock gutter when' },
	{ page: 'terminal', label: 'highlight.setting', desc: 'highlight.setting_desc', words: 'highlight colour color error warning ip mac address syntax' },
	{ page: 'terminal', label: 'highlight.kinds', desc: 'highlight.kinds_desc' },
	{ page: 'terminal', label: 'highlight.custom', desc: 'highlight.custom_desc', words: 'regex regexp regular expression pattern custom' },
	{ page: 'terminal', label: 'settings.shell_colors', desc: 'settings.shell_colors_desc', advanced: true },
	{ page: 'terminal', label: 'settings.login_message', desc: 'settings.login_message_desc', advanced: true },
	{ page: 'sessions', label: 'settings.log_mode', desc: 'settings.log_mode_desc', words: 'record file log' },
	{ page: 'sessions', label: 'settings.log_folder' },
	{ page: 'sessions', label: 'settings.log_name', desc: 'settings.log_name_desc' },
	{ page: 'sessions', label: 'settings.log_append', desc: 'settings.log_append_desc' },
	{ page: 'sessions', label: 'settings.tmux_new_tab', desc: 'settings.tmux_new_tab_desc', words: 'tmux window tab ctrl+t' },
	{ page: 'sessions', label: 'settings.tmux_connect_windows', desc: 'settings.tmux_connect_windows_desc', words: 'tmux windows tabs' },
	{ page: 'sessions', label: 'settings.tmux_hide_status', desc: 'settings.tmux_hide_status_desc', words: 'tmux status bar line' },
	{ page: 'sessions', label: 'settings.log_header', desc: 'settings.log_header_desc' },
	{ page: 'sessions', label: 'history.settings_mode', desc: 'history.settings_mode_desc', words: 'command history' },
	{ page: 'sessions', label: 'history.settings_keep', desc: 'history.settings_keep_desc' },
	{ page: 'sessions', label: 'history.settings_patterns', desc: 'history.settings_patterns_desc' },
	{ page: 'sessions', label: 'history.settings_stored' },
	{ page: 'remote', label: 'settings.freerdp_path', desc: 'settings.freerdp_path_desc', desktopOnly: true },
	{ page: 'remote', label: 'settings.rdp_hw_render', desc: 'settings.rdp_hw_render_desc' },
	{ page: 'remote', label: 'settings.rdp_fs_button', desc: 'settings.rdp_fs_button_desc' },
	{ page: 'remote', label: 'settings.rdp_gfx', desc: 'settings.rdp_gfx_desc', words: 'h.264 h264 video codec', advanced: true },
	{ page: 'remote', label: 'settings.rdp_gfx_new', desc: 'settings.rdp_gfx_new_desc', words: 'avc444 remotefx progressive codec picture', advanced: true },
	{ page: 'sync', label: 'backup.export' },
	{ page: 'sync', label: 'backup.import' },
	{ page: 'general', label: 'settings.language' },
	{ page: 'general', label: 'settings.startup_behavior', desc: 'settings.restore_tabs_desc', words: 'restore tabs reopen' },
	{ page: 'general', label: 'settings.minimize_to_tray', desc: 'settings.tray_desc', desktopOnly: true },
	{ page: 'general', label: 'settings.start_with_system', desc: 'settings.system_startup_desc', desktopOnly: true },
	{ page: 'general', label: 'settings.play_sounds', desc: 'settings.play_sounds_desc', words: 'sound mute chime notification audio beep quiet' },
	{ page: 'general', label: 'settings.rerun_setup', desc: 'settings.rerun_setup_desc', words: 'wizard first run onboarding welcome' },
	{ page: 'about', label: 'settings.check_updates', desc: 'settings.check_updates_desc', words: 'update version' },
	{ page: 'terminal', label: 'settings.file_editor', desc: 'settings.file_editor_desc', words: 'editor vim nano emacs code codium vscode sublime f4', desktopOnly: true },
	{ page: 'terminal', label: 'settings.default_shell', desc: 'settings.shell_desc', words: 'bash zsh powershell cmd' },
	{ page: 'terminal', label: 'settings.bell_sound', desc: 'settings.bell_sound_desc', words: 'bell beep ^g sound audible' },
	{ page: 'terminal', label: 'settings.bell_flash', desc: 'settings.bell_flash_desc', words: 'bell visual flash' },
	{ page: 'terminal', label: 'settings.bell_notify', desc: 'settings.bell_notify_desc', words: 'bell notification alert background' },
	{ page: 'terminal', label: 'settings.paste_warning', desc: 'settings.paste_warning_desc', words: 'clipboard multiline' },
	{ page: 'terminal', label: 'settings.paste_threshold', desc: 'settings.paste_threshold_desc' },
	{ page: 'terminal', label: 'settings.term_right_click', desc: 'settings.term_right_click_desc', words: 'mouse context menu paste', desktopOnly: true },
	{ page: 'terminal', label: 'settings.term_copy_on_select', desc: 'settings.term_copy_on_select_desc', words: 'clipboard selection', desktopOnly: true },
	{ page: 'terminal', label: 'settings.term_osc52', desc: 'settings.term_osc52_desc', words: 'clipboard osc 52 tmux vim', advanced: true, desktopOnly: true },
	{ page: 'terminal', label: 'settings.term_middle_click', desc: 'settings.term_middle_click_desc', words: 'mouse paste', advanced: true, desktopOnly: true },
	{ page: 'terminal', label: 'settings.term_word_separators', desc: 'settings.term_word_separators_desc', words: 'double click select word', advanced: true, desktopOnly: true },
	{ page: 'sessions', label: 'settings.double_click_open', desc: 'settings.double_click_open_desc' },
	{ page: 'sessions', label: 'settings.close_tab_on_exit', desc: 'settings.close_tab_on_exit_desc' },
	{ page: 'sessions', label: 'settings.server_monitoring', desc: 'settings.server_monitoring_desc', words: 'cpu memory disk stats' },
	{ page: 'sessions', label: 'settings.check_addresses', desc: 'settings.check_addresses_desc', words: 'address silent probe reachability ip' },
	{ page: 'about', label: 'settings.website', desc: 'settings.website_desc', words: 'home page download' },
	{ page: 'about', label: 'settings.changelog', desc: 'settings.changelog_desc', words: 'release notes what is new history' },
	{ page: 'about', label: 'settings.license', desc: 'settings.license_desc', words: 'gpl gnu mit copyright licence' },
	{ page: 'about', label: 'settings.community', desc: 'settings.community_desc' },
	{ page: 'about', label: 'report.title', desc: 'report.desc' },
	{ page: 'about', label: 'settings.support', desc: 'settings.support_desc' },
	{ page: 'ai', label: 'mcp.enable', desc: 'mcp.enable_desc', words: 'claude agent model context protocol' },
	{ page: 'ai', label: 'mcp.port', desc: 'mcp.port_desc' },
	{ page: 'ai', label: 'mcp.agent' },
	{ page: 'ai', label: 'mcp.mode' },
	{ page: 'pwmanagers', label: 'pwmanager.enable', desc: 'pwmanager.enable_desc' },
	{ page: 'pwmanagers', label: 'pwmanager.cli' },
	{ page: 'pwmanagers', label: 'pwmanager.op_enable', desc: 'pwmanager.op_enable_desc' },
	{ page: 'pwmanagers', label: 'pwmanager.op_cli' },
	{ page: 'pwmanagers', label: 'pwmanager.bw_enable', desc: 'pwmanager.bw_enable_desc' },
	{ page: 'pwmanagers', label: 'pwmanager.bw_cli' },
	{ page: 'pwmanagers', label: 'pwmanager.kp_enable', desc: 'pwmanager.kp_enable_desc' },
	{ page: 'pwmanagers', label: 'pwmanager.kp_database' },
	{ page: 'devops', label: 'settings.plugins_dir' },
	{ page: 'devops', label: 'settings.plugins_installed' },
	{ page: 'devops', label: 'settings.registries', words: 'plugin registry url', advanced: true },
	{ page: 'security', label: 'security.master_password_status' },
	{ page: 'security', label: 'security.lock_status' },
	{ page: 'security', label: 'security.auto_lock', desc: 'security.auto_lock_desc', words: 'timeout lock' },
	{ page: 'security', label: 'security.lock_on_system_lock', desc: 'security.lock_on_system_lock_desc' },
	{ page: 'security', label: 'security.unlock_keys_at_start', desc: 'security.unlock_keys_at_start_desc' },
	{ page: 'security', label: 'security.host_keys_remove_all', desc: 'security.host_keys_remove_all_desc', words: 'known hosts fingerprint server key' },
	{ page: 'security', label: 'password_history.limit', desc: 'password_history.limit_desc', words: 'old previous passwords history keep' },
	{ page: 'security', label: 'password_history.clear_all', desc: 'password_history.clear_all_desc', words: 'old previous passwords history forget delete' },
	{ page: 'security', label: 'security.biometric', words: 'face id touch id fingerprint windows hello' },
	{ page: 'security', label: 'security.keys', words: 'yubikey fido security key' },
	{ page: 'sharing', label: 'sharing.enable', desc: 'sharing.enable_desc', words: 'share session collaborate' },
	{ page: 'sync', label: 'sync.backend_title' },
	{ page: 'sync', label: 'sync.status_title' },
	{ page: 'sync', label: 'sync.identity' },
	{ page: 'sync', label: 'sync.user_uuid' },
	{ page: 'sync', label: 'sync.public_key' },
	{ page: 'sync', label: 'sync.link_device', words: 'pair another device' },
	{ page: 'sync', label: 'sync.join_device' },
	{ page: 'sync', label: 'sync.cloud_sync', words: 'turso cloud' },
	{ page: 'sync', label: 'sync.personal_database' },
	{ page: 'sync', label: 'sync.accept_invite' },
	{ page: 'terminal', label: 'termtheme.title', words: 'colours colors scheme theme iterm palette' },
	{ page: 'sessions', label: 'settings.x11_server', words: 'x11 xming vcxsrv display forwarding', desktopOnly: true },
	{ page: 'devops', label: 'settings.devops', words: 'ansible opentofu terraform databases containers docker kubernetes' },
	{ page: 'terminal', label: 'phonekeys.title', words: 'extra keys keyboard ctrl alt esc f1 function keys', phoneOnly: true },
	{ page: 'about', label: 'settings.version', words: 'version release' },
];

/** Rows whose label, description or extra words contain every word typed. */
export function searchSettings(
	query: string,
	translate: (key: string) => string,
	pageLabel: (page: PageId) => string,
	mobile: boolean
): IndexedRow[] {
	const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
	if (terms.length === 0) return [];
	return SETTINGS_INDEX.filter((row) => {
		if (mobile ? row.desktopOnly : row.phoneOnly) return false;
		const hay = [translate(row.label), row.desc ? translate(row.desc) : '', row.words ?? '', pageLabel(row.page)]
			.join(' ')
			.toLowerCase();
		return terms.every((term) => hay.includes(term));
	});
}
