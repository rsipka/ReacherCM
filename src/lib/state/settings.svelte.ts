import { invoke } from '@tauri-apps/api/core';
import * as settingsIpc from '#lib/ipc/settings.js';
import { isMobile } from '#lib/platform.js';
import type { SessionLogConfig } from '#lib/ipc/ssh.js';
import { defaults, readStoredSettings, storeSettings, STORAGE_KEY, type Settings } from '#lib/state/settings-store.js';
export { DEFAULT_LOG_NAME } from '#lib/state/settings-store.js';
import { decodePreferences, encodePreferences, type Preferences } from '#lib/sync/preference-catalog.js';
import { NEW_FEATURE_IDS } from '#lib/data/whats-new.js';

export type { Settings };

/** Secure settings stored encrypted in vault (API keys etc.) */
export interface SecureSettings {
	openrouterApiKey: string | null;
	openrouterUrl: string | null;
	defaultAiModel: string | null;
}


let settings = $state<Settings>({ ...defaults });

// Secure settings (API keys) - stored encrypted in vault
const secureDefaults: SecureSettings = {
	openrouterApiKey: null,
	openrouterUrl: null,
	defaultAiModel: null
};
let secureSettings = $state<SecureSettings>({ ...secureDefaults });
let secureLoading = $state(false);
let secureError = $state<string | null>(null);

export function getSettings(): Settings {
	return settings;
}

/**
 * The session log to ask `ssh_connect` for, or null when logging is off.
 * `session` is the saved session's name, for `&S` in the file name.
 */
export function sessionLogConfig(session = ''): SessionLogConfig | null {
	if (settings.sessionLogMode === 'off') return null;
	return {
		mode: settings.sessionLogMode,
		folder: isMobile() ? '' : settings.sessionLogFolder.trim(),
		name: settings.sessionLogName.trim(),
		session: session.trim(),
		append: settings.sessionLogAppend,
		header: settings.sessionLogHeader
	};
}

let onPortableChange: ((values: Preferences) => void) | null = null;

export function onPortableSettingChanged(listener: (values: Preferences) => void): () => void {
	onPortableChange = listener;
	return () => { if (onPortableChange === listener) onPortableChange = null; };
}

export function updateSetting<K extends keyof Settings>(key: K, value: Settings[K], syncUnchanged = false): void {
	const changed = settings[key] !== value;
	settings[key] = value;
	if ((changed || syncUnchanged) && onPortableChange) {
		const portable = encodePreferences({ [key]: value });
		if (Object.keys(portable).length > 0) onPortableChange(portable);
	}
	saveSettings();
}

export function applyPortableSettings(values: Preferences): void {
	const decoded = decodePreferences(values);
	const keys = Object.keys(decoded) as (keyof typeof decoded)[];
	if (keys.length === 0) return;
	const normalized = readStoredSettings({ ...settings, ...decoded });
	let changed = false;
	const previous = encodePreferences(settings);
	const next = encodePreferences(normalized);
	for (const key of keys) {
		if (previous[key] !== next[key]) {
			Object.assign(settings, { [key]: normalized[key] });
			changed = true;
		}
	}
	if (changed) saveSettings();
}

export function loadSettings(): void {
	if (typeof localStorage === 'undefined') return;

	try {
		// Reach 0.1.0 kept the passwords "Remember password" saved here, in
		// the clear. 0.1.1 moved them to the vault and stopped reading this
		// key, but left what was in it; nothing uses it, so it goes.
		localStorage.removeItem('reach_saved_passwords');

		const stored = localStorage.getItem(STORAGE_KEY);

		// No stored settings at all means this is a first run. Everything in the
		// what's-new list is marked seen, because all of it is new to someone
		// who has never opened Reach, and highlighting an arbitrary three of it
		// is noise rather than news.
		if (!stored) {
			settings.seenFeatures = [...NEW_FEATURE_IDS];
			saveSettings();
			return;
		}

		if (stored) {
			const parsed = JSON.parse(stored) as Partial<Settings>;
			// Earlier versions wrote the pending Turso token here in the clear; the
			// next save leaves it out, so save once now to take it off the disk.
			if (parsed.pendingTursoApiToken) queueMicrotask(saveSettings);
			// The token stays as it is: it is never stored.
			Object.assign(settings, readStoredSettings(parsed), { pendingTursoApiToken: settings.pendingTursoApiToken });
		}
	} catch {
		// If parsing fails, keep defaults
	}
}

/**
 * Record that the user has been shown one or more new features.
 *
 * Additive and idempotent: marking something already seen is a no-op, so a
 * component can call this on every render of the thing without churning
 * localStorage.
 */
export function markFeaturesSeen(ids: string[]): void {
	const fresh = ids.filter((id) => !settings.seenFeatures.includes(id));
	if (fresh.length === 0) return;
	settings.seenFeatures = [...settings.seenFeatures, ...fresh];
	saveSettings();
}

/** Launches before the community prompt is allowed to appear at all. */
const PROMPT_AFTER_LAUNCHES = 3;

/** How long "Not now" buys, in days. */
const PROMPT_SNOOZE_DAYS = 21;

/**
 * Count this launch. Called once, from the root layout.
 *
 * Separate from the prompt check so the count keeps rising even when the
 * prompt can never show — otherwise turning the prompt back on would start
 * someone from zero again.
 */
export function recordLaunch(): void {
	settings.launches += 1;
	saveSettings();
}

/**
 * Whether to show the community prompt on this launch.
 *
 * Deliberately not on first run. Someone who has opened Reach once has not
 * decided whether they like it yet, and asking them to join a chat server is
 * the fastest way to make the answer no.
 */
export function shouldShowCommunityPrompt(): boolean {
	if (settings.communityPromptDismissed) return false;
	if (!settings.setupComplete) return false;
	if (settings.launches < PROMPT_AFTER_LAUNCHES) return false;

	const snooze = PROMPT_SNOOZE_DAYS * 24 * 60 * 60 * 1000;
	return Date.now() - settings.communityPromptLastShown > snooze;
}

/** "Not now" — defer, do not silence. */
export function snoozeCommunityPrompt(): void {
	settings.communityPromptLastShown = Date.now();
	saveSettings();
}

/** The checkbox, and also what joining does: neither wants asking again. */
export function dismissCommunityPrompt(): void {
	settings.communityPromptDismissed = true;
	settings.communityPromptLastShown = Date.now();
	saveSettings();
}

/** Told after every save: other windows read the settings again (#lib/state/windows). */
let afterSave: (() => void) | null = null;

export function onSettingsSaved(fn: () => void): void {
	afterSave = fn;
}

export function saveSettings(): void {
	if (typeof localStorage === 'undefined') return;

	try {
		localStorage.setItem(STORAGE_KEY, storeSettings(settings));
		afterSave?.();
	} catch {
		// Storage might be full or unavailable
	}
}

// --- Secure Settings (API keys in encrypted vault) ---

/** Get secure settings (read-only). */
export function getSecureSettings(): SecureSettings {
	return secureSettings;
}

/** Check if secure settings are loading. */
export function isSecureLoading(): boolean {
	return secureLoading;
}

/** Get secure settings error. */
export function getSecureError(): string | null {
	return secureError;
}

/** Check if API key is configured. */
export function hasApiKey(): boolean {
	return !!secureSettings.openrouterApiKey;
}

/** Get OpenRouter API key. */
export function getOpenRouterApiKey(): string | null {
	return secureSettings.openrouterApiKey;
}

/** Get OpenRouter URL (with default). */
export function getOpenRouterUrl(): string {
	return secureSettings.openrouterUrl ?? 'https://openrouter.ai/api/v1/chat/completions';
}

/** Get default AI model (with default). */
export function getDefaultAiModel(): string {
	return secureSettings.defaultAiModel ?? 'anthropic/claude-3.5-sonnet';
}

/** Load secure settings from vault. Call after vault unlock. */
export async function loadSecureSettings(): Promise<void> {
	secureLoading = true;
	secureError = null;

	try {
		const loaded = await settingsIpc.getAll();
		secureSettings.openrouterApiKey = loaded.openrouterApiKey;
		secureSettings.openrouterUrl = loaded.openrouterUrl;
		secureSettings.defaultAiModel = loaded.defaultAiModel;
	} catch (e) {
		secureError = e instanceof Error ? e.message : String(e);
	} finally {
		secureLoading = false;
	}
}

/** Set OpenRouter API key. O(1). */
export async function setOpenRouterApiKey(apiKey: string): Promise<void> {
	await settingsIpc.set(settingsIpc.SETTING_KEYS.OPENROUTER_API_KEY, apiKey);
	secureSettings.openrouterApiKey = apiKey;
}

/** Set OpenRouter URL. O(1). */
export async function setOpenRouterUrl(url: string): Promise<void> {
	await settingsIpc.set(settingsIpc.SETTING_KEYS.OPENROUTER_URL, url);
	secureSettings.openrouterUrl = url;
}

/** Set default AI model. O(1). */
export async function setDefaultAiModel(model: string): Promise<void> {
	await settingsIpc.set(settingsIpc.SETTING_KEYS.DEFAULT_AI_MODEL, model);
	secureSettings.defaultAiModel = model;
}

/** Clear secure settings (on vault lock). */
export function clearSecureSettings(): void {
	secureSettings.openrouterApiKey = null;
	secureSettings.openrouterUrl = null;
	secureSettings.defaultAiModel = null;
}

/** Sync minimizeToTray setting to Rust backend AtomicBool. Call after loadSettings(). */
export async function syncTraySettings(): Promise<void> {
	try {
		await invoke('set_close_to_tray', { enabled: settings.minimizeToTray });
	} catch {
		// Backend not ready yet
	}
}

/** Set by a backup import: the next unlock takes the backup's settings. */
const RESTORE_PENDING_KEY = 'reach-restore-settings';

/** Call once a backup has been imported, before the app restarts. */
export function markSettingsRestorePending(): void {
	try {
		localStorage.setItem(RESTORE_PENDING_KEY, '1');
	} catch {
		// Storage unavailable: the backup's settings are not applied
	}
}

/**
 * Restore local settings from vault AppSettings, once, after a backup import
 * and relaunch. Only then: the vault holds a copy taken when a backup was
 * last exported, and applying it on every unlock put the font, language,
 * shell and tray choices back to that copy at each start.
 */
export async function restoreLocalSettingsFromVault(): Promise<void> {
	try {
		if (localStorage.getItem(RESTORE_PENDING_KEY) !== '1') return;
		const vaultSettings = await invoke<Record<string, unknown>>('vault_get_settings');
		localStorage.removeItem(RESTORE_PENDING_KEY);
		let changed = false;
		if (typeof vaultSettings.minimizeToTray === 'boolean') {
			settings.minimizeToTray = vaultSettings.minimizeToTray;
			changed = true;
		}
		if (typeof vaultSettings.startWithSystem === 'boolean') {
			settings.startWithSystem = vaultSettings.startWithSystem;
			changed = true;
		}
		if (typeof vaultSettings.defaultShell === 'string') {
			settings.defaultShell = vaultSettings.defaultShell;
			changed = true;
		}
		if (typeof vaultSettings.openLastSession === 'boolean') {
			settings.openLastSession = vaultSettings.openLastSession;
			changed = true;
		}
		if (typeof vaultSettings.fontSize === 'number') {
			settings.fontSize = vaultSettings.fontSize;
			changed = true;
		}
		if (typeof vaultSettings.fontFamily === 'string') {
			settings.fontFamily = vaultSettings.fontFamily;
			changed = true;
		}
		if (typeof vaultSettings.locale === 'string') {
			settings.locale = vaultSettings.locale;
			changed = true;
		}
		if (changed) {
			saveSettings();
			await syncTraySettings();
		}
	} catch {
		// Vault not unlocked or settings not available
	}
}
