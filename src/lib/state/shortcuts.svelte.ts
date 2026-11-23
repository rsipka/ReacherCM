import { newTab } from '#lib/terminal/tmux-tabs.js';
import { createTab, closeTab, getActiveTab, getTabs, activateTab, canShowFilesView, toggleFilesView } from './tabs.svelte';
import { toggleAIPanel } from './ai-chat.svelte';
import { togglePalette, requestSessionAction } from './palette.svelte';
import { stepUiZoom } from './ui-zoom';
import { isMac, isMobile } from '#lib/platform.js';
import { getSettings } from './settings.svelte';
import { captureTab } from './capture';
import { canonical, chordFromEvent, displayChord, keyMap, keysFor, type ActionId } from './keybindings';

let settingsOpener: ((target?: string) => void) | null = null;

export function registerSettingsOpener(fn: (target?: string) => void): void {
	settingsOpener = fn;
}

/** The sessions sidebar's own actions, so a key can reach them from anywhere. */
export interface SessionActions {
	newSession: () => void;
	quickConnect: () => void;
}

let sessionActions: SessionActions | null = null;

export function registerSessionActions(actions: SessionActions | null): void {
	sessionActions = actions;
}

/** How a fixed shortcut is written on a menu: ⌘N on a Mac, Ctrl+N
 *  elsewhere. For one the user can change, use actionLabel. */
export function shortcutLabel(key: string, mods: { ctrl?: boolean; shift?: boolean; alt?: boolean } = {}): string {
	const k = key.length === 1 ? key.toUpperCase() : key;
	const chord = [mods.ctrl && 'Mod', mods.alt && 'Alt', mods.shift && 'Shift', k].filter(Boolean).join('+');
	return displayChord(chord, isMac());
}

/** While Settings records a new key, every shortcut stands aside. */
let recording = false;

/** The keys an action has now (Settings > Keyboard shortcuts). */
export function actionKeys(id: ActionId): string[] {
	return keysFor(id, getSettings().keyboardShortcuts, isMac());
}

/** The first key an action has, as a menu writes it; '' when it has none. */
export function actionLabel(id: ActionId): string {
	const [first] = actionKeys(id);
	return first ? displayChord(first, isMac()) : '';
}

/** This key press is one of the action's keys. */
export function matchesAction(e: KeyboardEvent, id: ActionId): boolean {
	if (recording) return false;
	const mac = isMac();
	const chord = chordFromEvent(e, mac);
	return chord !== null && actionKeys(id).some((k) => canonical(k, mac) === chord);
}

export function setRecordingShortcut(on: boolean): void {
	recording = on;
}

function nextTab(): void {
	const tabs = getTabs();
	if (tabs.length <= 1) return;

	const activeIndex = tabs.findIndex((t) => t.active);
	const nextIndex = (activeIndex + 1) % tabs.length;
	activateTab(tabs[nextIndex].id);
}

function previousTab(): void {
	const tabs = getTabs();
	if (tabs.length <= 1) return;

	const activeIndex = tabs.findIndex((t) => t.active);
	const prevIndex = (activeIndex - 1 + tabs.length) % tabs.length;
	activateTab(tabs[prevIndex].id);
}

function closeActiveTab(): void {
	const active = getActiveTab();
	if (active) {
		closeTab(active.id);
	}
}

/**
 * Open Settings. `target` is a page id ('terminal') or a row's label key
 * ('settings.term_osc52'): the page opens with that row highlighted, so a
 * toast can say "Allow it in Settings" and take you there.
 */
export function openSettings(target?: string): void {
	settingsOpener?.(target);
}

/** A screenshot of the pane in front. */
function captureActive(mode: 'copy' | 'save'): void {
	const active = getActiveTab();
	if (active) void captureTab(active.id, mode, active.sessionName || active.title);
}

function openCommandPalette(): void {
	togglePalette();
}


/** What each key does anywhere in the app. Terminal-only actions (find,
 *  copy, split) are handled where the terminal is. */
const handlers: Partial<Record<ActionId, () => void>> = {
	'tab.new': newTab,
	'session.new': () => requestSessionAction({ kind: 'new-session' }),
	'session.quickConnect': () => requestSessionAction({ kind: 'quick-connect' }),
	'tab.close': closeActiveTab,
	'tab.next': nextTab,
	'tab.previous': previousTab,
	'app.settings': () => openSettings(),
	'app.palette': openCommandPalette,
	'app.aiPanel': toggleAIPanel,
	'files.toggle': () => {
		const active = getActiveTab();
		if (active && canShowFilesView(active)) toggleFilesView(active.id);
	},
	'capture.copy': () => captureActive('copy'),
	'capture.save': () => captureActive('save'),
	// The interface's size, as in a browser. Not on a phone: it follows the system.
	...(isMobile()
		? {}
		: {
				'zoom.in': () => stepUiZoom(1),
				'zoom.out': () => stepUiZoom(-1),
				'zoom.reset': () => stepUiZoom(0)
			})
};

/** The keys a browser reloads the page on: F5, Ctrl+R and Ctrl+F5, Cmd+R
 *  on a Mac, each with or without Shift. Reach is one page, so a reload
 *  threw away every open terminal, file list and tab. KeyR by position,
 *  so it is the same key on every layout. */
export function isReloadKey(e: Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'metaKey' | 'altKey'>): boolean {
	if (e.key === 'F5' || e.key === 'BrowserRefresh') return true;
	return (isMac() ? e.metaKey : e.ctrlKey) && !e.altKey && e.code === 'KeyR';
}

function onKeydown(e: KeyboardEvent): void {
	if (recording) return;
	// Only the page's own reload is stopped: the key still goes on to the
	// terminal, where F5 means something to programs like Midnight
	// Commander and Ctrl+R searches the shell's history.
	if (isReloadKey(e)) e.preventDefault();
	const mac = isMac();
	const chord = chordFromEvent(e, mac);
	if (!chord) return;
	const id = keyMap(getSettings().keyboardShortcuts, mac).get(chord);
	const handler = id ? handlers[id] : undefined;
	if (!handler) return;
	e.preventDefault();
	e.stopPropagation();
	handler();
}

let initialized = false;

export function initShortcuts(): void {
	if (initialized) return;
	document.addEventListener('keydown', onKeydown, true);
	initialized = true;
}

export function cleanupShortcuts(): void {
	if (!initialized) return;
	document.removeEventListener('keydown', onKeydown, true);
	initialized = false;
}
