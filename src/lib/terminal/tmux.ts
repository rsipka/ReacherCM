/**
 * tmux on the server (see src-tauri/src/ssh/tmux.rs): which SSH sessions
 * start inside a tmux session, and under which name.
 *
 * A session says `off` or `attach` itself, or leaves it to its folder; the
 * nearest folder that says on or off decides, else it is off. The name is
 * the session's own, or `reachercm-tmux` for every session: one tmux session
 * per server account, in which each tab is a window of its own. Before, the
 * default was `reachercm-` and the session's name in plain letters; such a
 * session is still attached to while it is the only one there (see
 * TmuxStart.legacy).
 */

import type { Folder, SessionConfig, TmuxSetting } from '#lib/ipc/sessions.js';
import type { TmuxStart } from '#lib/ipc/ssh.js';

export type { TmuxStart };

const MAX_NAME = 64;

/** Letters, digits, '-' and '_', not starting with '-': what the backend takes. */
export function validTmuxName(name: string): boolean {
	return name.length > 0 && name.length <= MAX_NAME && !name.startsWith('-') && /^[A-Za-z0-9_-]+$/.test(name);
}

/** The tmux session every session uses unless it names its own. */
export const DEFAULT_TMUX_NAME = 'reachercm-tmux';
/** What the default names started with before. */
const OLD_PREFIX = 'reachercm';

function nameWith(prefix: string, sessionName: string): string {
	const slug = sessionName
		.normalize('NFKD')
		.replace(/[\u0300-\u036f]/g, '')
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '')
		.slice(0, MAX_NAME - prefix.length - 1)
		.replace(/-+$/, '');
	return slug ? `${prefix}-${slug}` : prefix;
}

/** The default name earlier versions gave: "Web 01 (prod)" → "reachercm-web-01-prod". */
export function legacyTmuxName(sessionName: string): string {
	return nameWith(OLD_PREFIX, sessionName);
}

/** The folder setting that applies to a session in `folderId`: the nearest that says. */
export function folderTmux(folderId: string | null | undefined, folders: Folder[]): boolean | null {
	const seen = new Set<string>();
	let id = folderId ?? null;
	while (id && !seen.has(id)) {
		seen.add(id);
		const f = folders.find((x) => x.id === id);
		if (!f) return null;
		if (f.tmux != null) return f.tmux;
		id = f.parent_id;
	}
	return null;
}

/** Whether a session starts in tmux, and as what; null: it does not. SSH
 *  sessions, and WSL ones (which never ask: there is no list to ask from
 *  before the shell starts). */
export function tmuxFor(session: Pick<SessionConfig, 'name' | 'kind' | 'folder_id' | 'tmux'>, folders: Folder[]): TmuxStart | null {
	const kind = session.kind ?? 'ssh';
	if (kind !== 'ssh' && kind !== 'wsl') return null;
	const own: TmuxSetting | null | undefined = session.tmux;
	const mode = own?.mode;
	const on = mode === 'attach' || mode === 'ask' ? true : mode === 'off' ? false : folderTmux(session.folder_id, folders) === true;
	if (!on) return null;
	const name = own?.name?.trim();
	const ownName = !!name && validTmuxName(name);
	const start: TmuxStart = { name: ownName ? name : DEFAULT_TMUX_NAME };
	// The default name changed: a session started under the old one is
	// still there to go back to (the backend picks it while it is the only one).
	if (!ownName) start.legacy = legacyTmuxName(session.name);
	if (mode === 'ask' && kind === 'ssh') start.ask = true;
	if (own?.mouse) start.mouse = true;
	return start;
}

/**
 * A tmux tab whose connection dropped tries again by itself: nothing is
 * lost on the server. The wait before try `attempt` (counted from 0), or
 * null once the tab has tried for long enough since the drop.
 */
const RETRY_SECONDS = [2, 5, 10, 30];
export const AUTO_RECONNECT_FOR_MS = 10 * 60_000;
export function autoReconnectDelay(attempt: number, sinceDropMs: number): number | null {
	if (sinceDropMs >= AUTO_RECONNECT_FOR_MS) return null;
	return (RETRY_SECONDS[attempt] ?? 60) * 1000;
}

/** A failure that trying again by itself cannot fix: the login needs the
 *  user (a password, a passphrase, a host key, an approval). */
export function needsUser(error: string): boolean {
	return /auth|password|passphrase|host key|host identity|permission denied|denied|approv|refused the key|locked/i.test(error);
}

/** A tag for a tab's own view of a tmux session (see TmuxStart.view). */
export function newViewTag(): string {
	const bytes = crypto.getRandomValues(new Uint8Array(4));
	return Array.from(bytes, (b) => b.toString(36).padStart(2, '0').slice(-2)).join('');
}

/** A view tab sets the terminal title to its window's id and name
 *  ("@3 logs"): which window the tab shows. */
export function tmuxWindowFromTitle(title: string): { id: string; name: string } | null {
	const m = /^(@\d+) ([\s\S]*)$/.exec(title);
	return m ? { id: m[1], name: m[2] } : null;
}

