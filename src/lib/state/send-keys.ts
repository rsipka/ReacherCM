/**
 * Key combinations a remote desktop cannot get from the keyboard while this
 * computer keeps them for itself (Ctrl+Alt+Del, Alt+Tab, the Windows key…),
 * sent from a menu instead: the full-screen bar's Keys menu and the tab's
 * own menu.
 *
 * Each desktop pane registers how to send them under its tab's id, as the
 * screenshot does.
 */

/** A combination: its keys, by position (`KeyboardEvent.code`), held in order. */
export interface KeyCombo {
	id: string;
	label: string;
	codes: string[];
}

export const KEY_COMBOS: KeyCombo[] = [
	{ id: 'cad', label: 'Ctrl+Alt+Del', codes: ['ControlLeft', 'AltLeft', 'Delete'] },
	{ id: 'taskmgr', label: 'Ctrl+Shift+Esc', codes: ['ControlLeft', 'ShiftLeft', 'Escape'] },
	{ id: 'win', label: 'Win', codes: ['MetaLeft'] },
	{ id: 'lock', label: 'Win+L', codes: ['MetaLeft', 'KeyL'] },
	{ id: 'run', label: 'Win+R', codes: ['MetaLeft', 'KeyR'] },
	{ id: 'desktop', label: 'Win+D', codes: ['MetaLeft', 'KeyD'] },
	{ id: 'explorer', label: 'Win+E', codes: ['MetaLeft', 'KeyE'] },
	{ id: 'alttab', label: 'Alt+Tab', codes: ['AltLeft', 'Tab'] },
	{ id: 'altf4', label: 'Alt+F4', codes: ['AltLeft', 'F4'] },
	{ id: 'prtsc', label: 'Print Screen', codes: ['PrintScreen'] },
	{ id: 'altprtsc', label: 'Alt+Print Screen', codes: ['AltLeft', 'PrintScreen'] }
];

/** The few the tab menu offers; the full-screen bar has them all. */
export const TAB_MENU_COMBOS = ['cad', 'taskmgr', 'lock', 'alttab', 'altf4'];

export interface KeySender {
	/** Whether the desktop is connected and can take keys now. */
	ready: () => boolean;
	send: (codes: string[]) => void;
}

const senders = new Map<string, KeySender>();

export function registerSendKeys(tabId: string, sender: KeySender): () => void {
	senders.set(tabId, sender);
	return () => {
		if (senders.get(tabId) === sender) senders.delete(tabId);
	};
}

/** null when the tab is no desktop; false while it cannot take keys. */
export function sendKeysReady(tabId: string): boolean | null {
	const sender = senders.get(tabId);
	return sender ? sender.ready() : null;
}

export function sendKeys(tabId: string, combo: KeyCombo): void {
	const sender = senders.get(tabId);
	if (sender?.ready()) sender.send(combo.codes);
}
