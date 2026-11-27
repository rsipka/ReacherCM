/**
 * Keyboard shortcuts: the actions a key can start, their default keys, and
 * the user's own keys from Settings > Keyboard shortcuts.
 *
 * No Svelte here, so a Node test can check the rules
 * (scripts/keybindings-test.mjs). The handlers live in
 * #lib/state/shortcuts.svelte (anywhere in the app) and in the terminal and
 * split view (only while a terminal is in front).
 *
 * A key is stored as a chord string: `Mod+Shift+T`. `Mod` is Cmd on a Mac
 * and Ctrl elsewhere, so one saved key works on both; `Ctrl` is the Control
 * key itself on a Mac (and the same as `Mod` elsewhere). The order is always
 * Mod, Ctrl, Alt, Shift, key.
 */

export type ActionId =
	| 'tab.new'
	| 'tab.close'
	| 'tab.next'
	| 'tab.previous'
	| 'tab.last'
	| 'tab.go1'
	| 'tab.go2'
	| 'tab.go3'
	| 'tab.go4'
	| 'tab.go5'
	| 'tab.go6'
	| 'tab.go7'
	| 'tab.go8'
	| 'tab.goLast'
	| 'session.new'
	| 'session.quickConnect'
	| 'app.settings'
	| 'app.palette'
	| 'app.aiPanel'
	| 'app.mute'
	| 'zoom.in'
	| 'zoom.out'
	| 'zoom.reset'
	| 'terminal.find'
	| 'terminal.history'
	| 'terminal.commandPicker'
	| 'terminal.copy'
	| 'terminal.paste'
	| 'split.right'
	| 'split.down'
	| 'split.unsplit'
	| 'broadcast.toggle'
	| 'files.toggle'
	| 'capture.copy'
	| 'capture.save';

export type ActionGroup = 'tabs' | 'app' | 'terminal' | 'split';

export interface ActionDef {
	id: ActionId;
	group: ActionGroup;
	/** i18n key of the action's name. */
	label: string;
	/** Filled into the label, as t() does (`Go to tab {{n}}`). */
	labelParams?: Record<string, string | number>;
	defaults: string[];
	/** On a Mac, where they differ (Cmd+Tab belongs to the system). */
	macDefaults?: string[];
}

export const GROUP_LABELS: Record<ActionGroup, string> = {
	tabs: 'shortcuts.group_tabs',
	app: 'shortcuts.group_app',
	terminal: 'shortcuts.group_terminal',
	split: 'shortcuts.group_split'
};

export const ACTIONS: ActionDef[] = [
	// Ctrl+Shift for new tabs and sessions, as in GNOME Terminal and Windows
	// Terminal: plain Ctrl+T and Ctrl+N belong to programs in the terminal
	// (Midnight Commander's tag, Zellij's Tab and Resize modes, the shell).
	// A terminal cannot tell Ctrl+Shift+T from Ctrl+T, so nothing is lost.
	{ id: 'tab.new', group: 'tabs', label: 'shortcuts.tab_new', defaults: ['Mod+Shift+T'], macDefaults: ['Mod+T'] },
	{ id: 'tab.close', group: 'tabs', label: 'shortcuts.tab_close', defaults: ['Mod+Shift+W'] },
	// Ctrl+Tab sends a plain Tab to a terminal anyway. Cmd+Alt+arrows as in
	// Chrome on a Mac. Not Ctrl+PgUp/PgDn by default: Midnight Commander and
	// others use them in the terminal (add them in Settings if you like).
	{ id: 'tab.next', group: 'tabs', label: 'shortcuts.tab_next', defaults: ['Mod+Tab'], macDefaults: ['Ctrl+Tab', 'Mod+Alt+ArrowRight'] },
	{ id: 'tab.previous', group: 'tabs', label: 'shortcuts.tab_previous', defaults: ['Mod+Shift+Tab'], macDefaults: ['Ctrl+Shift+Tab', 'Mod+Alt+ArrowLeft'] },
	// Back to the tab that was in front before; again to return.
	{ id: 'tab.last', group: 'tabs', label: 'shortcuts.tab_last', defaults: ['Mod+Shift+L'] },
	// Ctrl+1…8 (Cmd on a Mac) as in a browser, Ctrl+9 the last tab. Not Alt+1…9:
	// shells, IRC clients and Emacs use Alt+digits, and Ctrl+Alt is AltGr on
	// Windows, which types characters on many layouts.
	...([1, 2, 3, 4, 5, 6, 7, 8] as const).map(
		(n): ActionDef => ({ id: `tab.go${n}`, group: 'tabs', label: 'shortcuts.tab_go', labelParams: { n }, defaults: [`Mod+${n}`] })
	),
	{ id: 'tab.goLast', group: 'tabs', label: 'shortcuts.tab_go_last', defaults: ['Mod+9'] },
	{ id: 'session.new', group: 'tabs', label: 'shortcuts.session_new', defaults: ['Mod+Shift+N'], macDefaults: ['Mod+N'] },
	{ id: 'session.quickConnect', group: 'tabs', label: 'shortcuts.session_quick', defaults: ['Mod+Shift+Q'], macDefaults: ['Mod+Shift+N'] },
	{ id: 'app.palette', group: 'app', label: 'shortcuts.palette', defaults: ['Mod+Shift+P'] },
	{ id: 'app.settings', group: 'app', label: 'shortcuts.settings', defaults: ['Mod+,'] },
	{ id: 'app.aiPanel', group: 'app', label: 'shortcuts.ai_panel', defaults: ['Mod+Shift+A'] },
	{ id: 'app.mute', group: 'app', label: 'shortcuts.mute', defaults: ['Mod+Shift+M'] },
	// Plus is Shift+= on most layouts and plain + on the number pad.
	{ id: 'zoom.in', group: 'app', label: 'shortcuts.zoom_in', defaults: ['Mod+=', 'Mod++', 'Mod+Shift++'] },
	{ id: 'zoom.out', group: 'app', label: 'shortcuts.zoom_out', defaults: ['Mod+-'] },
	{ id: 'zoom.reset', group: 'app', label: 'shortcuts.zoom_reset', defaults: ['Mod+0'] },
	{ id: 'terminal.find', group: 'terminal', label: 'shortcuts.find', defaults: ['Mod+Shift+F'] },
	{ id: 'terminal.history', group: 'terminal', label: 'shortcuts.history', defaults: ['Mod+Shift+H'] },
	// A shell's own Ctrl+R stays with the shell.
	{ id: 'terminal.commandPicker', group: 'terminal', label: 'shortcuts.command_picker', defaults: ['Mod+Shift+R'] },
	// Only while text is selected: otherwise Ctrl+C still interrupts.
	// The Control key copies and pastes on a Mac as well, as it always has.
	{ id: 'terminal.copy', group: 'terminal', label: 'shortcuts.copy', defaults: ['Mod+C'], macDefaults: ['Mod+C', 'Ctrl+C'] },
	{ id: 'terminal.paste', group: 'terminal', label: 'shortcuts.paste', defaults: ['Mod+V'], macDefaults: ['Mod+V', 'Ctrl+V'] },
	{ id: 'split.right', group: 'split', label: 'shortcuts.split_right', defaults: ['Mod+Shift+E'] },
	{ id: 'split.down', group: 'split', label: 'shortcuts.split_down', defaults: ['Mod+Shift+O'] },
	{ id: 'split.unsplit', group: 'split', label: 'shortcuts.unsplit', defaults: ['Mod+Shift+U'] },
	{ id: 'broadcast.toggle', group: 'split', label: 'shortcuts.broadcast', defaults: ['Mod+Shift+B'] },
	// An SSH tab's full-pane file manager, and back to its terminal.
	{ id: 'files.toggle', group: 'terminal', label: 'shortcuts.files_view', defaults: ['Mod+Shift+S'] },
	// A picture of the pane in front: a terminal, or a remote desktop.
	{ id: 'capture.copy', group: 'terminal', label: 'shortcuts.capture_copy', defaults: ['Mod+Shift+K'] },
	{ id: 'capture.save', group: 'terminal', label: 'shortcuts.capture_save', defaults: ['Mod+Alt+Shift+K'] }
];

const BY_ID = new Map(ACTIONS.map((a) => [a.id, a]));

export function isActionId(id: string): id is ActionId {
	return BY_ID.has(id as ActionId);
}

/** The user's keys, by action; an action not listed keeps its defaults,
 *  an empty list has no key at all. */
export type KeyOverrides = Partial<Record<ActionId, string[]>>;

export interface Chord {
	mod: boolean;
	ctrl: boolean;
	alt: boolean;
	shift: boolean;
	key: string;
}

/** `Mod+Shift+T` → parts; null when it is not a chord. `+` as the key is
 *  written `Mod++`. */
export function parseChord(text: string): Chord | null {
	if (typeof text !== 'string' || !text) return null;
	let rest = text;
	let key = '';
	if (rest === '+' || rest.endsWith('++')) {
		key = '+';
		rest = rest.slice(0, -2);
	} else {
		const at = rest.lastIndexOf('+');
		key = rest.slice(at + 1);
		rest = at < 0 ? '' : rest.slice(0, at);
	}
	if (!key) return null;
	const chord: Chord = { mod: false, ctrl: false, alt: false, shift: false, key };
	for (const part of rest ? rest.split('+') : []) {
		if (part === 'Mod') chord.mod = true;
		else if (part === 'Ctrl') chord.ctrl = true;
		else if (part === 'Alt') chord.alt = true;
		else if (part === 'Shift') chord.shift = true;
		else return null;
	}
	return chord;
}

export function formatChord(c: Chord): string {
	return [c.mod && 'Mod', c.ctrl && 'Ctrl', c.alt && 'Alt', c.shift && 'Shift', c.key].filter(Boolean).join('+');
}

/** The same key written one way, so two spellings compare equal. Off a
 *  Mac, Ctrl is Mod. */
export function canonical(text: string, mac: boolean): string | null {
	const c = parseChord(text);
	if (!c) return null;
	if (!mac && c.ctrl) {
		c.ctrl = false;
		c.mod = true;
	}
	return formatChord(c);
}

const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'OS', 'AltGraph', 'CapsLock', 'Fn', 'Hyper', 'Super']);

const NAMED: Record<string, string> = {
	' ': 'Space',
	Spacebar: 'Space',
	Esc: 'Escape',
	Left: 'ArrowLeft',
	Right: 'ArrowRight',
	Up: 'ArrowUp',
	Down: 'ArrowDown',
	Del: 'Delete'
};

type KeyEventLike = Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>;

/**
 * The chord a key press is, or null for a modifier on its own (and, off a
 * Mac, anything with the Windows key, which the system keeps).
 *
 * A letter is the letter typed when it is a Latin one, else the key's place
 * (KeyT), so Ctrl+T works on a Russian or Greek layout too. A digit is the
 * key's place, so Ctrl+Shift+1 is not Ctrl+Shift+!.
 */
export function chordFromEvent(e: KeyEventLike, mac: boolean): string | null {
	if (MODIFIER_KEYS.has(e.key)) return null;
	if (!mac && e.metaKey) return null;
	let key: string;
	const code = e.code ?? '';
	if (/^[a-z]$/i.test(e.key)) key = e.key.toUpperCase();
	else if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
	else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
	else if (/^Numpad[0-9]$/.test(code)) key = code.slice(6);
	else if (code === 'NumpadAdd') key = '+';
	else if (code === 'NumpadSubtract') key = '-';
	else key = NAMED[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase() : e.key);
	if (!key || key === 'Unidentified' || key === 'Dead' || key === 'Process') return null;
	const chord: Chord = {
		mod: mac ? e.metaKey : e.ctrlKey,
		ctrl: mac ? e.ctrlKey : false,
		alt: e.altKey,
		shift: e.shiftKey,
		key
	};
	return formatChord(chord);
}

/** The keys an action has now: the user's, else its defaults. */
export function keysFor(id: ActionId, overrides: KeyOverrides | undefined, mac: boolean): string[] {
	const own = overrides?.[id];
	if (Array.isArray(own)) return own;
	return defaultKeys(id, mac);
}

export function defaultKeys(id: ActionId, mac: boolean): string[] {
	const def = BY_ID.get(id);
	if (!def) return [];
	return [...(mac && def.macDefaults ? def.macDefaults : def.defaults)];
}

export function isDefault(id: ActionId, overrides: KeyOverrides | undefined, mac: boolean): boolean {
	return sameKeys(keysFor(id, overrides, mac), defaultKeys(id, mac), mac);
}

function sameKeys(a: string[], b: string[], mac: boolean): boolean {
	const ca = a.map((k) => canonical(k, mac)).sort();
	const cb = b.map((k) => canonical(k, mac)).sort();
	return ca.length === cb.length && ca.every((k, i) => k === cb[i]);
}

/** Every key in use, to the action it starts. The first action listed
 *  wins a key two of them share (only possible in a hand-edited file). */
export function keyMap(overrides: KeyOverrides | undefined, mac: boolean): Map<string, ActionId> {
	const map = new Map<string, ActionId>();
	for (const a of ACTIONS) {
		for (const k of keysFor(a.id, overrides, mac)) {
			const c = canonical(k, mac);
			if (c && !map.has(c)) map.set(c, a.id);
		}
	}
	return map;
}

/** The action already on this key, other than `except`. */
export function conflictFor(
	chord: string,
	overrides: KeyOverrides | undefined,
	mac: boolean,
	except?: ActionId
): ActionId | null {
	const c = canonical(chord, mac);
	if (!c) return null;
	for (const a of ACTIONS) {
		if (a.id === except) continue;
		if (keysFor(a.id, overrides, mac).some((k) => canonical(k, mac) === c)) return a.id;
	}
	return null;
}

/** Actions that share a key, which can only happen in an edited settings
 *  file or after defaults change. */
export function conflicts(overrides: KeyOverrides | undefined, mac: boolean): Map<string, ActionId[]> {
	const seen = new Map<string, ActionId[]>();
	for (const a of ACTIONS) {
		for (const k of keysFor(a.id, overrides, mac)) {
			const c = canonical(k, mac);
			if (!c) continue;
			const list = seen.get(c) ?? [];
			if (!list.includes(a.id)) list.push(a.id);
			seen.set(c, list);
		}
	}
	for (const [k, list] of seen) if (list.length < 2) seen.delete(k);
	return seen;
}

/**
 * Why a key cannot be used for a shortcut, or null when it can. A key with
 * no Ctrl, Cmd or Alt would stop that character reaching the terminal, so
 * only function keys may go without one.
 */
export function rejectReason(chord: string): 'needs_modifier' | null {
	const c = parseChord(chord);
	if (!c) return 'needs_modifier';
	if (c.mod || c.ctrl || c.alt) return null;
	return /^F([1-9]|1[0-9]|2[0-4])$/.test(c.key) ? null : 'needs_modifier';
}

/** Store only what differs from the defaults. */
function normalized(overrides: KeyOverrides, mac: boolean): KeyOverrides {
	const out: KeyOverrides = {};
	for (const a of ACTIONS) {
		const own = overrides[a.id];
		if (Array.isArray(own) && !sameKeys(own, defaultKeys(a.id, mac), mac)) out[a.id] = own;
	}
	return out;
}

/** Add a key to an action, taking it off whichever action had it. */
export function assignKey(overrides: KeyOverrides | undefined, id: ActionId, chord: string, mac: boolean): KeyOverrides {
	const c = canonical(chord, mac);
	if (!c) return { ...overrides };
	const next: KeyOverrides = {};
	for (const a of ACTIONS) {
		let keys = keysFor(a.id, overrides, mac).filter((k) => canonical(k, mac) !== c);
		if (a.id === id) keys = [...keys, c];
		next[a.id] = keys;
	}
	return normalized(next, mac);
}

/** Replace one of an action's keys with another (the chip clicked to change it). */
export function replaceKey(
	overrides: KeyOverrides | undefined,
	id: ActionId,
	old: string,
	chord: string,
	mac: boolean
): KeyOverrides {
	const removed = removeKey(overrides, id, old, mac);
	return assignKey(removed, id, chord, mac);
}

export function removeKey(overrides: KeyOverrides | undefined, id: ActionId, chord: string, mac: boolean): KeyOverrides {
	const c = canonical(chord, mac);
	const keys = keysFor(id, overrides, mac).filter((k) => canonical(k, mac) !== c);
	return normalized({ ...overrides, [id]: keys }, mac);
}

export function resetAction(overrides: KeyOverrides | undefined, id: ActionId, mac: boolean): KeyOverrides {
	const next = { ...overrides };
	delete next[id];
	return normalized(next, mac);
}

/** A stored copy read back: unknown actions and keys that are not chords
 *  are dropped. */
export function readOverrides(value: unknown): KeyOverrides {
	const out: KeyOverrides = {};
	if (!value || typeof value !== 'object' || Array.isArray(value)) return out;
	for (const [id, keys] of Object.entries(value as Record<string, unknown>)) {
		if (!isActionId(id) || !Array.isArray(keys)) continue;
		out[id] = keys.filter((k): k is string => typeof k === 'string' && parseChord(k) !== null);
	}
	return out;
}

const MAC_SYMBOLS: Record<string, string> = {
	ArrowUp: '↑',
	ArrowDown: '↓',
	ArrowLeft: '←',
	ArrowRight: '→',
	Enter: '↩',
	Backspace: '⌫',
	Delete: '⌦',
	Escape: '⎋',
	Tab: '⇥',
	Space: 'Space'
};

const PC_NAMES: Record<string, string> = {
	ArrowUp: 'Up',
	ArrowDown: 'Down',
	ArrowLeft: 'Left',
	ArrowRight: 'Right',
	Escape: 'Esc',
	Delete: 'Del',
	PageUp: 'PgUp',
	PageDown: 'PgDn'
};

/** How a key is written for people: ⌘⇧T on a Mac, Ctrl+Shift+T elsewhere. */
export function displayChord(chord: string, mac: boolean): string {
	const c = parseChord(chord);
	if (!c) return chord;
	if (mac) {
		const key = MAC_SYMBOLS[c.key] ?? c.key;
		return `${c.ctrl ? '⌃' : ''}${c.alt ? '⌥' : ''}${c.shift ? '⇧' : ''}${c.mod ? '⌘' : ''}${key}`;
	}
	const key = PC_NAMES[c.key] ?? c.key;
	return [(c.mod || c.ctrl) && 'Ctrl', c.alt && 'Alt', c.shift && 'Shift', key].filter(Boolean).join('+');
}
