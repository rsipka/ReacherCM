import { invoke } from '@tauri-apps/api/core';
import { getSettings, onPortableSettingChanged, applyPortableSettings } from '#lib/state/settings.svelte.js';
import { defaults } from '#lib/state/settings-store.js';
import {
	incomingPreferences, portablePreferences, reconcilePreferences,
	type PreferenceJournal, type PreferencePatch, type PreferenceSnapshot,
} from '#lib/sync/preferences.js';

export interface SyncAccount {
	server: string;
	username: string;
	deviceId: string;
}

interface Journal extends PreferenceJournal {
	scope: string;
}

const STORAGE_KEY = 'reachercm-preference-sync';
const POLL_INTERVAL = 30_000;
let current: SyncAccount | null = null;
let journal: Journal | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let running = false;
let accountResolved = false;
let resync = false;

export const preferenceSync = $state({ enabled: false, syncing: false, error: '' });

function scope(account: SyncAccount | null): string {
	return account ? JSON.stringify([account.server, account.username, account.deviceId]) : '';
}

function readJournal(): Journal | null {
	try {
		const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null') as Partial<Journal> | null;
		return value && typeof value.scope === 'string'
			? { scope: value.scope, initialized: value.initialized === true,
				pending: portablePreferences(value.pending), remote: portablePreferences(value.remote) }
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
	if (accountResolved && scope(current) === scope(account)) return;
	accountResolved = true;
	current = account;
	const saved = readJournal();
	journal = saved?.scope === scope(account) && account ? saved : null;
	preferenceSync.enabled = journal !== null;
	preferenceSync.error = '';
	persist();
	if (journal) schedule();
}

export function setPreferenceSync(enabled: boolean): void {
	if (enabled && (!current || journal)) return;
	journal = enabled ? { scope: scope(current), initialized: false, pending: {}, remote: {} } : null;
	preferenceSync.enabled = enabled;
	preferenceSync.error = '';
	persist();
	if (journal) schedule();
}

export function startPreferenceSync(): () => void {
	const stopChanges = onPortableSettingChanged((changed) => {
		if (!journal) return;
		Object.assign(journal.pending, changed);
		persist();
		if (running) resync = true;
		else schedule();
	});
	const poll = () => { void syncPreferences(); };
	const interval = setInterval(poll, POLL_INTERVAL);
	window.addEventListener('online', poll);
	window.addEventListener('focus', poll);
	let stopped = false;
	if (readJournal()) {
		void invoke<SyncAccount | null>('sync_account_status').then((account) => {
			if (!stopped && !accountResolved) bindPreferenceAccount(account);
		}).catch(() => {});
	}
	return () => {
		stopped = true;
		clearTimeout(timer);
		clearInterval(interval);
		stopChanges();
		window.removeEventListener('online', poll);
		window.removeEventListener('focus', poll);
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
	if (!account || !active || running) return;
	running = true;
	resync = false;
	preferenceSync.syncing = true;
	preferenceSync.error = '';
	try {
		const remote = await reconcilePreferences(
			active, (patch) => request(account.deviceId, patch),
			() => portablePreferences(getSettings()), () => journal === active, persist
		);
		if (!remote || journal !== active) return;
		applyPortableSettings(incomingPreferences(active.remote, remote, active.pending, portablePreferences(defaults)));
		active.remote = remote;
		persist();
	} catch (cause) {
		if (journal !== active) return;
		const error = typeof cause === 'string' && cause.startsWith('sync_account.')
			? cause : 'sync_account.error_server';
		if (error === 'sync_account.error_revoked') setPreferenceSync(false);
		if (error !== 'sync_account.busy') preferenceSync.error = error;
	} finally {
		running = false;
		preferenceSync.syncing = false;
		if (journal && (resync || journal !== active)) schedule();
	}
}
