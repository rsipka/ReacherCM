/** How a connection reaches its server when not directly, as short text for
 *  tooltips: the session list and the terminal bar say it the same way. */

export interface Hop {
	host: string;
	port?: number | null;
	username?: string | null;
}

/** `user@host → user@host2:2222`: the port only when it is not ssh's own. */
export function formatHops(hops: Hop[]): string {
	return hops.map((j) => `${j.username ? `${j.username}@` : ''}${j.host}${j.port && j.port !== 22 ? `:${j.port}` : ''}`).join(' → ');
}

export interface ProxyLike {
	proxy_type: string;
	host: string;
	port: number;
}

export function formatProxyAddress(p: ProxyLike): string {
	return `${p.host}:${p.port}`;
}

/** SVG paths of the marks: a route for jump hosts, a globe for a proxy, a
 *  shield for a WireGuard tunnel. */
export const ROUTE_ICON = {
	jump: 'M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15M6 16a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 2a3 3 0 1 0 0 6 3 3 0 0 0 0-6z',
	proxy: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3a14 14 0 0 1 0 18a14 14 0 0 1 0-18',
	/** A shield with a lock's keyhole: a WireGuard tunnel. */
	wireguard: 'M12 3l7 3v6c0 4.5-3 7.6-7 9-4-1.4-7-4.5-7-9V6zM12 10v4'
};

/** The jump chain a session editor saves: none for a desktop session, when
 *  "Use jump host" is off, or when every hop was removed. Never the chain the
 *  session had before, so taking jump hosts away sticks. */
export function jumpChainToSave<T>(desktop: boolean, enabled: boolean, hops: T[]): T[] | null {
	return !desktop && enabled && hops.length > 0 ? hops : null;
}
