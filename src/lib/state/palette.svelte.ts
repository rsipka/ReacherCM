/**
 * The command palette's state, and the requests it leaves for the sessions
 * sidebar.
 *
 * Connecting a session is the session list's own flow (password managers,
 * approvals, RDP, VNC, WSL), and the list exists only while its panel is
 * shown. So the palette does not connect anything itself: it leaves a
 * request here, the sidebar shows the sessions panel, and the list carries
 * the request out the way a click on the session would.
 */

import type { TypedAddress } from '#lib/utils/address.js';

export type SessionRequest =
	/** `shell` opens the session with another login shell (a container's). */
	| { kind: 'connect'; sessionId: string; shell?: string }
	/** Several, one after the other (a saved split layout opening). */
	| { kind: 'connect-many'; sessionIds: string[] }
	| { kind: 'new-session' }
	/** Show the session in the list (folders opened, scrolled to, marked). */
	| { kind: 'locate'; sessionId: string }
	/** Open the session's editor. */
	| { kind: 'edit'; sessionId: string }
	/** `prefill`: an address someone typed, to start the dialog from. */
	| { kind: 'quick-connect'; prefill?: TypedAddress }
	/** A server management session's serial console: SSH to the controller
	 *  at `port`, then type `command`. */
	| { kind: 'bmc-console'; sessionId: string; port: number; command: string };

let open = $state(false);
let request = $state<SessionRequest | null>(null);

export function isPaletteOpen(): boolean {
	return open;
}

export function openPalette(): void {
	open = true;
}

export function closePalette(): void {
	open = false;
}

export function togglePalette(): void {
	open = !open;
}

/** A request for the session list; the sidebar opens it to carry it out. */
export function requestSessionAction(r: SessionRequest): void {
	request = r;
}

export function pendingSessionAction(): SessionRequest | null {
	return request;
}

/** Take the request, so it runs once. */
export function takeSessionAction(): SessionRequest | null {
	const r = request;
	request = null;
	return r;
}

let focusRequests = $state(0);

/** Gives the keyboard back to the active terminal: after the palette has
 *  typed a snippet into it, Enter should go there and not to the page. */
export function requestTerminalFocus(): void {
	focusRequests++;
}

export function terminalFocusRequests(): number {
	return focusRequests;
}
