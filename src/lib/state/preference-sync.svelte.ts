import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getSettings, onPortableSettingChanged, applyPortableSettings } from '#lib/state/settings.svelte.js';
import { DEFAULT_SKIPPED_KEYS, encodePreferences } from '#lib/sync/preference-catalog.js';
import { defaults } from '#lib/state/settings-store.js';
import {
	incomingPreferences, portablePreferences, readPreferenceJournal, reconcilePreferences, setPreferenceIncluded, supportedPreferenceKeys,
	PREFERENCE_KEYS, type PreferenceJournal, type PreferenceKey, type PreferencePatch, type PreferenceSnapshot,
} from '#lib/sync/preferences.js';

export interface SyncAccount {
	server: string;
	username: string;
	deviceId: string;
}

interface Journal extends PreferenceJournal {
	scope: string;
	enabled: boolean;
}

const STORAGE_KEY = 'reachercm-preference-sync';
const POLL_INTERVAL = 30_000;
let current = $state<SyncAccount | null>(null);
let journal: Journal | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let running = false;
let accountRevision = 0;
let resync = false;

export const preferenceSync = $state({
	enabled: false, skipped: DEFAULT_SKIPPED_KEYS as readonly PreferenceKey[], supported: null as readonly string[] | null, syncing: false, error: '', online: true,
});

export function getSyncAccount(): SyncAccount | null {
	return current;
}

function scope(account: SyncAccount | null): string {
	return account ? JSON.stringify([account.server, account.username, account.deviceId]) : '';
}

function readJournal(): Journal | null {
	try {
		const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null') as Partial<Journal> | null;
		return value && typeof value.scope === 'string'
			// Existing journals were written only when sync was enabled.
			? { ...readPreferenceJournal(value), scope: value.scope, enabled: value.enabled !== false }
			: null;
	} catch {
		return null;
	}
}

function persist(): void {
	try {
		if (journal) localStorage.setItem(STORAGE_KEY, JSON.stringify(journal));
		else localStorage.removeItem(STORAGE_KEY);
	} catch {
		// Local settings remain available if storage is full or disabled.
	}
}

export function bindPreferenceAccount(account: SyncAccount | null): void {
	if (accountRevision && scope(current) === scope(account)) return;
	accountRevision++;
	current = account;
	const saved = readJournal();
	journal = saved?.scope === scope(account) && account ? saved : null;
	preferenceSync.enabled = journal?.enabled ?? false;
	preferenceSync.skipped = journal?.skipped ?? DEFAULT_SKIPPED_KEYS;
	preferenceSync.supported = null;
	preferenceSync.error = '';
	persist();
	if (journal?.enabled) schedule();
}

export function setPreferenceSync(enabled: boolean): void {
	if (!current || enabled === preferenceSync.enabled) return;
	journal = {
		scope: scope(current), enabled,
		...readPreferenceJournal({ catalogVersion: 2, skipped: journal?.skipped ?? DEFAULT_SKIPPED_KEYS, seed: journal?.seed }),
	};
	preferenceSync.enabled = enabled;
	preferenceSync.error = '';
	persist();
	if (enabled) schedule();
}

export function setPreferenceSyncIncluded(key: PreferenceKey, included: boolean): void {
	if (!current || !PREFERENCE_KEYS.includes(key)) return;
	journal ??= { scope: scope(current), enabled: false, ...readPreferenceJournal({}) };
	setPreferenceIncluded(journal, key, included);
	preferenceSync.skipped = journal.skipped;
	preferenceSync.error = '';
	persist();
	if (!journal.enabled) return;
	if (running) resync = true;
	else schedule();
}

export function startPreferenceSync(): () => void {
	const stopChanges = onPortableSettingChanged((changed) => {
		if (!journal?.enabled) return;
		const included = portablePreferences(changed, journal.skipped);
		if (Object.keys(included).length === 0) return;
		Object.assign(journal.pending, included);
		persist();
		if (running) resync = true;
		else schedule();
	});
	let stopped = false;
	function refreshAccount(): void {
		const revision = accountRevision;
		void invoke<SyncAccount | null>('sync_account_status').then((account) => {
			if (!stopped && revision === accountRevision) bindPreferenceAccount(account);
		}).catch(() => {});
	}
	function updateOnline(): void {
		preferenceSync.online = navigator.onLine;
	}
	function poll(): void {
		updateOnline();
		void syncPreferences();
	}
	function resume(): void {
		refreshAccount();
		poll();
	}
	const interval = setInterval(poll, POLL_INTERVAL);
	const unlisten = listen('vault-sync-status', () => {
		if (current) refreshAccount();
	}).catch(() => undefined);
	window.addEventListener('online', poll);
	window.addEventListener('offline', updateOnline);
	window.addEventListener('focus', resume);
	updateOnline();
	refreshAccount();
	return () => {
		stopped = true;
		clearTimeout(timer);
		clearInterval(interval);
		stopChanges();
		void unlisten.then((stop) => stop?.());
		window.removeEventListener('online', poll);
		window.removeEventListener('offline', updateOnline);
		window.removeEventListener('focus', resume);
	};
}

function schedule(): void {
	clearTimeout(timer);
	timer = setTimeout(() => { void syncPreferences(); }, 700);
}

function request(deviceId: string, patch?: PreferencePatch): Promise<PreferenceSnapshot> {
	return invoke<PreferenceSnapshot>('sync_account_preferences', { deviceId, patch: patch ?? null });
}

export async function syncPreferences(): Promise<void> {
	const account = current;
	const active = journal;
	if (!account || !active?.enabled || active.skipped.length === PREFERENCE_KEYS.length || running) return;
	running = true;
	resync = false;
	preferenceSync.syncing = true;
	preferenceSync.error = '';
	try {
		const remote = await reconcilePreferences(
			active, async (patch) => {
				const snapshot = await request(account.deviceId, patch);
				if (journal === active) preferenceSync.supported = supportedPreferenceKeys(snapshot);
				return snapshot;
			},
			() => encodePreferences(getSettings()), () => journal === active, persist
		);
		if (!remote || journal !== active) return;
		const unavailable = PREFERENCE_KEYS.filter((key) => !preferenceSync.supported?.includes(key));
		const previous = portablePreferences(active.remote, unavailable);
		applyPortableSettings(incomingPreferences(previous, remote, active.pending, encodePreferences(defaults)));
		for (const key of unavailable) {
			if (active.remote[key] !== undefined) remote[key] = active.remote[key];
		}
		active.remote = remote;
		persist();
	} catch (cause) {
		if (journal !== active || active.skipped.length === PREFERENCE_KEYS.length) return;
		const error = typeof cause === 'string' && cause.startsWith('sync_account.')
			? cause : 'sync_account.error_server';
		if (error === 'sync_account.error_revoked') bindPreferenceAccount(null);
		if (error !== 'sync_account.busy') preferenceSync.error = error;
	} finally {
		running = false;
		preferenceSync.syncing = false;
		if (journal?.enabled && (resync || journal !== active)) schedule();
	}
}
