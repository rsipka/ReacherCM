/**
 * The rules behind the app's notifications (toasts), kept free of Svelte so
 * a Node test can check them (scripts/notify-test.mjs): where they appear,
 * how long the history keeps them, and when one goes to the system instead.
 */

export type ToastPosition = 'top-center' | 'top-left' | 'top-right' | 'bottom-left' | 'bottom-center' | 'bottom-right';

export const TOAST_POSITIONS: readonly ToastPosition[] = [
	'top-center',
	'top-left',
	'top-right',
	'bottom-left',
	'bottom-center',
	'bottom-right'
];

/** Where they always appeared: under the top bar, in the middle. */
export const DEFAULT_TOAST_POSITION: ToastPosition = 'top-center';

export function toastPosition(value: unknown): ToastPosition {
	return TOAST_POSITIONS.includes(value as ToastPosition) ? (value as ToastPosition) : DEFAULT_TOAST_POSITION;
}

/**
 * System notifications for toasts:
 * - off: in the app only;
 * - background: also as a system notification while the app is not in front
 *   (minimised, behind another window, a phone's app in the background);
 * - always: as a system notification instead of in the app. A toast with a
 *   button (Undo) still shows in the app, where the button works.
 */
export type SystemNotifyMode = 'off' | 'background' | 'always';

export const SYSTEM_NOTIFY_MODES: readonly SystemNotifyMode[] = ['off', 'background', 'always'];

export function systemNotifyMode(value: unknown): SystemNotifyMode {
	return SYSTEM_NOTIFY_MODES.includes(value as SystemNotifyMode) ? (value as SystemNotifyMode) : 'off';
}

export interface ToastRoute {
	/** Shown in the app's own toast. */
	inApp: boolean;
	/** Sent to the system as a notification. */
	system: boolean;
}

/** Where one toast goes. `seen`: the app's window is in front and shown. */
export function routeToast(mode: SystemNotifyMode, seen: boolean, hasAction: boolean): ToastRoute {
	if (mode === 'always') return { inApp: hasAction, system: true };
	if (mode === 'background' && !seen) return { inApp: true, system: true };
	return { inApp: true, system: false };
}

/** How long the history keeps a notification: a day. */
export const HISTORY_KEEP_MS = 24 * 60 * 60 * 1000;
/** At most this many, the newest kept: a loop of errors must not fill storage. */
export const HISTORY_MAX = 200;

export type HistoryType = 'info' | 'success' | 'warning' | 'error';

export interface HistoryEntry {
	id: string;
	message: string;
	type: HistoryType;
	/** When it appeared, ms since the epoch. */
	at: number;
}

const TYPES: readonly HistoryType[] = ['info', 'success', 'warning', 'error'];

/** The entries still kept at `now`, oldest first, at most HISTORY_MAX. */
export function pruneHistory(entries: HistoryEntry[], now: number): HistoryEntry[] {
	const kept = entries.filter((e) => e.at <= now + 60_000 && now - e.at < HISTORY_KEEP_MS);
	kept.sort((a, b) => a.at - b.at);
	return kept.length > HISTORY_MAX ? kept.slice(kept.length - HISTORY_MAX) : kept;
}

/** A stored history read back; anything malformed is dropped. */
export function parseHistory(raw: string | null, now: number): HistoryEntry[] {
	if (!raw) return [];
	let parsed: unknown;
	try {
		parsed = JSON.parse(raw);
	} catch {
		return [];
	}
	if (!Array.isArray(parsed)) return [];
	const entries: HistoryEntry[] = [];
	for (const item of parsed) {
		if (!item || typeof item !== 'object') continue;
		const { id, message, type, at } = item as Record<string, unknown>;
		if (typeof id !== 'string' || typeof message !== 'string' || typeof at !== 'number') continue;
		entries.push({ id, message, at, type: TYPES.includes(type as HistoryType) ? (type as HistoryType) : 'info' });
	}
	return pruneHistory(entries, now);
}

/** Entries newer than the last time the history was opened. */
export function unreadCount(entries: HistoryEntry[], lastSeen: number): number {
	let n = 0;
	for (const e of entries) if (e.at > lastSeen) n++;
	return n;
}
