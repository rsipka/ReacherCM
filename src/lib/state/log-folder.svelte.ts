import { open } from '@tauri-apps/plugin-dialog';
import { sessionLogFolderExists, type SessionLogConfig } from '#lib/ipc/ssh.js';
import { askChoice } from '#lib/state/ask.svelte.js';
import { t } from '#lib/state/i18n.svelte.js';
import { getSyncAccount, preferenceSync, setPreferenceSyncIncluded } from '#lib/state/preference-sync.svelte.js';
import { getSettings, sessionLogConfig, updateSetting } from '#lib/state/settings.svelte.js';
import { addToast, removeToast } from '#lib/state/toasts.svelte.js';

type Source = 'local' | 'account';
const warnings: Partial<Record<Source, { path: string; id: string }>> = {};

export async function logFolderExists(path: string): Promise<boolean> {
	return !path.trim() || await sessionLogFolderExists(path).catch(() => false);
}

export function clearLogFolderWarning(source: Source): void {
	const warning = warnings[source];
	if (warning) removeToast(warning.id);
	delete warnings[source];
}

export function warnAboutLogFolder(path: string, source: Source, current: () => boolean): void {
	if (!current() || warnings[source]?.path === path) return;
	clearLogFolderWarning(source);
	const message = t(source === 'account' ? 'settings.log_folder_account_missing' : 'settings.log_folder_missing', { path });
	const toast = addToast(message, 'warning', 0, {
		label: t('settings.log_folder_review'),
		run: () => void reviewLogFolder(message, current),
	});
	warnings[source] = { path, id: toast.id };
}

async function reviewLogFolder(message: string, current: () => boolean): Promise<void> {
	if (!current()) return;
	const account = getSyncAccount();
	const canExclude = account && !preferenceSync.skipped.includes('sessionLogFolder');
	const choice = await askChoice(message, {
		confirmLabel: t('settings.log_browse'),
		confirmVariant: 'primary',
		otherLabel: canExclude ? t('sync_account.preference_keep_local') : undefined,
	});
	if (!current()) return;
	if (choice === 'other' && getSyncAccount() === account) {
		setPreferenceSyncIncluded('sessionLogFolder', false);
		clearLogFolderWarning('account');
	} else if (choice === 'confirm') {
		const folder = await open({ directory: true }).catch(() => null);
		if (typeof folder === 'string' && current()) updateSetting('sessionLogFolder', folder, true);
	}
}

async function checkLocalFolder(path: string): Promise<boolean> {
	const current = () => sessionLogConfig()?.folder === path;
	const valid = await logFolderExists(path);
	if (!current()) return false;
	if (valid) clearLogFolderWarning('local');
	else warnAboutLogFolder(path, 'local', current);
	return valid;
}

export function checkConfiguredLogFolder(): void {
	const path = sessionLogConfig()?.folder;
	if (path) void checkLocalFolder(path);
	else clearLogFolderWarning('local');
}

export async function validatedSessionLogConfig(session = ''): Promise<SessionLogConfig | null> {
	const config = sessionLogConfig(session);
	return config && (!config.folder || await checkLocalFolder(config.folder)) ? config : null;
}
