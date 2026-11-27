/**
 * Floating session windows: tabs moved out of the main window into windows
 * of their own, and back. The rules are in #lib/terminal/float-windows; the
 * windows themselves are made by the backend (ipc/window_commands.rs).
 *
 * Every window is the same app with its own tabs (#lib/state/tabs). Moving
 * tabs sends their records to the other window, which adopts them; each of
 * its terminals then asks this window for the terminal as it stands
 * (registerHandoff) and only then does this window let the tab go. A tab
 * whose window never asks (it failed to open) stays where it was.
 *
 * The connections never notice: they live in the backend, whose output
 * reaches every window.
 */

import { invoke } from '@tauri-apps/api/core';
import { emit, emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow, availableMonitors, cursorPosition } from '@tauri-apps/api/window';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { tick } from 'svelte';
import { isMobile } from '#lib/platform.js';
import { getTabs, getSplit, getActiveTab, adoptTabs, releaseTab, closeTab, clearHandoff, type Tab } from './tabs.svelte';
import { subLayout, rectAt, restoreRect, toPlacement, parsePlacement, parseKept, keepWindow, type MonitorInfo, type Placement, type Rect, type KeptWindow } from '#lib/terminal/float-windows.js';
import { toSaved } from '#lib/terminal/layouts.js';
import type { Layout } from '#lib/terminal/split.js';

/** This window's label when it is a floating one, from its address. */
const FLOAT_LABEL: string | null =
	typeof window === 'undefined' ? null : new URLSearchParams(window.location.search).get('float');

export function isFloatWindow(): boolean {
	return FLOAT_LABEL !== null;
}

/** This window's label: 'main', or 'float-2' and on. */
export function windowLabel(): string {
	return FLOAT_LABEL ?? 'main';
}

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** Phones and tablets have one window; so does the browser preview. */
export function canFloat(): boolean {
	return inTauri && !isMobile();
}

// ---- The terminal as it stands --------------------------------------------

export interface Snapshot {
	/** The screen and scrollback, as escape sequences (xterm's serialize addon). */
	data: string;
	/** The last output chunks it showed (see chunkKey), to drop what the new window also got. */
	tail: string[];
}

const providers = new Map<string, () => Snapshot | null>();

/**
 * A terminal says how to hand itself over. Called once, when the window
 * taking the tab asks: the terminal stops listening at that moment.
 */
export function registerHandoff(tabId: string, fn: () => Snapshot | null): () => void {
	providers.set(tabId, fn);
	return () => {
		if (providers.get(tabId) === fn) providers.delete(tabId);
	};
}

// ---- Moving tabs -----------------------------------------------------------

/** What one window sends another: the tabs, and their split there. */
interface MovePayload {
	from: string;
	moveId: string;
	tabs: Tab[];
	layout: Layout;
	active?: string;
}

/** Moves sent from here, and the tabs in each not yet taken. */
const outgoing = new Map<string, Set<string>>();

function payloadFor(ids: string[], layout?: Layout): MovePayload | null {
	const all = getTabs();
	// A DevOps workspace keeps its state in this window, so it stays here.
	const moving = ids.map((id) => all.find((t) => t.id === id)).filter((t): t is Tab => !!t && t.type !== 'devops');
	if (moving.length === 0) return null;
	const moveId = crypto.randomUUID();
	const active = getActiveTab()?.id;
	outgoing.set(moveId, new Set(moving.map((t) => t.id)));
	return {
		from: windowLabel(),
		moveId,
		tabs: moving.map((t) => {
			const plain = $state.snapshot(t) as Tab;
			return { ...plain, handoff: undefined };
		}),
		layout: layout !== undefined ? layout : subLayout(getSplit(), moving.map((t) => t.id)),
		active: active && moving.some((t) => t.id === active) ? active : moving[0].id
	};
}

export interface WindowInfo {
	label: string;
	title: string;
}

/** The other windows tabs can move to, main first. */
export async function otherWindows(): Promise<WindowInfo[]> {
	if (!canFloat()) return [];
	try {
		const all = await invoke<WindowInfo[]>('float_window_list');
		return all.filter((w) => w.label !== windowLabel());
	} catch {
		return [];
	}
}

/** The size a new window opens at: the last one's, else a comfortable default. */
function lastSize(): { width: number; height: number } {
	const p = lastPlacement();
	return p ? { width: p.width, height: p.height } : { width: 1100, height: 720 };
}

/** Open a new window showing these tabs (their split kept), at `rect` if given. */
export async function moveToNewWindow(ids: string[], rect?: Rect & { maximized?: boolean }, layout?: Layout): Promise<void> {
	const payload = payloadFor(ids, layout);
	if (!payload) return;
	try {
		const place = rect ?? (await placementForNew());
		await invoke('float_window_open', { payload, rect: place ?? null, title: titleFor(payload.tabs) });
	} catch (err) {
		outgoing.delete(payload.moveId);
		console.error('Opening a window failed:', err);
	}
}

/** Move these tabs into a window that is already open. */
export async function moveToWindow(ids: string[], label: string): Promise<void> {
	const payload = payloadFor(ids);
	if (!payload) return;
	await emitTo(label, 'float-adopt', { to: label, payload });
	void invoke('float_window_focus', { label }).catch(() => {});
}

/** A tab dragged out of the window and let go there: a new window under the pointer. */
export async function tearOff(id: string): Promise<void> {
	let point: { x: number; y: number } | null = null;
	let monitors: MonitorInfo[] = [];
	try {
		const p = await cursorPosition();
		point = { x: p.x, y: p.y };
		monitors = await monitorList();
	} catch {
		// No pointer position (Wayland gives none): the window opens where the last one was.
	}
	// Only the tab dragged goes, never the split it is in.
	await moveToNewWindow([id], point ? rectAt(point, lastSize(), monitors) : undefined);
}

function titleFor(tabs: Pick<Tab, 'sessionName' | 'title'>[]): string {
	const first = tabs[0];
	const name = first ? first.sessionName || first.title : '';
	return name ? `${name} - Reacher CM` : 'Reacher CM';
}

// ---- Taking tabs -----------------------------------------------------------

function adopt(p: MovePayload): void {
	const tabs = p.tabs.map((t) => ({ ...t, handoff: { from: p.from, moveId: p.moveId } }));
	adoptTabs(tabs, p.layout, p.active);
	// A remote desktop's panel attaches to the running desktop by itself;
	// the other window is only told to let it go. A terminal asks for its
	// screen once it listens (see takeOver).
	for (const t of tabs) {
		if (t.type === 'rdp' || t.type === 'vnc' || t.type === 'spice') {
			void requestSnapshot(t.id, t.handoff).then(() => clearHandoff(t.id));
		}
	}
}

interface SnapshotReply {
	to: string;
	tabId: string;
	moveId: string;
	snapshot: Snapshot | null;
}

const waiting = new Map<string, (s: Snapshot | null) => void>();

/** How long a window waits for the one giving a tab up. */
const SNAPSHOT_WAIT_MS = 8000;

/**
 * Ask the window a tab came from for its terminal as it stands; that window
 * then lets the tab go. Null: nothing came (no terminal there, or it is gone).
 */
export async function requestSnapshot(tabId: string, h: { from: string; moveId: string }): Promise<Snapshot | null> {
	const key = `${h.moveId}:${tabId}`;
	const got = new Promise<Snapshot | null>((resolve) => {
		const timer = setTimeout(() => {
			waiting.delete(key);
			resolve(null);
		}, SNAPSHOT_WAIT_MS);
		waiting.set(key, (s) => {
			clearTimeout(timer);
			waiting.delete(key);
			resolve(s);
		});
	});
	await emitTo(h.from, 'float-snapshot-request', { to: h.from, tabId, moveId: h.moveId, replyTo: windowLabel() });
	return got;
}

// ---- Listening -------------------------------------------------------------

let started = false;
const stops: UnlistenFn[] = [];

/** Listen for tabs arriving and for windows asking for theirs. Every window does. */
export async function startWindows(): Promise<void> {
	if (started || !canFloat()) return;
	started = true;
	const me = getCurrentWebviewWindow();
	const label = windowLabel();

	stops.push(
		await me.listen<{ to: string; payload: MovePayload }>('float-adopt', (e) => {
			if (e.payload.to === label) adopt(e.payload.payload);
		})
	);

	stops.push(
		await me.listen<{ to: string; tabId: string; moveId: string; replyTo: string }>('float-snapshot-request', (e) => {
			const { to, tabId, moveId, replyTo } = e.payload;
			if (to !== label) return;
			const pending = outgoing.get(moveId);
			// Only for a move sent from here, and once per tab.
			if (!pending?.has(tabId)) {
				void emitTo(replyTo, 'float-snapshot', { to: replyTo, tabId, moveId, snapshot: null } satisfies SnapshotReply);
				return;
			}
			let snapshot: Snapshot | null = null;
			try {
				snapshot = providers.get(tabId)?.() ?? null;
			} catch (err) {
				console.error('Handing a terminal over failed:', err);
			}
			providers.delete(tabId);
			void emitTo(replyTo, 'float-snapshot', { to: replyTo, tabId, moveId, snapshot } satisfies SnapshotReply);
			pending.delete(tabId);
			if (pending.size === 0) outgoing.delete(moveId);
			releaseTab(tabId);
		})
	);

	stops.push(
		await me.listen<SnapshotReply>('float-snapshot', (e) => {
			if (e.payload.to !== label) return;
			waiting.get(`${e.payload.moveId}:${e.payload.tabId}`)?.(e.payload.snapshot);
		})
	);

	if (isFloatWindow()) await startFloat();
}

// ---- A floating window's own life -----------------------------------------

/** Placement of the last floating window moved or sized: where the next one opens. */
const LAST_KEY = 'reach-float-placement';
/** Floating windows open now, by label, as kept for the next start. */
const KEPT_KEY = 'reach-float-windows';

function lastPlacement(): Placement | null {
	try {
		return parsePlacement(JSON.parse(localStorage.getItem(LAST_KEY) ?? 'null'));
	} catch {
		return null;
	}
}

async function monitorList(): Promise<MonitorInfo[]> {
	try {
		const ms = await availableMonitors();
		return ms.map((m) => ({ name: m.name, position: { x: m.position.x, y: m.position.y }, size: { width: m.size.width, height: m.size.height }, scaleFactor: m.scaleFactor }));
	} catch {
		return [];
	}
}

/** Where a new window opens: where the last one was, a little down and right
 *  when a window is already there. Null: let the system centre it. */
async function placementForNew(): Promise<(Rect & { maximized: boolean }) | null> {
	const p = lastPlacement();
	if (!p) return null;
	const r = restoreRect(p, await monitorList());
	const open = (await otherWindows()).filter((w) => w.label !== 'main').length;
	if (open > 0 && !r.maximized) {
		const step = 32 * Math.min(open, 6);
		return { ...r, x: r.x + step, y: r.y + step };
	}
	return r;
}

function readKept(): Record<string, unknown> {
	try {
		const raw = JSON.parse(localStorage.getItem(KEPT_KEY) ?? '{}');
		return raw && typeof raw === 'object' && !Array.isArray(raw) ? raw : {};
	} catch {
		return {};
	}
}

function writeKept(all: Record<string, unknown>): void {
	try {
		localStorage.setItem(KEPT_KEY, JSON.stringify(all));
	} catch {
		// Storage full: the window is just not reopened next time.
	}
}

let placement: Placement | null = null;

/** Keep what this window shows and where, for the next start (see reopenKeptWindows). */
function keepSelf(): void {
	const all = readKept();
	const kept = keepWindow(getTabs(), toSaved(getSplit(), getTabs()), placement);
	if (kept) all[windowLabel()] = kept;
	else delete all[windowLabel()];
	writeKept(all);
}

function forgetSelf(): void {
	const all = readKept();
	delete all[windowLabel()];
	writeKept(all);
}

async function notePlacement(): Promise<void> {
	try {
		const w = getCurrentWindow();
		if (await w.isMinimized()) return;
		const [pos, size, max] = await Promise.all([w.outerPosition(), w.innerSize(), w.isMaximized()]);
		const monitors = await monitorList();
		const p = toPlacement({ x: pos.x, y: pos.y, width: size.width, height: size.height }, max, monitors);
		// Maximized, the size is the screen's; the one to come back to is the last one before.
		placement = max && placement ? { ...placement, monitor: p.monitor, maximized: true } : p;
		localStorage.setItem(LAST_KEY, JSON.stringify(placement));
		keepSelf();
	} catch {
		// No window to ask (browser preview).
	}
}

/** The window is closing: everything it shows goes back to the main window. */
export async function moveAllToMain(): Promise<void> {
	const ids = getTabs().map((t) => t.id);
	if (ids.length) await moveToWindow(ids, 'main');
}

/** The window is closing and its sessions with it. */
export async function closeAllHere(): Promise<void> {
	for (const t of [...getTabs()]) closeTab(t.id);
	// The page's clean-up (+page.svelte) ends each connection as its tab goes.
	await tick();
	await new Promise((r) => setTimeout(r, 150));
}

/** Close this floating window once it shows nothing more. */
export async function closeSelf(): Promise<void> {
	forgetSelf();
	try {
		await getCurrentWindow().destroy();
	} catch {
		// Already gone.
	}
}

/** Bring the main window to the front (to unlock the vault, say). */
export function focusMain(): void {
	void invoke('float_window_focus', { label: 'main' }).catch(() => {});
}

async function startFloat(): Promise<void> {
	const w = getCurrentWindow();
	try {
		const p = await invoke<MovePayload | null>('float_window_take');
		if (p) adopt(p);
	} catch (err) {
		console.error('A floating window found nothing to show:', err);
	}
	placement = lastPlacement();
	void notePlacement();
	let timer: ReturnType<typeof setTimeout> | undefined;
	const later = () => {
		clearTimeout(timer);
		timer = setTimeout(() => void notePlacement(), 400);
	};
	stops.push(await w.onMoved(later));
	stops.push(await w.onResized(later));

	// Typing here is using the app: the main window's auto-lock hears of it.
	let lastSent = 0;
	const active = () => {
		const now = Date.now();
		if (now - lastSent < 10_000) return;
		lastSent = now;
		void emitTo('main', 'float-activity', null).catch(() => {});
	};
	for (const kind of ['pointerdown', 'keydown', 'wheel'] as const) {
		window.addEventListener(kind, active, { capture: true, passive: true });
	}
}

/** What this floating window shows changed: kept for the next start, and its title. */
export function noteContents(): void {
	if (!isFloatWindow()) return;
	keepSelf();
	const tab = getActiveTab();
	if (tab) void getCurrentWindow().setTitle(titleFor([tab])).catch(() => {});
}

// ---- The main window -------------------------------------------------------

/** The vault locked or opened: floating windows cover or show their sessions. */
export function announceLock(held: boolean): void {
	if (!canFloat() || isFloatWindow()) return;
	void emit('float-lock', { held }).catch(() => {});
}

/** Settings changed in one window: the others read them again. */
export function announceSettings(): void {
	if (!canFloat()) return;
	void emit('float-settings', { from: windowLabel() }).catch(() => {});
}

export function onOtherSettings(fn: () => void): void {
	if (!canFloat()) return;
	void listen<{ from: string }>('float-settings', (e) => {
		if (e.payload.from !== windowLabel()) fn();
	});
}

/**
 * At start, in the main window: the floating windows open when the app
 * last closed, read once and forgotten. With "reopen" off they are only
 * forgotten.
 */
export function takeKeptWindows(): KeptWindow[] {
	if (!canFloat() || isFloatWindow()) return [];
	const kept = parseKept(readKept());
	writeKept({});
	return kept;
}

/** Open a kept window again around tabs already opened here (see openKept in #lib/state/layouts). */
export async function reopenAround(ids: string[], kept: KeptWindow, layout: Layout): Promise<void> {
	if (ids.length === 0) return;
	const rect = kept.placement ? restoreRect(kept.placement, await monitorList()) : undefined;
	await moveToNewWindow(ids, rect, layout);
}
