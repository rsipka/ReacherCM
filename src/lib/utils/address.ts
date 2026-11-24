/**
 * Reading `user@host:port` typed into a search or quick-connect field.
 *
 * Deliberately conservative: a session called "a.b" must stay a search,
 * never a connection. It takes one unmistakable sign of an address: a user,
 * a port, an IPv4 address, or a dotted name ending in something TLD-shaped.
 */

export interface TypedAddress {
	host: string;
	username?: string;
	port?: number;
}

const IPV4 = /^\d{1,3}(\.\d{1,3}){3}$/;

export function parseAddress(raw: string): TypedAddress | null {
	const text = raw.trim();
	if (!text || /\s/.test(text)) return null;
	let username: string | undefined;
	let rest = text;
	const at = text.lastIndexOf('@');
	if (at > 0) {
		username = text.slice(0, at);
		rest = text.slice(at + 1);
	}
	let port: number | undefined;
	const withPort = rest.match(/^(.+):(\d{1,5})$/);
	if (withPort) {
		rest = withPort[1];
		port = Number(withPort[2]);
	}
	if (!rest || !/^[a-z0-9][a-z0-9.-]*$/i.test(rest)) return null;
	const unmistakable = username !== undefined || port !== undefined || IPV4.test(rest) || /\.[a-z]{2,}$/i.test(rest);
	return unmistakable ? { host: rest, username, port } : null;
}

/** The port a session of this kind uses unless told otherwise. */
export function defaultPort(kind: string | undefined): number {
	return kind === 'rdp' ? 3389 : kind === 'vnc' || kind === 'spice' ? 5900 : kind === 'telnet' ? 23 : kind === 'winbox' ? 8291 : kind === 'web' || kind === 'bmc' ? 443 : kind === 'proxmox' ? 8006 : kind === 'ftp' ? 21 : 22;
}

/** `user@host`, with `:port` only when it is not the protocol's own, as the
 *  session list, the start screen and the palette all show a session. A
 *  serial session: its port and line settings, `COM3 · 9600 8N1`. */
export function sessionAddress(s: {
	username?: string | null;
	host: string;
	port?: number | null;
	kind?: string;
	serial?: { baud_rate?: number; data_bits?: number; parity?: string; stop_bits?: number } | null;
	web?: { scheme?: string | null; path?: string | null } | null;
}): string {
	if (s.kind === 'web') {
		// As the window opens it: the address, not a login.
		const http = s.web?.scheme === 'http';
		const host = s.host.includes(':') ? `[${s.host}]` : s.host;
		const port = s.port && s.port !== (http ? 80 : 443) ? `:${s.port}` : '';
		const path = s.web?.path && s.web.path !== '/' ? s.web.path : '';
		return `${http ? 'http' : 'https'}://${host}${port}${path}`;
	}
	if (s.kind === 'serial') {
		const o = s.serial ?? {};
		return `${s.host} · ${o.baud_rate ?? 9600} ${o.data_bits ?? 8}${(o.parity ?? 'none')[0].toUpperCase()}${o.stop_bits ?? 1}`;
	}
	const user = s.username ? `${s.username}@` : '';
	const port = s.port && s.port !== defaultPort(s.kind) ? `:${s.port}` : '';
	return `${user}${s.host}${port}`;
}
