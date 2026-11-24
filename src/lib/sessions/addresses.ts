/**
 * A session's addresses: its main `host` and the others the same server
 * answers on (an office and an internet address, IPv4 and IPv6, other
 * names), and which one a connect uses.
 *
 * Auto: the addresses that are reached straight are checked all at once
 * with a plain TCP connect (`ssh_probe_addresses`); those that answer go
 * first, by priority, then those that go through the session's jump hosts
 * or proxy (they cannot be checked without logging in to the jump host),
 * then the ones that did not answer, in case the check was wrong. A connect
 * that fails before the login moves on to the next; a refused login does
 * not, since it is the same server.
 *
 * Pure logic, no IPC, so scripts/addresses-test.mjs can run it.
 */

import type { ConnectMode, SessionAddress, SessionConfig } from '#lib/ipc/sessions.js';

export interface ResolvedAddress {
	/** 0 for the main address, then the others in their order. */
	index: number;
	label: string;
	host: string;
	port: number;
	/** Goes through jump hosts or a proxy, so it cannot be checked. */
	routed: boolean;
	/** Goes through this saved SSH session rather than the session's own
	 *  route. */
	viaSessionId?: string;
	/** Goes through this WireGuard profile's tunnel: the main address
	 *  with the session's own jump hosts (the tunnel carries the first),
	 *  another address straight inside the tunnel. */
	wireguardId?: string;
	/** Another address that goes only through its WireGuard tunnel, not
	 *  the session's jump hosts or proxy. */
	tunnelOnly?: boolean;
}

type AddressSession = Pick<SessionConfig, 'host' | 'port' | 'host_label' | 'alt_addresses' | 'connect_mode' | 'jump_chain' | 'proxy' | 'kind'> & Partial<Pick<SessionConfig, 'wireguard_id'>>;

/** The session goes through jump hosts or a proxy. */
export function hasRoute(s: Pick<SessionConfig, 'jump_chain' | 'proxy'>): boolean {
	return (s.jump_chain?.length ?? 0) > 0 || !!s.proxy?.host;
}

/** Other addresses only count for SSH sessions for now. */
export function hasAltAddresses(s: AddressSession): boolean {
	return (s.kind ?? 'ssh') === 'ssh' && (s.alt_addresses?.some((a) => a.host.trim()) ?? false);
}

/** Whether a connect checks which addresses answer first: the session's
 *  own choice, else the setting (`checkByDefault`). Off is silent mode:
 *  nothing is sent to an address until it is tried. */
export function checksAddresses(s: Pick<SessionConfig, 'address_check'>, checkByDefault: boolean): boolean {
	return s.address_check ?? checkByDefault;
}

/** Silent mode's order: by priority, the address that worked last first. */
export function plainOrder(addresses: ResolvedAddress[], lastGood?: string | null): ResolvedAddress[] {
	const list = [...addresses];
	const at = lastGood ? list.findIndex((a) => addressKey(a) === lastGood) : -1;
	if (at > 0) list.unshift(...list.splice(at, 1));
	return list;
}

export function connectMode(s: AddressSession): ConnectMode {
	if (!hasAltAddresses(s)) return 'main';
	return s.connect_mode === 'ask' || s.connect_mode === 'main' ? s.connect_mode : 'auto';
}

/** Every address, main first. */
export function sessionAddresses(s: AddressSession): ResolvedAddress[] {
	const routed = hasRoute(s);
	const wg = s.wireguard_id?.trim() || undefined;
	// Inside a tunnel nothing can be checked from here.
	const out: ResolvedAddress[] = [{ index: 0, label: s.host_label?.trim() ?? '', host: s.host, port: s.port, routed: routed || !!wg, ...(wg ? { wireguardId: wg } : {}) }];
	if (!hasAltAddresses(s)) return out;
	(s.alt_addresses ?? []).forEach((a: SessionAddress, i) => {
		const host = a.host.trim();
		if (!host) return;
		const tunnel = a.via_wireguard?.trim() || undefined;
		if (tunnel) {
			out.push({ index: i + 1, label: a.label?.trim() ?? '', host, port: a.port || s.port, routed: true, wireguardId: tunnel, tunnelOnly: true });
			return;
		}
		const via = a.via_session_id?.trim() || undefined;
		out.push({ index: i + 1, label: a.label?.trim() ?? '', host, port: a.port || s.port, routed: via ? true : routed && !a.direct, viaSessionId: via });
	});
	return out;
}

/** `host:port`, as the last good address is remembered under. */
export function addressKey(a: Pick<ResolvedAddress, 'host' | 'port'>): string {
	return a.host.includes(':') ? `[${a.host}]:${a.port}` : `${a.host}:${a.port}`;
}

/** `host` or `host:port` when the port is not the session's own. */
export function addressText(a: Pick<ResolvedAddress, 'host' | 'port'>, sessionPort: number): string {
	if (a.port === sessionPort) return a.host;
	return addressKey(a);
}

/** The label, else the address itself. */
export function addressName(a: ResolvedAddress, sessionPort: number): string {
	return a.label || addressText(a, sessionPort);
}

/** The label with the address beside it, else the address alone. */
export function addressLabelled(a: ResolvedAddress, sessionPort: number): string {
	const text = addressText(a, sessionPort);
	return a.label ? `${a.label} (${text})` : text;
}

/**
 * The order Auto tries the addresses in. `probes[i]` is what the check of
 * `addresses[i]` found: milliseconds, or null when it did not answer
 * (routed ones are not checked and may hold anything). The address that
 * worked last on this device goes first among those of its group.
 */
export function autoOrder(addresses: ResolvedAddress[], probes: (number | null | undefined)[], lastGood?: string | null): ResolvedAddress[] {
	const answered: ResolvedAddress[] = [];
	const routed: ResolvedAddress[] = [];
	const silent: ResolvedAddress[] = [];
	addresses.forEach((a, i) => {
		if (a.routed) routed.push(a);
		else if (probes[i] != null) answered.push(a);
		else silent.push(a);
	});
	const lastFirst = (list: ResolvedAddress[]) => {
		const at = lastGood ? list.findIndex((a) => addressKey(a) === lastGood) : -1;
		if (at > 0) list.unshift(...list.splice(at, 1));
		return list;
	};
	return [...lastFirst(answered), ...lastFirst(routed), ...lastFirst(silent)];
}

/**
 * Whether a connect error means the address could not be reached (so the
 * next one is worth a try), rather than a login, key or host key problem.
 */
export function isUnreachable(err: string): boolean {
	if (!/^(Error: )?Connection failed/i.test(err.trim())) return false;
	// A host key turned down, a login or a key: the server was reached.
	if (/auth|passphrase|host key|server key|unknown key|identity|key file|rejected|denied|permission/i.test(err)) return false;
	return /timed? ?out|refused|unreachable|no route|resolve|lookup|not known|nodename|no such host|dns|reset|broken pipe|network|connection closed|disconnect|eof/i.test(err);
}

// --- The last address that worked, per device ---------------------------

const LAST_KEY = 'reach-last-address';

function readLast(): Record<string, string> {
	try {
		const raw = localStorage.getItem(LAST_KEY);
		const v = raw ? JSON.parse(raw) : {};
		return v && typeof v === 'object' ? v : {};
	} catch {
		return {};
	}
}

/** Kept on this device only: another device is on another network. */
export function lastAddress(sessionId: string): string | null {
	return readLast()[sessionId] ?? null;
}

export function rememberAddress(sessionId: string, a: ResolvedAddress): void {
	try {
		const all = readLast();
		all[sessionId] = addressKey(a);
		localStorage.setItem(LAST_KEY, JSON.stringify(all));
	} catch {
		// Nothing to keep it in: Auto still works, just without the hint.
	}
}
