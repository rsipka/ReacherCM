<script lang="ts">
	import { canFloat } from '#lib/state/windows.svelte.js';
	import { allowNotifications } from '#lib/terminal/bell.js';
	import { enable, disable, isEnabled } from '@tauri-apps/plugin-autostart';
	import { isMobile } from '#lib/platform.js';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import FaIcon from '#lib/components/shared/FaIcon.svelte';
	import { faBug, faCopy, faUpRightFromSquare } from '@fortawesome/free-solid-svg-icons';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import Dropdown from '#lib/components/shared/Dropdown.svelte';
	import DiscordIcon from '#lib/components/shared/DiscordIcon.svelte';
	import { COMMUNITY } from '#lib/data/community.js';
	import { SHOW_COMMUNITY, SHOW_SPONSOR, WEBSITE_URL, CHANGELOG_URL, LICENSE_URL } from '#lib/brand.js';
	import LanguageSelect from '#lib/components/shared/LanguageSelect.svelte';
	import Toggle from '#lib/components/shared/Toggle.svelte';
	import X11ServerSetting from './X11ServerSetting.svelte';
	import ReportProblem from './ReportProblem.svelte';
	import { getSettings, updateSetting, syncTraySettings } from '#lib/state/settings.svelte.js';
	import { manualUpdateCheck, getUpdaterState, openWhatsNew } from '#lib/state/updater.svelte.js';
	import { openSetupWizard } from '#lib/state/setup-wizard.svelte.js';
	import { effectiveThreshold, MIN_PASTE_THRESHOLD } from '#lib/terminal/paste.js';
	import { DEFAULT_WORD_SEPARATORS, rightClickMode } from '#lib/terminal/mouse.js';
	import { t, changeLocale } from '#lib/state/i18n.svelte.js';
	import { __APP_VERSION__ } from '#lib/version.js';
	import { detectEditors } from '#lib/explorer/editor-launch.js';
	import type { EditorInfo } from '#lib/ipc/localfs.js';
	import { winboxFind, type WinboxPasswordMode } from '#lib/ipc/winbox.js';
	import { open as openDialog } from '@tauri-apps/plugin-dialog';

	/** Which slice of these rows to show: Settings arranges them by topic,
	 *  so one tab file feeds several pages (see settings-pages.ts). */
	type Part = 'general' | 'shell' | 'paste' | 'bell' | 'paste-adv' | 'sessions' | 'tmux' | 'updates' | 'about';
	let { part }: { part: Part } = $props();

	const settings = getSettings();
	const updater = getUpdaterState();

	/** Report a problem: the dialog cleans and shows the log before anything is shared. */
	let reportOpen = $state(false);

	function onLanguageChange(value: string) {
		changeLocale(value);
		updateSetting('locale', value);
	}

	const shellOptions = [
		{ label: 'Bash', value: '/bin/bash' },
		{ label: 'Zsh', value: '/bin/zsh' },
		{ label: 'PowerShell', value: 'powershell' },
		{ label: 'CMD', value: 'cmd' }
	];

	// The full-pane file manager's Edit (F4): which program opens files.
	let editors = $state<EditorInfo[]>([]);
	let editorsLooked = $state(false);
	$effect(() => {
		if (part !== 'shell' || isMobile() || editorsLooked) return;
		editorsLooked = true;
		void detectEditors().then((found) => (editors = found));
	});
	// WinBox 4 sessions: where the program is, as Settings would start it.
	let winboxFound = $state<string | null | undefined>(undefined);
	$effect(() => {
		if (part !== 'sessions' || isMobile()) return;
		const path = settings.winboxPath;
		void winboxFind(path).then((found) => (winboxFound = found), () => (winboxFound = null));
	});
	const winboxPasswordOptions = $derived<{ label: string; value: WinboxPasswordMode }[]>([
		{ label: t('settings.winbox_password_argument'), value: 'argument' },
		{ label: t('settings.winbox_password_clipboard'), value: 'clipboard' },
		{ label: t('settings.winbox_password_none'), value: 'none' }
	]);
	async function browseWinbox(): Promise<void> {
		try {
			const picked = await openDialog({ multiple: false, directory: false, title: t('settings.winbox_path') });
			if (typeof picked === 'string') updateSetting('winboxPath', picked);
		} catch {
			// Cancelled.
		}
	}

	let editorOptions = $derived([
		{ label: t('settings.file_editor_system'), value: '' },
		...editors.map((e) => ({ label: e.terminal ? `${e.name} (${t('settings.file_editor_in_terminal')})` : e.name, value: e.id })),
		// Picked before and not found now: still shown, so the choice is not
		// silently swapped (opening falls back to the system's program).
		...(settings.fileEditor && settings.fileEditor !== 'custom' && editorsLooked && !editors.some((e) => e.id === settings.fileEditor)
			? [{ label: t('settings.file_editor_missing', { name: settings.fileEditor }), value: settings.fileEditor }]
			: []),
		{ label: t('settings.file_editor_custom'), value: 'custom' }
	]);

	function onShellChange(value: string) {
		updateSetting('defaultShell', value);
	}

	function onPasteWarningChange(checked: boolean) {
		updateSetting('warnOnMultilinePaste', checked);
	}

	/**
	 * A threshold below the minimum would stop a one-word paste, which
	 * reads as a bug rather than a setting, so the field settles to the
	 * lowest value that means anything. Turning the warning off is what the
	 * toggle above is for.
	 */
	function onPasteThresholdChange(e: Event & { currentTarget: HTMLInputElement }) {
		const typed = Number(e.currentTarget.value);
		const next = effectiveThreshold(typed);
		e.currentTarget.value = String(next);
		updateSetting('multilinePasteThreshold', next);
	}

	let rightClickOptions = $derived([
		{ label: t('settings.term_right_click_menu'), value: 'menu' },
		{ label: t('settings.term_right_click_paste'), value: 'paste' },
		{ label: t('settings.term_right_click_copy_paste'), value: 'copy-paste' }
	]);

	let tmuxWindowOptions = $derived([
		{ label: t('settings.tmux_windows_last'), value: 'last' },
		{ label: t('settings.tmux_windows_all'), value: 'all' }
	]);

	function onLastSessionChange(checked: boolean) {
		updateSetting('openLastSession', checked);
	}

	/** Turning bell notifications on asks the system for permission first;
	 *  refused, the switch stays off and says why. */
	async function onBellNotifyChange(checked: boolean) {
		if (checked && !(await allowNotifications())) {
			updateSetting('terminalBellNotify', false);
			addToast(t('settings.bell_notify_denied'), 'warning', 6000);
			return;
		}
		updateSetting('terminalBellNotify', checked);
	}

	function onCheckForUpdatesChange(checked: boolean) {
		updateSetting('checkForUpdates', checked);
	}

	/** Check now: works with automatic checks off too. A newer release opens
	 *  the usual update banner; otherwise a toast says how it went. */
	let checkingUpdates = $state(false);
	async function onCheckNow() {
		if (checkingUpdates) return;
		checkingUpdates = true;
		try {
			const { found, error } = await manualUpdateCheck();
			if (error) addToast(t('settings.update_check_failed', { error }), 'error');
			else if (!found) addToast(t('settings.update_up_to_date'), 'success');
		} finally {
			checkingUpdates = false;
		}
	}

	async function onStartWithSystemChange(checked: boolean) {
		if (checked) {
			await enable();
		} else {
			await disable();
		}
		updateSetting('startWithSystem', checked);
	}
</script>

<div class="tab-content">

	{#if part === 'general'}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.language')}</span>
		</div>
		<div class="setting-control">
			<LanguageSelect value={settings.locale} onchange={onLanguageChange} />
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.startup_behavior')}</span>
			<span class="setting-description">{t('settings.restore_tabs_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.openLastSession}
				label={t('settings.open_last_session')}
				onchange={onLastSessionChange}
			/>
		</div>
	</div>

	{#if !isMobile()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.minimize_to_tray')}</span>
				<span class="setting-description">{t('settings.tray_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.minimizeToTray}
					label={t('settings.minimize_to_tray')}
					onchange={(checked) => updateSetting('minimizeToTray', checked)}
				/>
			</div>
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.start_with_system')}</span>
				<span class="setting-description">{t('settings.system_startup_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.startWithSystem}
					label={t('settings.start_with_system')}
					onchange={onStartWithSystemChange}
				/>
			</div>
		</div>
	{/if}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.play_sounds')}</span>
			<span class="setting-description">{t('settings.play_sounds_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.playSounds}
				label={t('settings.play_sounds')}
				onchange={(v) => updateSetting('playSounds', v)}
			/>
		</div>
	</div>

	<!-- The first-run questions again, over whatever is open. -->
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.rerun_setup')}</span>
			<span class="setting-description">{t('settings.rerun_setup_desc')}</span>
		</div>
		<div class="setting-control">
			<button class="discord-link" onclick={openSetupWizard}>
				<span>{t('settings.rerun_setup_button')}</span>
			</button>
		</div>
	</div>

	{/if}

	{#if part === 'shell'}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.default_shell')}</span>
			<span class="setting-description">{t('settings.shell_desc')}</span>
		</div>
		<div class="setting-control">
			<Dropdown
				options={shellOptions}
				selected={settings.defaultShell}
				onchange={onShellChange}
			/>
		</div>
	</div>

	{#if !isMobile()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.file_editor')}</span>
				<span class="setting-description">{t('settings.file_editor_desc')}</span>
			</div>
			<div class="setting-control">
				<Dropdown options={editorOptions} selected={settings.fileEditor} onchange={(v) => updateSetting('fileEditor', v)} />
			</div>
		</div>
		{#if settings.fileEditor === 'custom'}
			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{t('settings.file_editor_command')}</span>
					<span class="setting-description">{t('settings.file_editor_command_desc')}</span>
				</div>
				<div class="setting-control separators">
					<input
						class="threshold-input command-input"
						type="text"
						spellcheck="false"
						autocomplete="off"
						autocapitalize="off"
						placeholder="subl -w"
						aria-label={t('settings.file_editor_command')}
						value={settings.fileEditorCommand}
						onchange={(e) => updateSetting('fileEditorCommand', e.currentTarget.value.trim())}
					/>
				</div>
			</div>
			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{t('settings.file_editor_terminal')}</span>
					<span class="setting-description">{t('settings.file_editor_terminal_desc')}</span>
				</div>
				<div class="setting-control">
					<Toggle hideLabel checked={settings.fileEditorTerminal} label={t('settings.file_editor_terminal')} onchange={(v) => updateSetting('fileEditorTerminal', v)} />
				</div>
			</div>
		{/if}
	{/if}

	{/if}

	{#if part === 'bell'}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.bell_sound')}</span>
			<span class="setting-description">{t('settings.bell_sound_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.terminalBellSound}
				label={t('settings.bell_sound')}
				onchange={(v) => updateSetting('terminalBellSound', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.bell_flash')}</span>
			<span class="setting-description">{t('settings.bell_flash_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.terminalBellFlash}
				label={t('settings.bell_flash')}
				onchange={(v) => updateSetting('terminalBellFlash', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.bell_notify')}</span>
			<span class="setting-description">{t('settings.bell_notify_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.terminalBellNotify}
				label={t('settings.bell_notify')}
				onchange={onBellNotifyChange}
			/>
		</div>
	</div>

	{/if}

	{#if part === 'paste'}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.paste_warning')}</span>
			<span class="setting-description">{t('settings.paste_warning_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.warnOnMultilinePaste}
				label={t('settings.paste_warning')}
				onchange={onPasteWarningChange}
			/>
		</div>
	</div>

	<!-- Belongs to the switch above, so it sits under it, indented. -->
	{#if settings.warnOnMultilinePaste}
		<div class="setting-row sub-setting">
			<div class="setting-info">
				<span class="setting-label">{t('settings.paste_threshold')}</span>
				<span class="setting-description">{t('settings.paste_threshold_desc')}</span>
			</div>
			<div class="setting-control">
				<input
					class="threshold-input"
					type="number"
					min={MIN_PASTE_THRESHOLD}
					step="1"
					value={settings.multilinePasteThreshold}
					onchange={onPasteThresholdChange}
				/>
			</div>
		</div>
	{/if}

	<!-- The mouse in the terminal; a phone has none. -->

	{#if !isMobile()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.term_right_click')}</span>
				<span class="setting-description">{t('settings.term_right_click_desc')}</span>
			</div>
			<div class="setting-control">
				<Dropdown
					options={rightClickOptions}
					selected={settings.terminalRightClick}
					onchange={(v) => updateSetting('terminalRightClick', rightClickMode(v))}
				/>
			</div>
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.term_copy_on_select')}</span>
				<span class="setting-description">{t('settings.term_copy_on_select_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.terminalCopyOnSelect}
					label={t('settings.term_copy_on_select')}
					onchange={(v) => updateSetting('terminalCopyOnSelect', v)}
				/>
			</div>
		</div>
	{/if}

	{/if}

	{#if part === 'paste-adv'}

	{#if !isMobile()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.term_osc52')}</span>
				<span class="setting-description">{t('settings.term_osc52_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.terminalOsc52}
					label={t('settings.term_osc52')}
					onchange={(v) => updateSetting('terminalOsc52', v)}
				/>
			</div>
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.term_middle_click')}</span>
				<span class="setting-description">{t('settings.term_middle_click_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.terminalMiddleClickPaste}
					label={t('settings.term_middle_click')}
					onchange={(v) => updateSetting('terminalMiddleClickPaste', v)}
				/>
			</div>
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.term_word_separators')}</span>
				<span class="setting-description">{t('settings.term_word_separators_desc')}</span>
			</div>
			<div class="setting-control separators">
				<input
					class="threshold-input separators-input"
					type="text"
					spellcheck="false"
					autocomplete="off"
					autocapitalize="off"
					aria-label={t('settings.term_word_separators')}
					value={settings.terminalWordSeparators}
					oninput={(e) => updateSetting('terminalWordSeparators', e.currentTarget.value)}
				/>
				{#if settings.terminalWordSeparators !== DEFAULT_WORD_SEPARATORS}
					<button class="reset-link" onclick={() => updateSetting('terminalWordSeparators', DEFAULT_WORD_SEPARATORS)}>
						{t('settings.term_word_separators_reset')}
					</button>
				{/if}
			</div>
		</div>
	{/if}

	{/if}

	{#if part === 'sessions'}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.double_click_open')}</span>
			<span class="setting-description">{t('settings.double_click_open_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.doubleClickToOpen}
				label={t('settings.double_click_open')}
				onchange={(v) => updateSetting('doubleClickToOpen', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.show_unsupported_sessions')}</span>
			<span class="setting-description">{t('settings.show_unsupported_sessions_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.showUnsupportedSessions}
				label={t('settings.show_unsupported_sessions')}
				onchange={(v) => updateSetting('showUnsupportedSessions', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.close_tab_on_exit')}</span>
			<span class="setting-description">{t('settings.close_tab_on_exit_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.closeTabOnExit}
				label={t('settings.close_tab_on_exit')}
				onchange={(v) => updateSetting('closeTabOnExit', v)}
			/>
		</div>
	</div>

	{#if canFloat()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.reopen_windows')}</span>
				<span class="setting-description">{t('settings.reopen_windows_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.reopenFloatWindows}
					label={t('settings.reopen_windows')}
					onchange={(v) => updateSetting('reopenFloatWindows', v)}
				/>
			</div>
		</div>
	{/if}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.server_monitoring')}</span>
			<span class="setting-description">{t('settings.server_monitoring_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.serverMonitoring}
				label={t('settings.server_monitoring')}
				onchange={(v) => updateSetting('serverMonitoring', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.check_addresses')}</span>
			<span class="setting-description">{t('settings.check_addresses_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.checkAddresses}
				label={t('settings.check_addresses')}
				onchange={(v) => updateSetting('checkAddresses', v)}
			/>
		</div>
	</div>

	<!-- WinBox 4 is a program on this computer: desktop only. -->
	{#if !isMobile()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.winbox_path')}</span>
				<span class="setting-description">
					{#if winboxFound}
						{t('settings.winbox_found', { path: winboxFound })}
					{:else if winboxFound === null}
						{settings.winboxPath ? t('settings.winbox_path_bad') : t('settings.winbox_not_found')}
					{:else}
						{t('settings.winbox_path_desc')}
					{/if}
				</span>
			</div>
			<div class="setting-control separators">
				<input
					class="threshold-input command-input"
					type="text"
					spellcheck="false"
					autocomplete="off"
					autocapitalize="off"
					placeholder={t('settings.winbox_path_auto')}
					aria-label={t('settings.winbox_path')}
					value={settings.winboxPath}
					onchange={(e) => updateSetting('winboxPath', e.currentTarget.value.trim())}
				/>
				<button type="button" class="reset-link" onclick={browseWinbox}>{t('settings.winbox_browse')}</button>
			</div>
		</div>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.winbox_password')}</span>
				<span class="setting-description">{t('settings.winbox_password_desc')}</span>
			</div>
			<div class="setting-control">
				<Dropdown options={winboxPasswordOptions} selected={settings.winboxPassword} onchange={(v) => updateSetting('winboxPassword', v as WinboxPasswordMode)} />
			</div>
		</div>
	{/if}

	<!-- A phone has no X server to point at: desktop only. -->

	{#if !isMobile()}
		<X11ServerSetting />
	{/if}

	{/if}

	{#if part === 'tmux'}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.tmux_new_tab')}</span>
			<span class="setting-description">{t('settings.tmux_new_tab_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.tmuxNewTabWindow}
				label={t('settings.tmux_new_tab')}
				onchange={(v) => updateSetting('tmuxNewTabWindow', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.tmux_connect_windows')}</span>
			<span class="setting-description">{t('settings.tmux_connect_windows_desc')}</span>
		</div>
		<div class="setting-control">
			<Dropdown
				options={tmuxWindowOptions}
				selected={settings.tmuxConnectWindows}
				onchange={(v) => updateSetting('tmuxConnectWindows', v === 'all' ? 'all' : 'last')}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.tmux_hide_status')}</span>
			<span class="setting-description">{t('settings.tmux_hide_status_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.tmuxHideStatus}
				label={t('settings.tmux_hide_status')}
				onchange={(v) => updateSetting('tmuxHideStatus', v)}
			/>
		</div>
	</div>

	{/if}

	{#if part === 'updates'}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.version')}</span>
		</div>
		<div class="setting-control"><span class="version">v{__APP_VERSION__}</span></div>
	</div>

	{#if updater.updateAvailable && updater.updateVersion}
		<!-- The same What's new dialog as the update banner's Info button. -->
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('updater.available', { version: updater.updateVersion })}</span>
			</div>
			<div class="setting-control">
				<button class="discord-link" onclick={openWhatsNew}>
					<span>{t('updater.whats_new')}</span>
				</button>
			</div>
		</div>
	{/if}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.check_updates')}</span>
			<span class="setting-description">{t('settings.check_updates_desc')}</span>
		</div>
		<div class="setting-control update-controls">
			<button class="discord-link" onclick={onCheckNow} disabled={checkingUpdates}>
				<span>{checkingUpdates ? t('settings.check_updates_checking') : t('settings.check_updates_now')}</span>
			</button>
			<Toggle
				hideLabel
				checked={settings.checkForUpdates}
				label={t('settings.check_updates')}
				onchange={onCheckForUpdatesChange}
			/>
		</div>
	</div>

	{/if}

	{#if part === 'about'}

	<!-- Website, changelog and license, opened in the system browser
	     (on phones too, through the opener plugin). -->
	{#each [
		{ label: 'settings.website', desc: 'settings.website_desc', url: WEBSITE_URL },
		{ label: 'settings.changelog', desc: 'settings.changelog_desc', url: CHANGELOG_URL },
		{ label: 'settings.license', desc: 'settings.license_desc', url: LICENSE_URL }
	] as link (link.label)}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t(link.label)}</span>
				<span class="setting-description">{t(link.desc)}</span>
			</div>
			<div class="setting-control">
				<button class="discord-link" title={link.url} onclick={() => void openUrl(link.url)}>
					<span>{t('settings.open_link')}</span>
					<span class="ext"><FaIcon icon={faUpRightFromSquare} size={10} /></span>
				</button>
			</div>
		</div>
	{/each}

	{#if SHOW_COMMUNITY}
		<!-- The permanent route to the community. The launch prompt can be
		     dismissed forever; this is what makes that safe to do. -->
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.community')}</span>
				<span class="setting-description">{t('settings.community_desc')}</span>
			</div>
			<div class="setting-control">
				<!-- One control, two ways out of it: the body opens the invite, the
				     glyph on the right copies it. A second button beside the first
				     read as two half-width controls where every other row has one. -->
				<button
					class="discord-link"
					title={COMMUNITY.discordInvite}
					onclick={() => openUrl(COMMUNITY.discordInvite)}
				>
					<DiscordIcon size={14} />
					<span>{t('settings.open_discord')}</span>
					<span class="ext"><FaIcon icon={faUpRightFromSquare} size={10} /></span>
					<!-- Nested interactive content is invalid inside a <button>, so the
					     copy affordance is a span with the keyboard behaviour written
					     out: Enter and Space, and a tab stop of its own. -->
					<span
						class="copy"
						role="button"
						tabindex="0"
						title={t('common.copy')}
						aria-label={t('common.copy')}
						onclick={async (e) => {
							// Without this the click reaches the parent and the browser
							// opens as well as the clipboard being written.
							e.stopPropagation();
							await navigator.clipboard.writeText(COMMUNITY.discordInvite);
							addToast(t('vault.copied_toast'), 'success');
						}}
						onkeydown={async (e) => {
							if (e.key !== 'Enter' && e.key !== ' ') return;
							e.preventDefault();
							e.stopPropagation();
							await navigator.clipboard.writeText(COMMUNITY.discordInvite);
							addToast(t('vault.copied_toast'), 'success');
						}}
					>
						<FaIcon icon={faCopy} size={12} />
					</span>
				</button>
			</div>
		</div>
	{/if}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('report.title')}</span>
			<span class="setting-description">{t('report.desc')}</span>
		</div>
		<div class="setting-control">
			<button class="discord-link" onclick={() => (reportOpen = true)}>
				<FaIcon icon={faBug} size={12} />
				<span>{t('report.open')}</span>
			</button>
		</div>
	</div>
	<ReportProblem open={reportOpen} onclose={() => (reportOpen = false)} />

	{#if SHOW_SPONSOR}
		<!-- Not a nag: a row that exists, once, where someone who wants to give
		     back can find it. No prompt, no badge, no reminder. -->
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.support')}</span>
				<span class="setting-description">{t('settings.support_desc')}</span>
			</div>
			<div class="setting-control">
				<button
					class="discord-link sponsor-link"
					title={COMMUNITY.sponsorLabel}
					onclick={() => openUrl(COMMUNITY.sponsorUrl)}
				>
					<svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
						<path
							d="M4.25 2.5c-1.336 0-2.75 1.164-2.75 3 0 2.15 1.58 4.144 3.365 5.682A20.6 20.6 0 0 0 8 13.393a20.6 20.6 0 0 0 3.135-2.211C12.92 9.644 14.5 7.65 14.5 5.5c0-1.836-1.414-3-2.75-3-1.373 0-2.609.986-3.029 2.456a.75.75 0 0 1-1.442 0C6.859 3.486 5.623 2.5 4.25 2.5ZM8 14.25l-.345.666a.75.75 0 0 0 .69 0L8 14.25Z"
						/>
					</svg>
					<span>{t('settings.sponsor')}</span>
					<span class="ext"><FaIcon icon={faUpRightFromSquare} size={10} /></span>
				</button>
			</div>
		</div>
	{/if}

	{/if}

</div>

<style>
	/* The dropdowns' box: it was the page's black, the one dark well in a
	   column of grey controls. */
	.version {
		font-family: var(--font-mono, monospace);
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.update-controls {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.threshold-input {
		padding: 9px 12px;
		border-radius: var(--radius-btn);
		border: 1px solid var(--color-border-strong);
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
		font-family: inherit;
		font-size: 0.8125rem;
	}

	.sub-setting {
		margin-left: 12px;
		padding-left: 14px;
		border-left: 2px solid var(--color-border);
	}

	.separators {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 4px;
	}

	.command-input {
		width: 100%;
		font-family: var(--font-mono, monospace);
	}

	.separators-input {
		width: 100%;
		font-family: var(--font-mono, monospace);
		letter-spacing: 0.15em;
	}

	.reset-link {
		padding: 0;
		border: none;
		background: none;
		font-family: inherit;
		font-size: 0.75rem;
		color: var(--color-accent);
		cursor: pointer;
	}

	.reset-link:hover {
		text-decoration: underline;
	}

	.threshold-input:focus {
		outline: none;
		border-color: var(--color-accent);
	}


	/* The label takes the space; the two glyphs sit at the far edge. */
	.discord-link > span:not(.ext):not(.copy) {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	/* The glyph that says "this leaves the app". */
	.ext {
		display: inline-flex;
		margin-left: auto;
		color: var(--color-text-tertiary);
	}

	.discord-link:hover .ext {
		color: inherit;
	}

	/* Spacing, not a rule. A hairline here drew the eye to the divider rather
	   than to either glyph, which is the thing it was meant to stop doing. */
	.copy {
		display: inline-flex;
		align-items: center;
		padding: 3px;
		margin-left: 6px;
		border-radius: 3px;
		color: var(--color-text-tertiary);
		transition:
			color 0.15s,
			background-color 0.15s;
	}

	/* The hover tint is what separates it now: nothing until you reach for it. */
	.copy:hover {
		color: var(--color-text-primary);
		background: var(--color-surface-hover);
	}

	.copy:focus-visible {
		outline: 2px solid var(--color-accent);
		outline-offset: 1px;
	}

	/* Full column width, like the fields above: one edge for every control. */
	.discord-link {
		display: inline-flex;
		align-items: center;
		width: 100%;
		gap: 8px;
		padding: 6px 12px;
		font-family: inherit;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
		background: var(--color-surface-sunken, transparent);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		cursor: pointer;
		transition: color 0.15s, border-color 0.15s;
	}

	.discord-link:hover {
		color: #5865f2;
		border-color: color-mix(in srgb, #5865f2 50%, var(--color-border));
	}

	/* GitHub Sponsors' own pink, the way the Discord row takes blurple. */
	.sponsor-link:hover {
		color: #ea4aaa;
		border-color: color-mix(in srgb, #ea4aaa 50%, var(--color-border));
	}
</style>
