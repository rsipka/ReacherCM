import { Channel, invoke } from '@tauri-apps/api/core';
import type { SecretRef } from '#lib/ipc/pwmanager.js';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

/**
 * Remote desktop over RDP.
 *
 * Frames do not come back from `invoke`; they arrive on a Channel for the life
 * of the session, as raw bytes rather than JSON. See src-tauri/src/rdp for the
 * wire format and the reasoning. Everything else here is one command each.
 */

export interface RdpConnectParams {
	id: string;
	host: string;
	port: number;
	username: string;
	password: string;
	/** A password-manager entry, resolved by the backend at connect; wins over `password`. */
	passwordRef?: SecretRef | null;
	/** The saved session this desktop is for; required with `passwordRef` (the backend takes the entry from it). */
	sessionId?: string | null;
	domain?: string;
	/** A local folder to show inside the desktop as a drive. */
	sharePath?: string;
	/** Advertise the graphics pipeline; see the setting. */
	graphicsPipeline?: boolean;
	/** Keep the graphics pipeline to the older picture formats; see the setting. */
	olderGraphicsFormats?: boolean;
	/** What the clipboard carries: unset for everything, `text` for no files, `off` for nothing. */
	clipboard?: RdpClipboardMode | null;
	/** Whether the desktop panel forwards lock key presses. Enabled by default. */
	forwardLockKeys?: boolean;
	/** The saved session's own settings. */
	options?: RdpOptions | null;
	/** The remote's display scaling in percent; worked out by the panel. */
	scale?: number | null;
	/** The size the panel can show. The server renders at this size. */
	width: number;
	height: number;
}

/**
 * A saved RDP session's own settings (src-tauri/src/rdp/options.rs). Every
 * field left out keeps the default.
 */
export interface RdpOptions {
	/** A fixed desktop size, `1920x1080`. Absent: follows the panel. */
	resolution?: string | null;
	/** Display scaling in percent, 100 to 500. Absent: the screen's own. */
	scale?: number | null;
	/** 16 or 32. */
	color_depth?: number | null;
	/** Windows keyboard layout id, 0x0405 for Czech. Absent: the server's. */
	keyboard_layout?: number | null;
	sound?: 'off' | null;
	audio_quality?: 'dynamic' | 'medium' | 'high' | null;
	performance?: 'fast' | 'best' | null;
	/** False turns compression off. */
	compression?: boolean | null;
	/** Seconds between keep-alive input while idle. */
	keepalive?: number | null;
	/** Seconds between small mouse moves while idle. */
	jiggler?: number | null;
	admin?: boolean | null;
	/** Started instead of the desktop. */
	program?: string | null;
	work_dir?: string | null;
}

/** `1920x1080` as width and height, or null when it is not a size the server takes. */
export function parseResolution(text: string | null | undefined): { width: number; height: number } | null {
	const m = /^\s*(\d{3,4})\s*[xX×]\s*(\d{3,4})\s*$/.exec(text ?? '');
	if (!m) return null;
	const width = Number(m[1]);
	const height = Number(m[2]);
	const ok = (v: number) => v >= 200 && v <= 8192;
	return ok(width) && ok(height) ? { width: width & ~1, height: height & ~1 } : null;
}

/** A session's clipboard choice, as saved with it; absent means everything. */
export type RdpClipboardMode = 'text' | 'off';

/** What the clipboard reports, on `rdp-clipboard-{id}`. `incoming` is from the desktop to here. */
export type RdpClipEvent =
	| { kind: 'progress'; incoming: boolean; done: number; total: number; files: number }
	| { kind: 'finished'; incoming: boolean; files: number }
	| { kind: 'cancelled'; incoming: boolean }
	| { kind: 'failed'; incoming: boolean; reason: string }
	| { kind: 'held'; files: number; total: number }
	| { kind: 'heldGone' };

export type RdpStatus =
	| { state: 'connected' }
	| { state: 'loggedIn' }
	/** `clean`: the user logged off or disconnected from the desktop itself. */
	| { state: 'closed'; reason: string; clean: boolean }
	| { state: 'error'; message: string };

/** Frame kinds, matching `kind` in src-tauri/src/rdp/mod.rs. */
export const FRAME_REGION = 1;
export const FRAME_FULL = 2;
export const FRAME_POINTER = 3;
export const FRAME_POINTER_SHAPE = 4;
export const FRAME_POINTER_HIDDEN = 5;
export const FRAME_POINTER_DEFAULT = 6;

/** Header length for REGION and FULL frames: kind + six u16 fields. */
export const FRAME_HEADER = 13;

/** Length of a POINTER frame: kind + two u16 fields. */
export const FRAME_POINTER_LEN = 5;

/** Header length of a POINTER_SHAPE frame: kind + w, h, hotspot x, hotspot y. */
export const FRAME_POINTER_SHAPE_HEADER = 9;

/**
 * Open a desktop. `onFrame` is called for every message until the session
 * ends. A message holds one or more frames back to back; after painting all
 * of them the caller must `rdpAck` it, or the next one never comes.
 *
 * The channel delivers whatever the runtime hands it; on every Tauri build
 * this has been an ArrayBuffer, but a typed array is accepted too so a
 * runtime change here fails loudly in one place rather than silently in the
 * frame parser.
 */
export async function rdpConnect(
	params: RdpConnectParams,
	onFrame: (frame: ArrayBuffer) => void
): Promise<void> {
	const onFrameChannel = new Channel<ArrayBuffer | Uint8Array | number[]>();
	onFrameChannel.onmessage = (data) => {
		if (data instanceof ArrayBuffer) {
			onFrame(data);
			return;
		}
		// A view's `.buffer` may be a SharedArrayBuffer as far as the types
		// know, and may be larger than the view; copying into a fresh buffer
		// settles both and costs one memcpy per frame.
		const bytes = ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : Uint8Array.from(data);
		const copy = new ArrayBuffer(bytes.byteLength);
		new Uint8Array(copy).set(bytes);
		onFrame(copy);
	};
	const { sessionId, forwardLockKeys: _forwardLockKeys, ...rest } = params;
	await ensureSessionWireguard(sessionId, params.host, params.port);
	await invoke('rdp_connect', { params: rest, sessionId: sessionId ?? null, onFrame: onFrameChannel });
}

export async function rdpDisconnect(id: string): Promise<void> {
	await invoke('rdp_disconnect', { id });
}

/**
 * The desktop gained focus. The backend looks at the local clipboard and, if
 * it changed, offers it to the remote so a paste there finds it.
 */
export async function rdpClipboardSync(id: string): Promise<void> {
	await invoke('rdp_clipboard_sync', { id });
}

/** Files dropped on the desktop: put them on its clipboard. Only files that were really dropped are taken. */
export async function rdpClipboardSendFiles(id: string, paths: string[]): Promise<void> {
	await invoke('rdp_clipboard_send_files', { id, paths });
}

/** Download what the desktop copied that was too large to take unasked. */
export async function rdpClipboardFetch(id: string): Promise<void> {
	await invoke('rdp_clipboard_fetch', { id });
}

/** The share path that means the app's own folder in Files (iPhone and iPad). */
export const APP_FILES_SHARE = '@app-files';

/** Pick files with the system's picker and put them on the desktop's clipboard. Resolves to how many (0: none picked). */
export async function rdpClipboardPickFiles(id: string): Promise<number> {
	return invoke<number>('rdp_clipboard_pick_files', { id });
}

/** Save what the last download from the desktop brought, one Save dialog per file; folders are skipped. */
export async function rdpClipboardSaveReceived(id: string): Promise<{ saved: number; skipped: number }> {
	return invoke<{ saved: number; skipped: number }>('rdp_clipboard_save_received', { id });
}

/** Stop the clipboard transfer in progress, either way. */
export async function rdpClipboardCancel(id: string): Promise<void> {
	await invoke('rdp_clipboard_cancel', { id });
}

/** The app window goes full screen, or comes back, for a desktop. */
export async function rdpWindowFullscreen(on: boolean): Promise<void> {
	await invoke('rdp_window_fullscreen', { on });
}

/** Close every open desktop; for the way out of the app. */
export async function rdpDisconnectAll(): Promise<void> {
	await invoke('rdp_disconnect_all');
}

/**
 * One message has been painted. The backend keeps at most two unacknowledged
 * messages in flight and folds everything else into the next one, so this is
 * what turns a busy screen into fewer, larger updates instead of a backlog.
 */
export async function rdpAck(id: string): Promise<void> {
	await invoke('rdp_ack', { id });
}

export type MouseAction = 'move' | 'down' | 'up' | 'wheel';

/** `button`: 0 left, 1 middle, 2 right. `delta`: wheel step, negative for away. */
export async function rdpMouse(
	id: string,
	x: number,
	y: number,
	action: MouseAction,
	button = 0,
	delta = 0
): Promise<void> {
	await invoke('rdp_mouse', { id, x, y, action, button, delta });
}

/** A PC/AT set-1 scancode, `extended` for the E0-prefixed keys. */
export async function rdpKey(id: string, scancode: number, extended: boolean, release: boolean): Promise<void> {
	await invoke('rdp_key', { id, scancode, extended, release });
}

/** Set the remote's CapsLock, NumLock and ScrollLock to these states. */
export async function rdpSyncLocks(id: string, caps: boolean, num: boolean, scroll: boolean): Promise<void> {
	await invoke('rdp_sync_locks', { id, caps, num, scroll });
}

/** The local NumLock as the system shows it (Linux), or null if unknown. */
export async function rdpLocalNumlock(): Promise<boolean | null> {
	return (await invoke<boolean | null>('rdp_local_numlock')) ?? null;
}

/** A character rather than a key, for text with no scancode here. */
export async function rdpUnicode(id: string, code: number, release: boolean): Promise<void> {
	await invoke('rdp_unicode', { id, code, release });
}

export async function rdpResize(id: string, width: number, height: number): Promise<void> {
	await invoke('rdp_resize', { id, width, height });
}
