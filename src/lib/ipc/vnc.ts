import { Channel, invoke } from '@tauri-apps/api/core';
import type { MouseAction, RdpStatus } from '#lib/ipc/rdp.js';
import type { SecretRef } from '#lib/ipc/pwmanager.js';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';
import type { ProxmoxConsole } from '#lib/ipc/proxmox.js';

/**
 * Remote desktop over VNC. The backend sends frames in the RDP wire format
 * (see src-tauri/src/vnc), so the same panel paints both; only the commands
 * differ. Keys go as X11 keysyms; see `#lib/vnc/keysym.js`.
 */

export interface VncConnectParams {
	id: string;
	host: string;
	port: number;
	/** A macOS user name: a Mac is then logged in to with that account. Empty for a VNC password. */
	username?: string;
	/** The VNC password, or with `username` the macOS password; empty for a server that asks for none. */
	password: string;
	/** A password-manager entry, resolved by the backend at connect; wins over `password`. */
	passwordRef?: SecretRef | null;
	/** The saved session this desktop is for; required with `passwordRef` (the backend takes the entry from it). */
	sessionId?: string | null;
	/** A saved SSH session to reach the server through. */
	viaSessionId?: string | null;
	/** A Proxmox guest's display: opened through the Proxmox session's tab,
	 *  with a password made for each connect. */
	proxmox?: ProxmoxConsole | null;
	/** The whole connection in TLS (an iDRAC's VNC server with SSL encryption on). */
	tls?: boolean;
	/** VeNCrypt: TLS negotiated inside VNC, required. */
	vencrypt?: boolean;
	/** A server management session's VNC server: a refused password asks
	 *  for another one there. */
	bmcSessionId?: string;
}

/** How a connected VNC session is protected on the network. */
export interface VncSecurity {
	/** `tls`, `vencrypt` (TLS inside VNC), `ssh` (through an SSH session), `proxmox` (inside its HTTPS) or `none`. */
	kind: 'tls' | 'vencrypt' | 'ssh' | 'proxmox' | 'none';
	protocol: string | null;
	cipher: string | null;
	bits: number | null;
}

/** `null` before the session is up. */
export async function vncSecurity(id: string): Promise<VncSecurity | null> {
	return invoke<VncSecurity | null>('vnc_security', { id });
}

export type VncStatus = RdpStatus;

/** A connect error starting with this is a certificate the session does not
 *  trust yet; its SHA-256 fingerprint follows. */
export const VNC_UNTRUSTED_CERT = 'VNC_UNTRUSTED_CERT:';

/** Trust a certificate for a saved VNC session, by the fingerprint its
 *  failed connect reported. */
export async function vncTrustCert(sessionId: string, fingerprint: string): Promise<void> {
	await invoke('vnc_session_cert_trust', { sessionId, fingerprint });
}

export async function vncConnect(params: VncConnectParams, onFrame: (frame: ArrayBuffer) => void): Promise<void> {
	const onFrameChannel = new Channel<ArrayBuffer | Uint8Array | number[]>();
	onFrameChannel.onmessage = (data) => {
		if (data instanceof ArrayBuffer) {
			onFrame(data);
			return;
		}
		const bytes = ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : Uint8Array.from(data);
		const copy = new ArrayBuffer(bytes.byteLength);
		new Uint8Array(copy).set(bytes);
		onFrame(copy);
	};
	const { sessionId, bmcSessionId: _bmc, ...rest } = params;
	await ensureSessionWireguard(sessionId, params.host, params.port);
	await invoke('vnc_connect', { params: rest, sessionId: sessionId ?? null, onFrame: onFrameChannel });
}

export async function vncDisconnect(id: string): Promise<void> {
	await invoke('vnc_disconnect', { id });
}

export async function vncDisconnectAll(): Promise<void> {
	await invoke('vnc_disconnect_all');
}

export async function vncAck(id: string): Promise<void> {
	await invoke('vnc_ack', { id });
}

/** As `rdpMouse`: `button` 0 left, 1 middle, 2 right; `delta` positive for away. */
export async function vncMouse(id: string, x: number, y: number, action: MouseAction, button = 0, delta = 0): Promise<void> {
	await invoke('vnc_mouse', { id, x, y, action, button, delta });
}

export async function vncKey(id: string, keysym: number, down: boolean): Promise<void> {
	await invoke('vnc_key', { id, keysym, down });
}

export async function vncResize(id: string, width: number, height: number): Promise<void> {
	await invoke('vnc_resize', { id, width, height });
}

/** The desktop gained focus: offer the local clipboard's text to the server. */
export async function vncClipboardSync(id: string): Promise<void> {
	await invoke('vnc_clipboard_sync', { id });
}
