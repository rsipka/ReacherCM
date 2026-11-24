import { Channel, invoke } from '@tauri-apps/api/core';
import type { MouseAction, RdpStatus } from '#lib/ipc/rdp.js';
import type { SecretRef } from '#lib/ipc/pwmanager.js';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

/**
 * Remote desktop over SPICE: KVM/QEMU virtual machines (Proxmox VE, libvirt,
 * oVirt). The backend sends frames in the RDP wire format (see
 * src-tauri/src/spice), so the same panel paints it; only the commands
 * differ. Keys go as PC/AT scancodes, as for RDP.
 */

export interface SpiceConnectParams {
	id: string;
	host: string;
	/** The plain port; unused when `tlsPort` is set. */
	port: number;
	/** The SPICE password (ticket); empty for a server that asks for none. */
	password: string;
	/** A password-manager entry, resolved by the backend at connect; wins over `password`. */
	passwordRef?: SecretRef | null;
	/** The saved session this desktop is for; required with `passwordRef`. */
	sessionId?: string | null;
	/** A saved SSH session to reach the server through. */
	viaSessionId?: string | null;
	/** The TLS port: every channel goes over TLS to it. */
	tlsPort?: number | null;
	/** The CA that signed the server's certificate, PEM. */
	ca?: string | null;
	/** The certificate subject the server must have (`C=..,O=..,CN=..`). */
	hostSubject?: string | null;
	/** An HTTP proxy to go through (Proxmox's spiceproxy); from a .vv file only. */
	proxy?: string | null;
}

/** A saved SPICE session's own settings (snake_case, as stored). */
export interface SpiceOptions {
	tls_port?: number | null;
	ca?: string | null;
	host_subject?: string | null;
}

/** What a virt-viewer (.vv) file says to connect to. */
export interface VvFile {
	host: string;
	port: number;
	tlsPort: number | null;
	password: string;
	ca: string | null;
	hostSubject: string | null;
	proxy: string | null;
	title: string | null;
	deleteAfter: boolean;
}

export type SpiceStatus = RdpStatus;

export async function spiceConnect(params: SpiceConnectParams, onFrame: (frame: ArrayBuffer) => void): Promise<void> {
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
	const { sessionId, ...rest } = params;
	await ensureSessionWireguard(sessionId, params.host, params.port);
	await invoke('spice_connect', { params: rest, sessionId: sessionId ?? null, onFrame: onFrameChannel });
}

export async function spiceDisconnect(id: string): Promise<void> {
	await invoke('spice_disconnect', { id });
}

export async function spiceDisconnectAll(): Promise<void> {
	await invoke('spice_disconnect_all');
}

export async function spiceAck(id: string): Promise<void> {
	await invoke('spice_ack', { id });
}

/** As `rdpMouse`: `button` 0 left, 1 middle, 2 right; `delta` positive for away. */
export async function spiceMouse(id: string, x: number, y: number, action: MouseAction, button = 0, delta = 0): Promise<void> {
	await invoke('spice_mouse', { id, x, y, action, button, delta });
}

/** A key by its PC/AT set-1 scancode, as `rdpKey`. */
export async function spiceKey(id: string, scancode: number, extended: boolean, release: boolean): Promise<void> {
	await invoke('spice_key', { id, scancode, extended, release });
}

/** Text with no key behind it (a phone keyboard), typed as US-layout keystrokes. */
export async function spiceType(id: string, text: string): Promise<void> {
	await invoke('spice_type', { id, text });
}

export async function spiceResize(id: string, width: number, height: number): Promise<void> {
	await invoke('spice_resize', { id, width, height });
}

/** The desktop gained focus: offer the local clipboard's text to the guest. */
export async function spiceClipboardSync(id: string): Promise<void> {
	await invoke('spice_clipboard_sync', { id });
}

/** Read a virt-viewer (.vv) file the user picked. */
export async function spiceReadVv(path: string): Promise<VvFile> {
	return await invoke<VvFile>('spice_read_vv', { path });
}

/** The connect parameters a saved SPICE session's options add. */
export function spiceParamsFromOptions(o: SpiceOptions | null | undefined): Pick<SpiceConnectParams, 'tlsPort' | 'ca' | 'hostSubject'> {
	return { tlsPort: o?.tls_port ?? null, ca: o?.ca ?? null, hostSubject: o?.host_subject ?? null };
}
