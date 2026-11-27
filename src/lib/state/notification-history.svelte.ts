/**
 * The notification history behind the bell in the top bar: every toast of
 * the last day, so one that vanished after three seconds can still be read.
 *
 * Kept in this device's local storage only (never synced): a message can
 * name a host. Floating session windows share it through the storage event.
 * Settings > General > Notification history off empties it and hides the bell.
 */

import { getSettings } from '#lib/state/settings.svelte.js';
import { parseHistory, pruneHistory, unreadCount, type HistoryEntry, type HistoryType } from '#lib/state/notify-rules.js';

const STORAGE_KEY = 'reachercm-notification-history';
const SEEN_KEY = 'reachercm-notification-history-seen';

let entries = $state<HistoryEntry[]>([]);
let lastSeen = $state(0);
let loaded = false;

function readStorage(): void {
	try {
		entries = parseHistory(localStorage.getItem(STORAGE_KEY), Date.now());
		lastSeen = Number(localStorage.getItem(SEEN_KEY)) || 0;
	} catch {
		// No storage here (private mode): the history lives in memory.
	}
}

function writeStorage(): void {
	try {
		if (entries.length === 0) localStorage.removeItem(STORAGE_KEY);
		else localStorage.setItem(STORAGE_KEY, JSON.stringify(entries));
		localStorage.setItem(SEEN_KEY, String(lastSeen));
	} catch {
		// Storage full or unavailable: keep what is in memory.
	}
}

function ensureLoaded(): void {
	if (loaded || typeof window === 'undefined') return;
	loaded = true;
	readStorage();
	// Another window (a floating session) added or cleared one.
	window.addEventListener('storage', (e) => {
		if (e.key === STORAGE_KEY || e.key === SEEN_KEY || e.key === null) readStorage();
	});
}

/** The kept notifications, newest first. */
export function getHistory(): HistoryEntry[] {
	ensureLoaded();
	return [...entries].reverse();
}

/** How many came in since the history was last opened. */
export function getUnreadCount(): number {
	ensureLoaded();
	return unreadCount(entries, lastSeen);
}

export function recordNotification(message: string, type: HistoryType): void {
	if (!getSettings().notificationHistory) return;
	ensureLoaded();
	// Read again first: another window may have written since.
	readStorage();
	const now = Date.now();
	entries = pruneHistory([...entries, { id: crypto.randomUUID(), message, type, at: now }], now);
	writeStorage();
}

/** Opening the history reads them all. */
export function markHistorySeen(): void {
	ensureLoaded();
	entries = pruneHistory(entries, Date.now());
	lastSeen = Date.now();
	writeStorage();
}

export function removeHistoryEntry(id: string): void {
	ensureLoaded();
	entries = entries.filter((e) => e.id !== id);
	writeStorage();
}

export function clearHistory(): void {
	ensureLoaded();
	entries = [];
	writeStorage();
}
