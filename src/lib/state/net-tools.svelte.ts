/**
 * Asking the Network tools panel (the sidebar's) for a tool, from a
 * session's menu: which tool, for which session, and whether to start it.
 */

export type NetTool = 'ping' | 'scan' | 'wake';

export interface NetToolsRequest {
	tool: NetTool;
	sessionId?: string;
	/** Start at once (Ping, Scan ports, Wake from the menu). */
	run?: boolean;
	/** Tells one request from the next with the same contents. */
	n: number;
}

let request = $state<NetToolsRequest | null>(null);
let counter = 0;

/** Show the Network tools panel on `tool`, for `sessionId` if given. */
export function openNetworkTools(r: Omit<NetToolsRequest, 'n'>): void {
	request = { ...r, n: ++counter };
	window.dispatchEvent(new CustomEvent('reach:sidebar-section', { detail: 'network' }));
}

export function netToolsRequest(): NetToolsRequest | null {
	return request;
}

/** The panel took the request. */
export function takeNetToolsRequest(n: number): void {
	if (request?.n === n) request = null;
}

/** Sessions the tools can reach: those with a network address. */
export function hasNetworkAddress(s: { kind?: string | null; host: string }): boolean {
	const kind = s.kind ?? 'ssh';
	return kind !== 'serial' && kind !== 'wsl' && !!s.host.trim();
}
