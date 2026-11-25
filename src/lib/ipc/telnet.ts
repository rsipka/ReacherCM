import { invoke } from '@tauri-apps/api/core';
import type { SecretRef } from './pwmanager';
import { ensureWireguard } from '#lib/state/wireguard.svelte.js';
import type { ProxmoxConsole } from './proxmox';

/** How a Telnet session talks to the device (see src-tauri/src/telnet/protocol.rs). */
export interface TelnetOptions {
	/** active: Reach asks for the options it wants; passive: it only answers;
	 *  raw: plain TCP, no Telnet (a console server's raw port). */
	mode: 'active' | 'passive' | 'raw';
	/** What Enter sends. */
	newline: 'crlf' | 'cr' | 'lf';
	/** What Backspace sends. */
	backspace: 'del' | 'ctrlh';
}

export const DEFAULT_TELNET_OPTIONS: TelnetOptions = { mode: 'active', newline: 'crlf', backspace: 'del' };

export interface TelnetConnectParams {
	host: string;
	port: number;
	options: TelnetOptions;
	/** Typed at the device's login prompt, if it shows one. */
	username?: string | null;
	/** Typed at the device's password prompt. */
	password?: string | null;
	/** A password manager entry; the backend fetches it. */
	passwordRef?: SecretRef | null;
	/** The saved session, which a password manager entry must come from. */
	sessionId?: string | null;
	/** A WireGuard profile whose tunnel carries the connection. */
	wireguard?: string | null;
	/** A Proxmox text console instead of a Telnet device (container,
	 *  serial port, node shell), through the Proxmox session's tab. */
	proxmox?: ProxmoxConsole | null;
}

/** Connect; output then arrives as `telnet-data-{id}` until `telnet-exit-{id}`. */
export async function telnetConnect(id: string, params: TelnetConnectParams, cols: number, rows: number): Promise<void> {
	// Without the tunnel when the person chose to connect straight.
	const wireguard = params.wireguard && (await ensureWireguard(params.wireguard)) ? params.wireguard : null;
	return invoke('telnet_connect', {
		params: {
			id,
			host: params.host,
			port: params.port,
			cols,
			rows,
			options: params.options,
			username: params.username || null,
			password: params.password || null,
			passwordRef: params.passwordRef ?? null,
			wireguard,
			proxmox: params.proxmox ?? null,
		},
		sessionId: params.sessionId ?? null,
	});
}

export async function telnetSend(id: string, data: number[]): Promise<void> {
	return invoke('telnet_send', { id, data });
}

export async function telnetResize(id: string, cols: number, rows: number): Promise<void> {
	return invoke('telnet_resize', { id, cols, rows });
}

export async function telnetDisconnect(id: string): Promise<void> {
	return invoke('telnet_disconnect', { id });
}
