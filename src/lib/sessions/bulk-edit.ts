/**
 * Rules for editing many sessions at once, shared by the bulk editor and its
 * test. They mirror the backend's (`src-tauri/src/ipc/session_bulk.rs`),
 * which has the last word: a session that does not take a field is skipped
 * there and reported back.
 */

import type { AuthMethod, SessionConfig, SessionKind, SessionPatch } from '#lib/ipc/sessions.js';

export type BulkField =
	| 'username'
	| 'password'
	| 'credential'
	| 'port'
	| 'domain'
	| 'tags'
	| 'notes'
	| 'terminal_theme'
	| 'history'
	| 'cursor_style'
	| 'cursor_blink'
	| 'auth'
	| 'try_agent_keys'
	| 'jump_chain'
	| 'proxy'
	| 'shell'
	| 'custom_fields'
	| 'telnet'
	| 'monitoring'
	| 'tmux'
	| 'via_session'
	| 'wireguard'
	| 'connect_mode'
	| 'address_check'
	| 'files'
	| 'terminal'
	| 'rdp'
	| 'wol'
	| 'favorite'
	| 'os_icon';

/** The kinds the session editor offers a WireGuard tunnel for. */
export const WIREGUARD_KINDS: SessionKind[] = ['ssh', 'telnet', 'rdp', 'vnc', 'spice', 'web', 'winbox'];
/** The kinds the session editor offers an OS icon for. */
export const OS_ICON_KINDS: SessionKind[] = ['ssh', 'telnet', 'vnc', 'spice', 'winbox'];

/** The value meaning "the session's own login" in `credential_id`. */
export const OWN_LOGIN = 'own';

function kindOf(s: Pick<SessionConfig, 'kind'>): SessionKind {
	return s.kind ?? 'ssh';
}

/** The kinds the session editor offers a saved credential for. */
export function takesCredential(s: Pick<SessionConfig, 'kind'>): boolean {
	const k = kindOf(s);
	return k === 'ssh' || k === 'rdp' || k === 'telnet' || k === 'winbox' || k === 'web' || k === 'bmc';
}

/** Whether a saved credential fills in the session's login, once the
 *  patch's credential choice (if any) is made. A choice of "the folder's"
 *  is judged by whether a folder's credential applies now: a guess, which
 *  the backend's report corrects. */
export function loginFromCredential(s: SessionConfig, credential?: SessionPatch['credential']): boolean {
	if (!takesCredential(s)) return false;
	if (credential === undefined) return !!s.credential && !s.credential.missing;
	if (credential === 'clear') return !!s.credential?.folder_id && !s.credential.missing;
	return credential.set !== OWN_LOGIN;
}

/** Whether `field` can change on `s`. `credential` is the patch's own
 *  credential choice, which decides whether the login is the session's. */
export function appliesTo(
	field: BulkField,
	s: SessionConfig,
	credential?: SessionPatch['credential'],
	auth?: AuthMethod['type']
): boolean {
	const k = kindOf(s);
	// The login method the session has once the patch's own (if any) is made.
	const method = k === 'ssh' && auth && !loginFromCredential(s, credential) ? auth : s.auth_method?.type;
	switch (field) {
		case 'username':
			return k !== 'wsl' && k !== 'serial' && !loginFromCredential(s, credential);
		case 'password':
			return k !== 'wsl' && k !== 'serial' && method === 'Password' && !loginFromCredential(s, credential);
		case 'credential':
			return takesCredential(s);
		case 'port':
			return k !== 'wsl' && k !== 'serial';
		case 'domain':
			return k === 'rdp';
		case 'terminal_theme':
		case 'history':
		case 'cursor_style':
		case 'cursor_blink':
			return k === 'ssh' || k === 'telnet' || k === 'wsl' || k === 'serial';
		case 'tags':
		case 'notes':
		case 'custom_fields':
			return true;
		case 'auth':
			return k === 'ssh' && !loginFromCredential(s, credential);
		case 'try_agent_keys':
			return k === 'ssh' && method === 'Key' && !loginFromCredential(s, credential);
		case 'jump_chain':
		case 'proxy':
		case 'shell':
			return k === 'ssh';
		case 'telnet':
			return k === 'telnet';
		case 'monitoring':
		case 'tmux':
			return k === 'ssh' || k === 'wsl';
		case 'via_session':
			return k === 'vnc' || k === 'spice';
		case 'wireguard':
			return WIREGUARD_KINDS.includes(k);
		case 'connect_mode':
		case 'address_check':
			// Picking among addresses: only where there are others.
			return k === 'ssh' && (s.alt_addresses?.length ?? 0) > 0;
		case 'files':
		case 'terminal':
			return k === 'ssh';
		case 'rdp':
			return k === 'rdp';
		case 'wol':
			// The MAC is each machine's own: only where one is set.
			return !!s.wol?.mac?.trim();
		case 'favorite':
			return true;
		case 'os_icon':
			return OS_ICON_KINDS.includes(k);
	}
}

/** How many of `sessions` take `field`. */
export function countApplying(
	field: BulkField,
	sessions: SessionConfig[],
	credential?: SessionPatch['credential'],
	auth?: AuthMethod['type']
): number {
	return sessions.filter((s) => appliesTo(field, s, credential, auth)).length;
}

export interface ValueSummary {
	/** Every session has this value (empty string for none). */
	same: string | null;
	/** Distinct values, most common first. */
	counts: [string, number][];
}

/** The values a field has across sessions, for "Mixed" and its tooltip. */
export function summarize<T>(sessions: T[], get: (s: T) => string | number | null | undefined): ValueSummary {
	const counts = new Map<string, number>();
	for (const s of sessions) {
		const v = get(s);
		const key = v === null || v === undefined ? '' : String(v);
		counts.set(key, (counts.get(key) ?? 0) + 1);
	}
	const sorted = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
	return { same: sorted.length === 1 ? sorted[0][0] : null, counts: sorted };
}

/** "root ×12, admin ×4"; `blank` names the empty value. At most `limit`. */
export function describeCounts(summary: ValueSummary, blank: string, limit = 8): string {
	const parts = summary.counts.slice(0, limit).map(([v, n]) => `${v === '' ? blank : v} ×${n}`);
	if (summary.counts.length > limit) parts.push('…');
	return parts.join(', ');
}

/** Whether the session has a password of its own (typed or from a manager). */
export function hasOwnPassword(s: SessionConfig): boolean {
	return s.auth_method?.type === 'Password' && (!!s.auth_method.password || !!s.auth_method.password_ref);
}

/** Tags typed as "a, b c" → ["a", "b", "c"]. */
export function parseTags(text: string): string[] {
	return [...new Set(text.split(/[,\s]+/).map((t) => t.trim()).filter(Boolean))];
}

/** The fields a patch changes, in the editor's order. */
export function changedFields(p: SessionPatch): BulkField[] {
	const out: BulkField[] = [];
	if (p.credential !== undefined) out.push('credential');
	if (p.username !== undefined) out.push('username');
	if (p.password !== undefined) out.push('password');
	if (p.port !== undefined) out.push('port');
	if (p.domain !== undefined) out.push('domain');
	if ((p.tags_add?.length ?? 0) > 0 || (p.tags_remove?.length ?? 0) > 0) out.push('tags');
	if (p.notes !== undefined) out.push('notes');
	if (p.terminal_theme !== undefined) out.push('terminal_theme');
	if (p.history !== undefined) out.push('history');
	if (p.cursor_style !== undefined) out.push('cursor_style');
	if (p.cursor_blink !== undefined) out.push('cursor_blink');
	if (p.auth !== undefined) out.push('auth');
	if (p.try_agent_keys !== undefined) out.push('try_agent_keys');
	if (p.jump_chain !== undefined) out.push('jump_chain');
	if (p.proxy !== undefined) out.push('proxy');
	if (p.shell !== undefined) out.push('shell');
	if ((p.fields_set?.length ?? 0) > 0 || (p.fields_remove?.length ?? 0) > 0) out.push('custom_fields');
	if (p.telnet !== undefined && Object.keys(p.telnet).length > 0) out.push('telnet');
	if (p.monitoring !== undefined) out.push('monitoring');
	if (p.tmux !== undefined && Object.keys(p.tmux).length > 0) out.push('tmux');
	if (p.via_session !== undefined) out.push('via_session');
	if (p.wireguard !== undefined) out.push('wireguard');
	if (p.connect_mode !== undefined) out.push('connect_mode');
	if (p.address_check !== undefined) out.push('address_check');
	if (p.files !== undefined) out.push('files');
	if (p.terminal !== undefined && Object.keys(p.terminal).length > 0) out.push('terminal');
	if (p.rdp !== undefined && Object.keys(p.rdp).length > 0) out.push('rdp');
	if (p.wol !== undefined && Object.keys(p.wol).length > 0) out.push('wol');
	if (p.favorite !== undefined) out.push('favorite');
	if (p.os_icon !== undefined) out.push('os_icon');
	return out;
}

/** The distinct vaults of `sessions` (null: this device's own sessions). */
export function vaultsOf(sessions: SessionConfig[]): (string | null)[] {
	return [...new Set(sessions.map((s) => s.vault_id ?? null))];
}
