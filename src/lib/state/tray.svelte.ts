/**
 * The tray icon's tooltip and menu (desktop only). The Rust side builds the
 * menu; this keeps it told what is open, which sessions are favorites or were
 * used lately, whether the vault is open and which language to speak, and
 * runs the items that need the interface (see `src-tauri/src/tray.rs`).
 *
 * Only session ids and names cross over, never a host, a login or a secret.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getTabs, isTabConnected, createTab } from './tabs.svelte';
import { recentSessionIds } from './recents.svelte';
import { vaultState, lock } from './vault.svelte';
import { requestSessionAction } from './palette.svelte';
import { openSettings } from './shortcuts.svelte';
import { setActivePage } from './navigation.svelte';
import { isMuted, toggleMute } from './mute.svelte';
import { t } from './i18n.svelte';
import { sessionList, type SessionConfig } from '#lib/ipc/sessions.js';
import { isMobile } from '#lib/platform.js';

const MAX = 15;

let sessions = $state<{ id: string; name: string; favorite: boolean }[]>([]);
let started = false;

function loadSessions(): void {
	if (vaultState.locked) {
		sessions = [];
		return;
	}
	void sessionList()
		.then((list: SessionConfig[]) => {
			sessions = list.map((s) => ({ id: s.id, name: s.name, favorite: !!s.favorite }));
		})
		.catch(() => (sessions = []));
}

function status(): string {
	if (vaultState.locked) return t('tray.status_locked');
	const tabs = getTabs();
	if (tabs.length === 0) return t('tray.status_none');
	const connected = tabs.filter((tab) => isTabConnected(tab)).length;
	return t('tray.status_tabs', { open: tabs.length, connected });
}

function trayState() {
	const byId = new Map(sessions.map((s) => [s.id, s]));
	const favorites = sessions
		.filter((s) => s.favorite)
		.sort((a, b) => a.name.localeCompare(b.name))
		.slice(0, MAX)
		.map(({ id, name }) => ({ id, name }));
	// As on the start screen: lately used, less those already in Favorites.
	const recent = recentSessionIds()
		.map((id) => byId.get(id))
		.filter((s): s is NonNullable<typeof s> => !!s && !s.favorite)
		.map(({ id, name }) => ({ id, name }));
	return {
		status: status(),
		locked: vaultState.locked,
		canLock: vaultState.hasIdentity,
		localTerminal: true,
		favorites,
		recent,
		labels: {
			show: t('tray.show'),
			hide: t('tray.hide'),
			favorites: t('tray.favorites'),
			recent: t('tray.recent'),
			localTerminal: t('tray.local_terminal'),
			quickConnect: t('tray.quick_connect'),
			mute: isMuted() ? t('titlebar.unmute') : t('titlebar.mute'),
			lock: t('tray.lock'),
			settings: t('tray.settings'),
			quit: t('tray.quit')
		}
	};
}

type TrayAction =
	| { action: 'connect'; sessionId: string }
	| { action: 'local-terminal' | 'quick-connect' | 'lock' | 'mute' | 'settings' };

function run(a: TrayAction): void {
	switch (a.action) {
		case 'connect':
			setActivePage('terminal');
			requestSessionAction({ kind: 'connect', sessionId: a.sessionId });
			break;
		case 'local-terminal':
			setActivePage('terminal');
			createTab('local');
			break;
		case 'quick-connect':
			setActivePage('terminal');
			requestSessionAction({ kind: 'quick-connect' });
			break;
		case 'lock':
			void lock().catch((err) => console.error('Lock from the tray failed:', err));
			break;
		case 'mute':
			toggleMute();
			break;
		case 'settings':
			openSettings();
			break;
	}
}

/** Tell the tray the window was hidden or shown (close to tray). */
export function syncTrayVisibility(): void {
	if (!started) return;
	void invoke('tray_sync').catch(() => {});
}

/** Start keeping the tray up to date. Main window only; safe to call twice. */
export function startTray(): void {
	if (started || isMobile()) return;
	started = true;

	void listen<TrayAction>('tray-action', (e) => run(e.payload));

	// The session list changes in many places (an edit, a sync, a favorite);
	// reading it again when the window loses focus, which is when the tray
	// gets clicked, keeps the menu right without each place telling us.
	window.addEventListener('reach:sessions-changed', loadSessions);
	window.addEventListener('blur', loadSessions);

	$effect.root(() => {
		// The vault opened or locked: its sessions come or go.
		$effect(() => {
			void vaultState.locked;
			loadSessions();
		});

		let timer: ReturnType<typeof setTimeout> | undefined;
		let last = '';
		$effect(() => {
			const state = trayState();
			const json = JSON.stringify(state);
			if (json === last) return;
			clearTimeout(timer);
			// Tabs connecting one after another make one rebuild, not ten.
			timer = setTimeout(() => {
				last = json;
				void invoke('tray_update', { state }).catch((err) => console.warn('Tray update failed:', err));
			}, 300);
		});
	});
}
