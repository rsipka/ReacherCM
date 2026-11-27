<script lang="ts">
	/**
	 * A remote desktop in a tab.
	 *
	 * The canvas is the framebuffer at the server's size; CSS scales it to fit
	 * the panel without stretching, and every pointer coordinate is mapped back
	 * through that scale before it is sent. Frames arrive as raw RGBA regions
	 * and are painted with putImageData at the region's offset — no full-frame
	 * redraw unless the server sent one.
	 *
	 * Keyboard events are translated from KeyboardEvent.code to PC/AT set-1
	 * scancodes, which is what RDP wants: the key's position, not the character
	 * it produced. That is why Ctrl+C reaches the remote as Ctrl+C rather than
	 * being eaten here as a copy. Characters with no scancode on this keyboard
	 * (an IME commit, a pasted glyph) go as Unicode instead.
	 *
	 * A VNC desktop uses this same panel: its backend sends frames in the same
	 * format, and only the commands differ. VNC wants the character a key
	 * produced, not its position, so there keys go as X11 keysyms.
	 *
	 * A SPICE desktop (a KVM/QEMU virtual machine) does too. SPICE takes keys
	 * by position like RDP, so it gets the same scancodes; text with no key
	 * behind it is typed as US-layout keystrokes.
	 */
	import { onDestroy, onMount } from 'svelte';
	import { registerCapture } from '#lib/state/capture.js';
	import { KEY_COMBOS, registerSendKeys, type KeyCombo } from '#lib/state/send-keys.js';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { getCurrentWebview } from '@tauri-apps/api/webview';
	import { currentMonitor, getCurrentWindow, type Monitor } from '@tauri-apps/api/window';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import {
		FRAME_FULL,
		FRAME_HEADER,
		FRAME_POINTER,
		FRAME_POINTER_DEFAULT,
		FRAME_POINTER_HIDDEN,
		FRAME_POINTER_LEN,
		FRAME_POINTER_SHAPE,
		FRAME_POINTER_SHAPE_HEADER,
		FRAME_REGION,
		rdpAck,
		rdpClipboardCancel,
		rdpClipboardFetch,
		rdpClipboardPickFiles,
		rdpClipboardSaveReceived,
		rdpClipboardSendFiles,
		rdpClipboardSync,
		rdpConnect,
		rdpDisconnect,
		rdpKey,
		rdpMouse,
		rdpResize,
		rdpSyncLocks,
		rdpWindowFullscreen,
		rdpUnicode,
		parseResolution,
		rdpRailCommand,
		rdpRailWindows,
		rdpViewAck,
		rdpViewAttach,
		rdpViewDetach,
		railWindowsSync,
		type RdpRailWindow,
		type MouseAction,
		type RdpClipEvent,
		type RdpConnectParams,
		type RdpStatus,
	} from '#lib/ipc/rdp.js';
	import {
		vncAck,
		vncClipboardSync,
		vncConnect,
		vncKey,
		vncMouse,
		vncResize,
		vncTrustCert,
		VNC_UNTRUSTED_CERT,
		type VncConnectParams,
	} from '#lib/ipc/vnc.js';
	import {
		spiceAck,
		spiceClipboardSync,
		spiceConnect,
		spiceKey,
		spiceMouse,
		spiceResize,
		spiceType,
		type SpiceConnectParams,
	} from '#lib/ipc/spice.js';
	import { keysymForChar, keysymForCode, keysymForEvent } from '#lib/vnc/keysym.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { requestSessionAction } from '#lib/state/palette.svelte.js';
	import { untrustedRefsFromError } from '#lib/ipc/sessions.js';
	import { getSettings, updateSetting } from '#lib/state/settings.svelte.js';
	import { isMac, isMobile } from '#lib/platform.js';
	import { isLockKey, localLocks, onLocksChange, type Locks } from '#lib/utils/lock-keys.js';
	import { setTabLink, setDesktopUp, closeTab, getTabs } from '#lib/state/tabs.svelte.js';
	import MobileKeyBar, { type BarKey } from '#lib/components/shared/MobileKeyBar.svelte';

	interface Props {
		id: string;
		params: RdpConnectParams | VncConnectParams | SpiceConnectParams;
		/** Which protocol the desktop speaks. RDP unless said otherwise. */
		protocol?: 'rdp' | 'vnc' | 'spice';
		active: boolean;
		/**
		 * A RemoteApp window of its own: the session's picture shown from
		 * this remote window's place, in a window of this computer. Unset:
		 * the session's tab, which connects.
		 */
		railView?: number;
	}

	let { id, params, protocol = 'rdp', active, railView }: Props = $props();
	/** This view's name to the session's frame pump. */
	const viewKey = $derived(railView === undefined ? '' : `${id}-${railView}`);

	const isVnc = (): boolean => protocol === 'vnc';
	const isSpice = (): boolean => protocol === 'spice';
	const forwardLockKeys = $derived(protocol === 'rdp' && (params as RdpConnectParams).forwardLockKeys !== false);
	/** Who the desktop is, for the full-screen bar. */
	const label = $derived('username' in params && params.username ? `${params.username}@${params.host}` : params.host);

	/** The commands both protocols have, each sent the right way. */
	const remote = {
		ack: () => (railView !== undefined ? rdpViewAck(id, viewKey) : isVnc() ? vncAck(id) : isSpice() ? spiceAck(id) : rdpAck(id)),
		mouse: (x: number, y: number, action: MouseAction, button = 0, delta = 0) =>
			isVnc()
				? vncMouse(id, x, y, action, button, delta)
				: isSpice()
					? spiceMouse(id, x, y, action, button, delta)
					: rdpMouse(id, x, y, action, button, delta),
		resize: (width: number, height: number) =>
			isVnc() ? vncResize(id, width, height) : isSpice() ? spiceResize(id, width, height) : rdpResize(id, width, height),
		clipboardSync: () => (isVnc() ? vncClipboardSync(id) : isSpice() ? spiceClipboardSync(id) : rdpClipboardSync(id)),
		/** A key by its scancode (RDP and SPICE). */
		key: (scancode: number, extended: boolean, release: boolean) =>
			isSpice() ? spiceKey(id, scancode, extended, release) : rdpKey(id, scancode, extended, release),
	};

	/**
	 * Fire off one piece of input. Input is best effort: once the desktop has
	 * ended (the server logged the user off, say) the backend refuses it, and
	 * every mouse move would otherwise be an unhandled rejection.
	 */
	function sendInput(input: () => Promise<unknown>): Promise<void> {
		// One at a time, in order. Each IPC call runs as its own task on the
		// backend, so two sent together can arrive the other way round: a key
		// up overtaking its key down leaves the key held there, and a server
		// that repeats held keys itself (GNOME Remote Desktop) then types it
		// over and over until the next key comes.
		const sent = inputQueue.then(input).then(
			() => {},
			() => {},
		);
		inputQueue = sent;
		return sent;
	}
	let inputQueue: Promise<void> = Promise.resolve();

	/** Press or release a key named by its position (`KeyboardEvent.code`). */
	async function pressCode(code: string, down: boolean): Promise<void> {
		if (isVnc()) {
			const keysym = keysymForCode(code);
			if (keysym !== null) await vncKey(id, keysym, down);
			return;
		}
		const entry = SCANCODES[code];
		if (entry) await remote.key(entry[0], entry[1], !down);
	}

	/** Type one character that came without a key: an IME commit, a phone keyboard. */
	async function typeChar(point: number): Promise<void> {
		if (isVnc()) {
			const keysym = keysymForChar(String.fromCodePoint(point));
			if (keysym === null) return;
			await vncKey(id, keysym, true);
			await vncKey(id, keysym, false);
			return;
		}
		if (isSpice()) {
			await spiceType(id, String.fromCodePoint(point));
			return;
		}
		await rdpUnicode(id, point, false);
		await rdpUnicode(id, point, true);
	}

	let host: HTMLDivElement;
	let canvas: HTMLCanvasElement;
	let ctx: CanvasRenderingContext2D | null = null;

	type Phase = 'connecting' | 'connected' | 'closed' | 'error';
	let phase = $state<Phase>('connecting');
	// A desktop that closed or failed is marked on its tab, and quitting the
	// app does not count it as a live session.
	$effect(() => {
		setTabLink(id, phase === 'closed' || phase === 'error' ? 'down' : undefined);
		// The sessions list marks a session whose desktop is up.
		setDesktopUp(id, phase === 'connected');
	});
	let message = $state('');
	/** VNC: the error was a certificate the session does not trust yet; its
	 *  SHA-256 fingerprint, offered to trust. */
	let untrustedCert = $derived(isVnc() && message.startsWith(VNC_UNTRUSTED_CERT) ? message.slice(VNC_UNTRUSTED_CERT.length) : null);
	let certDeclined = $state(false);

	/** Trust the certificate for the saved session, and connect again. */
	async function trustCert(fingerprint: string): Promise<void> {
		const sessionId = (params as VncConnectParams).sessionId;
		if (!sessionId) return;
		try {
			await vncTrustCert(sessionId, fingerprint);
		} catch (err) {
			message = String(err);
			return;
		}
		const tab = getTabs().find((x) => x.id === id);
		if (tab) tab.reconnect = (tab.reconnect ?? 0) + 1;
	}
	// Closed because the user logged off or disconnected, not on an error.
	let ended = $state(false);

	let unlisten: UnlistenFn | null = null;
	let unlistenClip: UnlistenFn | null = null;
	let unlistenWindows: UnlistenFn | null = null;
	let unlistenFocus: UnlistenFn | null = null;
	/** A RemoteApp session: the program's windows, for the row above the picture. */
	const remoteApp = $derived(protocol === 'rdp' && (params as RdpConnectParams).options?.remote_app === true && !!(params as RdpConnectParams).options?.program?.trim());
	let railWindows = $state<RdpRailWindow[]>([]);
	/**
	 * Seamless: each of the program's windows in a window of its own here,
	 * the tab only holding the session. Desktops only.
	 */
	const seamless = $derived(remoteApp && railView === undefined && !isMobile() && (params as RdpConnectParams).options?.app_windows === 'separate');
	/** The screen the seamless windows are on; the remote desktop takes its size. */
	let monitor: Monitor | null = null;
	/** A view: the picture's size, and this screen's pixels per CSS pixel. */
	let fbSize = $state({ width: 0, height: 0 });
	const dpr = typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1;
	/** A view: where its remote window is on the remote desktop. */
	const viewRect = $derived(railView === undefined ? null : (railWindows.find((w) => w.id === railView) ?? null));

	/** Open, move and close the local windows to match the remote ones. */
	function placeWindows(list: RdpRailWindow[]): void {
		if (!seamless) return;
		const at = monitor?.position ?? { x: 0, y: 0 };
		// Bottom first, so that windows opened together stack as they do there.
		const places = list
			.filter((w) => w.state !== 'minimized' && w.width > 0 && w.height > 0)
			.sort((a, b) => b.z - a.z)
			.map((w) => ({ id: w.id, title: w.title, x: at.x + w.x, y: at.y + w.y, width: w.width, height: w.height, popup: !w.listed, active: w.active }));
		void railWindowsSync(id, places).catch(() => {});
	}

	/** A window button: bring it forward, or back from minimised. */
	function railClick(w: RdpRailWindow): void {
		void rdpRailCommand(id, w.id, w.state === 'minimized' ? 'restore' : 'activate').catch(() => {});
		canvas?.focus();
	}

	function railClose(w: RdpRailWindow): void {
		void rdpRailCommand(id, w.id, 'close').catch(() => {});
	}
	let unlistenDrop: UnlistenFn | null = null;

	/* --- clipboard transfers (RDP) ----------------------------------------- */

	/** What the session's clipboard carries; see the session editor. */
	const clipMode = $derived(isVnc() || isSpice() ? 'off' : ((params as RdpConnectParams).clipboard ?? 'all'));
	/** A file transfer in progress, for the bar at the bottom of the desktop. */
	let transfer = $state<{ incoming: boolean; done: number; total: number; files: number } | null>(null);
	/** Files the desktop copied that wait for a yes before coming down. */
	let heldCopy = $state<{ files: number; total: number } | null>(null);
	/** Files are being dragged over the desktop. */
	let dropping = $state(false);
	/** Phone: files came down from the desktop and wait to be saved. */
	let toSave = $state<number | null>(null);

	/** Phone: pick files and put them on the desktop's clipboard. */
	function pickFiles(): void {
		if (clipMode !== 'all') {
			addToast(t('rdp.drop_files_off'), 'info');
			return;
		}
		rdpClipboardPickFiles(id)
			.then((count) => {
				if (count > 0) addToast(t('rdp.pick_ready', { count }), 'success', 5000);
			})
			.catch((err) => addToast(String(err), 'error'));
	}

	/** Phone: save what came down, one Save dialog per file. */
	function saveReceived(): void {
		toSave = null;
		rdpClipboardSaveReceived(id)
			.then(({ saved, skipped }) => {
				if (saved > 0) addToast(t('rdp.saved', { count: saved }), 'success');
				if (skipped > 0) addToast(t('rdp.saved_folders_skipped', { count: skipped }), 'info', 6000);
			})
			.catch((err) => addToast(String(err), 'error'));
	}

	function formatBytes(n: number): string {
		if (n < 1024) return `${n} B`;
		const units = ['KB', 'MB', 'GB', 'TB'];
		let v = n / 1024;
		let i = 0;
		while (v >= 1024 && i < units.length - 1) {
			v /= 1024;
			i++;
		}
		return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
	}

	function onClipEvent(e: RdpClipEvent): void {
		switch (e.kind) {
			case 'progress':
				transfer = { incoming: e.incoming, done: e.done, total: e.total, files: e.files };
				break;
			case 'finished':
				// Only a transfer that was shown gets a word that it ended.
				if (e.incoming && onPhone) {
					// Nothing on a phone can paste files: offer to save them.
					toSave = e.files;
				} else if (transfer) {
					addToast(e.incoming ? t('rdp.clip_received', { count: e.files }) : t('rdp.clip_sent', { count: e.files }), 'success');
				}
				transfer = null;
				break;
			case 'cancelled':
				if (transfer) addToast(t('rdp.clip_cancelled'), 'info');
				transfer = null;
				break;
			case 'failed':
				addToast(t('rdp.clip_failed', { reason: e.reason }), 'error', 6000);
				transfer = null;
				break;
			case 'held':
				heldCopy = { files: e.files, total: e.total };
				break;
			case 'heldGone':
				heldCopy = null;
				break;
		}
	}

	function downloadHeld(): void {
		if (!heldCopy) return;
		transfer = { incoming: true, done: 0, total: heldCopy.total, files: heldCopy.files };
		heldCopy = null;
		void rdpClipboardFetch(id).catch(() => {});
	}

	function cancelTransfer(): void {
		void rdpClipboardCancel(id).catch(() => {});
	}

	/** Is a drop at this window position (physical pixels) on this desktop? */
	function overDesktop(position: { x: number; y: number }): boolean {
		if (!active || phase !== 'connected' || !screen) return false;
		const scale = window.devicePixelRatio || 1;
		const x = position.x / scale;
		const y = position.y / scale;
		const r = screen.getBoundingClientRect();
		return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
	}

	async function listenForDrops(): Promise<void> {
		unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
			const p = event.payload;
			if (p.type === 'leave') {
				dropping = false;
				return;
			}
			if (p.type === 'enter' || p.type === 'over') {
				dropping = overDesktop(p.position);
				return;
			}
			if (p.type !== 'drop') return;
			const here = overDesktop(p.position);
			dropping = false;
			if (!here || p.paths.length === 0) return;
			if (clipMode !== 'all') {
				addToast(t('rdp.drop_files_off'), 'info');
				return;
			}
			rdpClipboardSendFiles(id, p.paths)
				.then(() => {
					addToast(t('rdp.drop_ready', { count: p.paths.length }), 'success', 5000);
					canvas?.focus();
				})
				.catch((err) => addToast(String(err), 'error'));
		});
	}

	let sizeObserver: ResizeObserver | null = null;
	let resizeTimer: ReturnType<typeof setTimeout> | null = null;
	/**
	 * No resize goes to the server before this. Asked for within the first
	 * second after login, a resize is accepted but never answered — the
	 * server's display-control channel is still coming up — and the client
	 * can only give up on it and reconnect. Holding the request that long
	 * costs nothing anyone sees; the latest size wins when it fires.
	 */
	let resizeNotBefore = 0;
	const SETTLE_MS = 1500;


	/* --- full screen ------------------------------------------------------- */

	/**
	 * The desktop takes the whole window: the panel goes fixed over
	 * everything and the window itself goes full screen, so the remote is
	 * resized to the real screen. A bar at the top edge, shown briefly on
	 * entry and whenever the mouse touches the edge, is the way back;
	 * Ctrl+Alt+Enter toggles either way, as in Microsoft's client.
	 */
	let fullscreen = $state(false);
	let barShown = $state(false);
	let barTimer: ReturnType<typeof setTimeout> | null = null;

	/** Pinned, the bar stays; otherwise it tucks itself away again. */
	const barPinned = $derived(getSettings().rdpBarPinned);
	let keysOpen = $state(false);

	function showBar(): void {
		barShown = true;
		if (barTimer) clearTimeout(barTimer);
		barTimer = setTimeout(() => {
			// Not while its Keys menu is open, or it hides the menu too.
			if (!keysOpen) barShown = false;
		}, 2500);
	}

	function togglePin(): void {
		updateSetting('rdpBarPinned', !barPinned);
		// Unpinned, it stays a moment before it tucks away.
		showBar();
		canvas?.focus();
	}

	/** Keys this computer keeps for itself, from the menu: held in order,
	 *  let go in reverse. */
	async function sendCombo(codes: string[]): Promise<void> {
		for (const code of codes) await pressCode(code, true);
		for (const code of [...codes].reverse()) await pressCode(code, false);
	}

	function pickCombo(combo: KeyCombo): void {
		keysOpen = false;
		if (phase === 'connected') sendInput(() => sendCombo(combo.codes));
		canvas?.focus();
		showBar();
	}

	async function minimizeWindow(): Promise<void> {
		try {
			await getCurrentWindow().minimize();
		} catch (err) {
			console.error('minimise:', err);
		}
	}

	/**
	 * Where the panel sat before it was lifted to the document body. An
	 * ancestor of the panel clips fixed positioning to its own box — that is
	 * what `contain` and transforms do — so over-everything means out of
	 * that ancestor for the duration.
	 */
	let placeholder: Comment | null = null;

	function lift(on: boolean): void {
		if (on && !placeholder) {
			placeholder = document.createComment('rdp panel');
			host.parentNode?.insertBefore(placeholder, host);
			document.body.appendChild(host);
		} else if (!on && placeholder) {
			placeholder.parentNode?.insertBefore(host, placeholder);
			placeholder.remove();
			placeholder = null;
		}
	}

	async function setFullscreen(on: boolean): Promise<void> {
		if (fullscreen === on) return;
		fullscreen = on;
		lift(on);
		try {
			await rdpWindowFullscreen(on);
		} catch (err) {
			// The panel is over everything either way, which is most of the
			// point; the rest is said, not swallowed.
			console.error('full screen:', err);
		}
		if (on) showBar();
		else {
			keysOpen = false;
			if (barTimer) clearTimeout(barTimer);
		}
		// The panel's size just changed; the observer sends the new size.
		canvas?.focus();
	}

	/** A click anywhere but the Keys menu closes it. */
	function onHostDown(e: PointerEvent): void {
		if (keysOpen && !(e.target as Element | null)?.closest?.('.fs-keys')) keysOpen = false;
	}

	function onHostMove(e: MouseEvent): void {
		if (fullscreen && e.clientY <= 4) showBar();
	}

	/*
	 * The Full screen button can be dragged out of the way, since in the top
	 * right corner it sat over the remote's own window buttons (#145), and
	 * stays where it was put. Its place is kept as fractions of the room it
	 * has, so it stays in the same spot as the window is resized.
	 */
	const FS_MARGIN = 8;
	let fsButton = $state<HTMLButtonElement>();
	let fsDrag: { id: number; dx: number; dy: number; x0: number; y0: number; moved: boolean } | null = null;
	/** Set by a drag, so the click that ends it does not go full screen. */
	let fsDragged = false;
	/** The place while it is being dragged, saved when the drag ends. */
	let fsDragAt = $state<{ x: number; y: number } | null>(null);

	const clamp01 = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0);
	const fsAt = $derived.by(() => {
		const at = fsDragAt ?? getSettings().rdpFullscreenButtonAt;
		return at ? { x: clamp01(at.x), y: clamp01(at.y) } : null;
	});

	function fsDown(e: PointerEvent): void {
		if (e.button !== 0 || !fsButton) return;
		const r = fsButton.getBoundingClientRect();
		fsDrag = { id: e.pointerId, dx: e.clientX - r.left, dy: e.clientY - r.top, x0: e.clientX, y0: e.clientY, moved: false };
		fsButton.setPointerCapture(e.pointerId);
	}

	function fsMove(e: PointerEvent): void {
		if (!fsDrag || e.pointerId !== fsDrag.id || !fsButton || !screen) return;
		if (!fsDrag.moved && Math.hypot(e.clientX - fsDrag.x0, e.clientY - fsDrag.y0) < 4) return;
		fsDrag.moved = true;
		const box = screen.getBoundingClientRect();
		const b = fsButton.getBoundingClientRect();
		const roomX = box.width - 2 * FS_MARGIN - b.width;
		const roomY = box.height - 2 * FS_MARGIN - b.height;
		fsDragAt = {
			x: roomX > 0 ? clamp01((e.clientX - fsDrag.dx - box.left - FS_MARGIN) / roomX) : 0,
			y: roomY > 0 ? clamp01((e.clientY - fsDrag.dy - box.top - FS_MARGIN) / roomY) : 0
		};
	}

	function fsUp(e: PointerEvent): void {
		if (!fsDrag || e.pointerId !== fsDrag.id) return;
		fsDragged = fsDrag.moved;
		fsDrag = null;
		if (fsDragged && fsDragAt) updateSetting('rdpFullscreenButtonAt', fsDragAt);
		fsDragAt = null;
	}

	function fsClick(): void {
		if (fsDragged) {
			fsDragged = false;
			return;
		}
		void setFullscreen(true);
	}

	/* --- GPU path ---------------------------------------------------------- */

	/**
	 * WebGL when the setting allows and the webview can: each region is
	 * uploaded into one texture the size of the framebuffer, and the texture
	 * is drawn as a quad once per message. The 2D canvas path below stays as
	 * the fallback; the two never mix on one canvas, since a canvas gives out
	 * one kind of context for its lifetime.
	 */
	let gl: WebGLRenderingContext | null = null;
	let glW = 0;
	let glH = 0;

	function initGl(): void {
		if (!getSettings().rdpHardwareRendering) return;
		const g = canvas.getContext('webgl', { alpha: false, antialias: false, depth: false, stencil: false, preserveDrawingBuffer: true, premultipliedAlpha: false });
		if (!g) return;
		const compile = (type: number, src: string): WebGLShader | null => {
			const sh = g.createShader(type);
			if (!sh) return null;
			g.shaderSource(sh, src);
			g.compileShader(sh);
			return g.getShaderParameter(sh, g.COMPILE_STATUS) ? sh : null;
		};
		const vs = compile(g.VERTEX_SHADER, 'attribute vec2 p; varying vec2 t; void main() { t = vec2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5); gl_Position = vec4(p, 0.0, 1.0); }');
		const fs = compile(g.FRAGMENT_SHADER, 'precision mediump float; varying vec2 t; uniform sampler2D s; void main() { gl_FragColor = texture2D(s, t); }');
		const prog = g.createProgram();
		if (!vs || !fs || !prog) return;
		g.attachShader(prog, vs);
		g.attachShader(prog, fs);
		g.linkProgram(prog);
		if (!g.getProgramParameter(prog, g.LINK_STATUS)) return;
		g.useProgram(prog);
		const quad = g.createBuffer();
		g.bindBuffer(g.ARRAY_BUFFER, quad);
		g.bufferData(g.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), g.STATIC_DRAW);
		const p = g.getAttribLocation(prog, 'p');
		g.enableVertexAttribArray(p);
		g.vertexAttribPointer(p, 2, g.FLOAT, false, 0, 0);
		const tex = g.createTexture();
		g.bindTexture(g.TEXTURE_2D, tex);
		g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MIN_FILTER, g.NEAREST);
		g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MAG_FILTER, g.NEAREST);
		g.texParameteri(g.TEXTURE_2D, g.TEXTURE_WRAP_S, g.CLAMP_TO_EDGE);
		g.texParameteri(g.TEXTURE_2D, g.TEXTURE_WRAP_T, g.CLAMP_TO_EDGE);
		g.pixelStorei(g.UNPACK_ALIGNMENT, 1);
		gl = g;
	}

	function glSized(w: number, h: number): void {
		if (!gl) return;
		gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
		gl.viewport(0, 0, w, h);
		glW = w;
		glH = h;
	}

	/* --- frames ------------------------------------------------------------ */

	/**
	 * One message from the backend: a pointer position and any number of
	 * regions, back to back. Painted in order, then acknowledged once — the
	 * backend holds the next message until it hears this, which is what keeps
	 * a busy screen from piling up messages faster than they can be drawn.
	 */
	function paint(buf: ArrayBuffer): void {
		const view = new DataView(buf);
		let at = 0;
		let painted = false;

		while (at < buf.byteLength) {
			const kind = view.getUint8(at);

			if (kind === FRAME_POINTER) {
				// The server moved the cursor. The local pointer is not
				// warped — that is not something a webview may do — so the
				// position is noted and nothing more.
				at += FRAME_POINTER_LEN;
				continue;
			}
			if (kind === FRAME_POINTER_HIDDEN) {
				canvas.style.cursor = 'none';
				at += 1;
				continue;
			}
			if (kind === FRAME_POINTER_DEFAULT) {
				canvas.style.cursor = 'default';
				at += 1;
				continue;
			}
			if (kind === FRAME_POINTER_SHAPE) {
				if (at + FRAME_POINTER_SHAPE_HEADER > buf.byteLength) break;
				const w = view.getUint16(at + 1, true);
				const h = view.getUint16(at + 3, true);
				const hx = view.getUint16(at + 5, true);
				const hy = view.getUint16(at + 7, true);
				const bytes = w * h * 4;
				at += FRAME_POINTER_SHAPE_HEADER;
				if (w === 0 || h === 0 || at + bytes > buf.byteLength) break;
				setCursorShape(new Uint8ClampedArray(buf, at, bytes), w, h, hx, hy);
				at += bytes;
				continue;
			}
			if (kind !== FRAME_REGION && kind !== FRAME_FULL) break;
			if (at + FRAME_HEADER > buf.byteLength) break;

			const x = view.getUint16(at + 1, true);
			const y = view.getUint16(at + 3, true);
			const w = view.getUint16(at + 5, true);
			const h = view.getUint16(at + 7, true);
			const fbW = view.getUint16(at + 9, true);
			const fbH = view.getUint16(at + 11, true);
			const bytes = w * h * 4;
			at += FRAME_HEADER;
			if (w === 0 || h === 0 || at + bytes > buf.byteLength) break;

			// The framebuffer grew, shrank, or was replaced: size the canvas
			// to it. Resizing clears a canvas, so only when it has to be.
			if (canvas.width !== fbW || canvas.height !== fbH) {
				canvas.width = fbW;
				if (railView !== undefined) fbSize = { width: fbW, height: fbH };
				canvas.height = fbH;
				if (!gl) ctx = canvas.getContext('2d', { alpha: false });
			}
			if (gl) {
				if (glW !== fbW || glH !== fbH) glSized(fbW, fbH);
				gl.texSubImage2D(gl.TEXTURE_2D, 0, x, y, w, h, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(buf, at, bytes));
				painted = true;
			} else {
				if (!ctx) ctx = canvas.getContext('2d', { alpha: false });
				if (ctx) {
					const pixels = new Uint8ClampedArray(buf, at, bytes);
					ctx.putImageData(new ImageData(pixels, w, h), x, y);
				}
			}
			at += bytes;
		}

		if (gl && painted) gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
		void remote.ack().catch(() => {});
	}

	/**
	 * Show the server's cursor as the canvas's CSS cursor. Straight RGBA in,
	 * one small canvas to turn it into a PNG data URL, and the hotspot goes
	 * along so the click lands where the arrow's tip is. The mouse then moves
	 * at the speed of the local mouse, whatever the picture is doing.
	 */
	let cursorCanvas: HTMLCanvasElement | null = null;

	function setCursorShape(rgba: Uint8ClampedArray, w: number, h: number, hx: number, hy: number): void {
		cursorCanvas ??= document.createElement('canvas');
		cursorCanvas.width = w;
		cursorCanvas.height = h;
		const cctx = cursorCanvas.getContext('2d');
		if (!cctx) return;
		// A view into the frame message; ImageData wants its own buffer.
		cctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
		const url = cursorCanvas.toDataURL('image/png');
		// Hotspots are clamped by the engine to the image; `auto` is the
		// fallback for an image the engine will not use (over 128px).
		canvas.style.cursor = `url(${url}) ${Math.min(hx, w - 1)} ${Math.min(hy, h - 1)}, auto`;
	}

	/* --- pointer ----------------------------------------------------------- */

	/** Where on the remote desktop a pointer event landed. */
	function remotePoint(e: MouseEvent): { x: number; y: number } | null {
		const r = canvas.getBoundingClientRect();
		if (r.width === 0 || r.height === 0) return null;
		const x = Math.round(((e.clientX - r.left) / r.width) * canvas.width);
		const y = Math.round(((e.clientY - r.top) / r.height) * canvas.height);
		return {
			x: Math.max(0, Math.min(canvas.width - 1, x)),
			y: Math.max(0, Math.min(canvas.height - 1, y)),
		};
	}

	// Moves are coalesced to one per frame: a fast mouse produces hundreds a
	// second and the server only needs the newest.
	let pendingMove: { x: number; y: number } | null = null;
	let moveScheduled = false;

	function onMove(e: MouseEvent): void {
		if (phase !== 'connected') return;
		const p = remotePoint(e);
		if (!p) return;
		pendingMove = p;
		if (moveScheduled) return;
		moveScheduled = true;
		requestAnimationFrame(() => {
			moveScheduled = false;
			if (!pendingMove || phase !== 'connected') return;
			const { x, y } = pendingMove;
			pendingMove = null;
			sendInput(() => remote.mouse(x, y, 'move'));
		});
	}

	function onButton(e: MouseEvent, action: 'down' | 'up'): void {
		if (phase !== 'connected') return;
		if (e.button > 2) return;
		const p = remotePoint(e);
		if (!p) return;
		if (action === 'down') canvas.focus();
		// A window of its own shows its part of one shared picture: where
		// another remote window lies over it there, the click would land on
		// that one. Bring this one to the front there first.
		if (action === 'down') activateRemote();
		e.preventDefault();
		sendInput(() => remote.mouse(p.x, p.y, action, e.button));
	}

	function onWheel(e: WheelEvent): void {
		if (phase !== 'connected') return;
		const p = remotePoint(e);
		if (!p || e.deltaY === 0) return;
		e.preventDefault();
		// A browser notch is deltaY ≈ +100 downwards; an RDP notch is 120 and
		// positive means towards the user. One notch per event keeps trackpads
		// from firing a flood of tiny steps.
		sendInput(() => remote.mouse(p.x, p.y, 'wheel', 0, e.deltaY < 0 ? 120 : -120));
	}

	/* --- phone ------------------------------------------------------------- */

	/**
	 * A phone has no keyboard to send keys from, and Android never opens its
	 * on-screen keyboard for a canvas. What it types lands in a hidden text
	 * field instead, and is sent on as it arrives. RDP never says where the
	 * focus is on the remote, so the keyboard is opened by a button, as in
	 * Microsoft's own mobile client.
	 */
	const onPhone = isMobile();
	/** The part of the panel the desktop is drawn in; the key bar sits under it. */
	let screen: HTMLDivElement | undefined = $state();
	let typeEl: HTMLTextAreaElement | undefined = $state();
	let barCtrl = $state(false);
	let barAlt = $state(false);
	/** What the keyboard's current composition has already sent. Gboard and
	 *  most keyboards compose a word as it is typed; the difference is sent
	 *  each time it changes, so letters appear on the remote as they are typed. */
	let composed = '';

	async function pressTap(code: string): Promise<void> {
		await pressCode(code, true);
		await pressCode(code, false);
	}

	function tapKey(code: string): void {
		sendInput(() => pressTap(code));
	}

	/** Hold Ctrl, Alt and Win as the sticky bar keys say, around `send`. */
	async function withModifiers(send: () => Promise<void>, win = false): Promise<void> {
		const mods = [barCtrl && 'ControlLeft', barAlt && 'AltLeft', win && 'MetaLeft'].filter(Boolean) as string[];
		barCtrl = false;
		barAlt = false;
		for (const m of mods) await pressCode(m, true);
		await send();
		for (const m of mods.reverse()) await pressCode(m, false);
	}

	/** The physical key for a letter or digit, so Ctrl+C is a real Ctrl+C. */
	function codeFor(ch: string): string | null {
		if (/^[a-z]$/i.test(ch)) return `Key${ch.toUpperCase()}`;
		if (/^[0-9]$/.test(ch)) return `Digit${ch}`;
		return null;
	}

	function sendText(text: string): void {
		for (const ch of text) {
			if (ch === '\n') {
				tapKey('Enter');
				continue;
			}
			const code = codeFor(ch);
			if ((barCtrl || barAlt) && code) {
				sendInput(() => withModifiers(() => pressTap(code)));
				continue;
			}
			sendInput(() => typeChar(ch.codePointAt(0) ?? 0));
		}
	}

	/** Send the change from `before` to `after`: backspaces, then new text. */
	function sendChange(before: string, after: string): void {
		let same = 0;
		while (same < before.length && same < after.length && before[same] === after[same]) same++;
		for (let i = same; i < before.length; i++) tapKey('Backspace');
		sendText(after.slice(same));
	}

	function onSinkBeforeInput(e: InputEvent): void {
		if (phase !== 'connected' || e.isComposing) return;
		if (e.inputType === 'deleteContentBackward') {
			e.preventDefault();
			tapKey('Backspace');
		} else if (e.inputType === 'insertLineBreak' || e.inputType === 'insertParagraph') {
			e.preventDefault();
			tapKey('Enter');
		} else if (e.inputType === 'insertText' && e.data) {
			e.preventDefault();
			sendText(e.data);
		}
	}

	function onSinkComposition(e: CompositionEvent): void {
		if (phase !== 'connected') return;
		if (e.type === 'compositionstart') {
			composed = '';
			return;
		}
		sendChange(composed, e.data ?? '');
		composed = e.data ?? '';
		if (e.type === 'compositionend') {
			composed = '';
			if (typeEl) typeEl.value = '';
		}
	}

	/** A hardware keyboard plugged into the phone still sends real keys: those
	 *  go the desktop's way. The on-screen keyboard's keys (code 229) do not. */
	function onSinkKeyDown(e: KeyboardEvent): void {
		if (e.keyCode !== 229 && isKey(e)) onKeyDown(e);
	}

	function onSinkKeyUp(e: KeyboardEvent): void {
		if (e.keyCode !== 229 && isKey(e)) onKeyUp(e);
	}

	function toggleKeyboard(): void {
		if (!typeEl) return;
		if (document.activeElement === typeEl) typeEl.blur();
		else typeEl.focus({ preventScroll: true });
	}

	function onBarKey(key: BarKey): void {
		if (phase !== 'connected') return;
		if (typeof key === 'object' && 'fn' in key) {
			sendInput(() => withModifiers(() => pressTap(`F${key.fn}`)));
			return;
		}
		if (typeof key === 'object') {
			sendText(key.char);
			return;
		}
		if (key === 'ctrl-alt-del') {
			barCtrl = true;
			barAlt = true;
			sendInput(() => withModifiers(() => pressTap('Delete')));
			return;
		}
		if (key === 'win') {
			sendInput(() => withModifiers(() => pressTap('MetaLeft')));
			return;
		}
		// Text size is the terminal's; this bar does not offer it.
		if (key === 'smaller' || key === 'larger') return;
		const code = {
			esc: 'Escape', tab: 'Tab', up: 'ArrowUp', down: 'ArrowDown', left: 'ArrowLeft', right: 'ArrowRight',
			home: 'Home', end: 'End', pgup: 'PageUp', pgdn: 'PageDown', del: 'Delete'
		}[key];
		if (code) sendInput(() => withModifiers(() => pressTap(code)));
	}

	/** A long press is the right button, as on every touch screen. */
	function onLongPress(e: MouseEvent): void {
		e.preventDefault();
		if (!onPhone || phase !== 'connected') return;
		const p = remotePoint(e);
		if (!p) return;
		sendInput(() => remote.mouse(p.x, p.y, 'down', 2).then(() => remote.mouse(p.x, p.y, 'up', 2)));
	}

	/* --- keyboard ---------------------------------------------------------- */

	/**
	 * KeyboardEvent.code → PC/AT set-1 scancode. `[code, extended]`.
	 *
	 * The extended (E0-prefixed) keys are the ones that were added after the
	 * original 83-key layout and reuse a scancode from it: right Ctrl shares
	 * 0x1D with left Ctrl, the arrow cluster shares codes with the numpad.
	 */
	const SCANCODES: Record<string, [number, boolean]> = {
		Escape: [0x01, false],
		Digit1: [0x02, false], Digit2: [0x03, false], Digit3: [0x04, false], Digit4: [0x05, false],
		Digit5: [0x06, false], Digit6: [0x07, false], Digit7: [0x08, false], Digit8: [0x09, false],
		Digit9: [0x0a, false], Digit0: [0x0b, false], Minus: [0x0c, false], Equal: [0x0d, false],
		Backspace: [0x0e, false], Tab: [0x0f, false],
		KeyQ: [0x10, false], KeyW: [0x11, false], KeyE: [0x12, false], KeyR: [0x13, false],
		KeyT: [0x14, false], KeyY: [0x15, false], KeyU: [0x16, false], KeyI: [0x17, false],
		KeyO: [0x18, false], KeyP: [0x19, false], BracketLeft: [0x1a, false], BracketRight: [0x1b, false],
		Enter: [0x1c, false], ControlLeft: [0x1d, false],
		KeyA: [0x1e, false], KeyS: [0x1f, false], KeyD: [0x20, false], KeyF: [0x21, false],
		KeyG: [0x22, false], KeyH: [0x23, false], KeyJ: [0x24, false], KeyK: [0x25, false],
		KeyL: [0x26, false], Semicolon: [0x27, false], Quote: [0x28, false], Backquote: [0x29, false],
		ShiftLeft: [0x2a, false], Backslash: [0x2b, false],
		KeyZ: [0x2c, false], KeyX: [0x2d, false], KeyC: [0x2e, false], KeyV: [0x2f, false],
		KeyB: [0x30, false], KeyN: [0x31, false], KeyM: [0x32, false], Comma: [0x33, false],
		Period: [0x34, false], Slash: [0x35, false], ShiftRight: [0x36, false],
		NumpadMultiply: [0x37, false], AltLeft: [0x38, false], Space: [0x39, false], CapsLock: [0x3a, false],
		F1: [0x3b, false], F2: [0x3c, false], F3: [0x3d, false], F4: [0x3e, false], F5: [0x3f, false],
		F6: [0x40, false], F7: [0x41, false], F8: [0x42, false], F9: [0x43, false], F10: [0x44, false],
		NumLock: [0x45, false], ScrollLock: [0x46, false],
		Numpad7: [0x47, false], Numpad8: [0x48, false], Numpad9: [0x49, false], NumpadSubtract: [0x4a, false],
		Numpad4: [0x4b, false], Numpad5: [0x4c, false], Numpad6: [0x4d, false], NumpadAdd: [0x4e, false],
		Numpad1: [0x4f, false], Numpad2: [0x50, false], Numpad3: [0x51, false],
		Numpad0: [0x52, false], NumpadDecimal: [0x53, false], IntlBackslash: [0x56, false],
		F11: [0x57, false], F12: [0x58, false],
		// Extended.
		NumpadEnter: [0x1c, true], ControlRight: [0x1d, true], NumpadDivide: [0x35, true],
		PrintScreen: [0x37, true], AltRight: [0x38, true],
		Home: [0x47, true], ArrowUp: [0x48, true], PageUp: [0x49, true],
		ArrowLeft: [0x4b, true], ArrowRight: [0x4d, true],
		End: [0x4f, true], ArrowDown: [0x50, true], PageDown: [0x51, true],
		Insert: [0x52, true], Delete: [0x53, true],
		MetaLeft: [0x5b, true], MetaRight: [0x5c, true], ContextMenu: [0x5d, true],
	};

	/** Keys currently down, so a lost focus can release them. A modifier stuck
	 *  down on the remote is the classic remote-desktop misery. */
	const held = new Set<string>();

	/** VNC: the keysym each held key went down as, so it comes up as the same
	 *  one even if Shift was let go in between. */
	const heldKeysyms = new Map<string, number>();
	/** When Left Ctrl last went down. On Windows AltGr arrives as Left Ctrl
	 *  then Right Alt at the same instant; that Ctrl is not the user's. */
	let ctrlDownAt = 0;

	/** RDP: the lock state last sent to the remote. */
	let sentLocks: Locks | null = null;
	const heldLocks = new Set<string>();
	let lockKeyInput: Promise<void> | null = null;
	let lockSyncTimer: ReturnType<typeof setTimeout> | undefined;

	/**
	 * RDP: make the remote's CapsLock, NumLock and ScrollLock the local ones,
	 * as mstsc and RDM do. A new remote session starts with its own lock
	 * state, so this runs on connect, on focus, and whenever the local state
	 * changes, in every connected session, so all of them follow the keyboard.
	 * When forwarding lock keys, wait for their releases before syncing so
	 * the press cannot toggle the remote back after the sync.
	 */
	function sendLocks(force = false): void {
		if (protocol !== 'rdp' || phase !== 'connected' || heldLocks.size || lockKeyInput) return;
		const locks = localLocks();
		if (!locks) return;
		if (!force && sentLocks && sentLocks.caps === locks.caps && sentLocks.num === locks.num && sentLocks.scroll === locks.scroll) return;
		sentLocks = locks;
		sendInput(() => rdpSyncLocks(id, locks.caps, locks.num, locks.scroll));
	}
	const unlistenLocks = onLocksChange(() => {
		if (!forwardLockKeys) {
			sendLocks();
			return;
		}
		// The window learns about the change during capture. Let the focused
		// desktop handle the key before deciding whether to sync.
		if (lockSyncTimer !== undefined) return;
		lockSyncTimer = setTimeout(() => {
			lockSyncTimer = undefined;
			sendLocks();
		}, 0);
	});

	function sendLockKey(input: () => Promise<void>): void {
		// In order with the other keys, like all input.
		const pending = sendInput(input);
		lockKeyInput = pending;
		void pending.then(() => {
			if (lockKeyInput !== pending) return;
			lockKeyInput = null;
			sendLocks(true);
		});
	}

	function onLockKey(e: KeyboardEvent): void {
		e.preventDefault();
		if (!forwardLockKeys || e.repeat) return;
		const code = e.code;
		if (isMac()) {
			// Caps Lock gives one event per press: down when enabled, up when
			// disabled. The keypad's Clear key arrives with the NumLock code.
			if (code === 'NumLock') return;
			sendLockKey(async () => {
				await pressCode(code, true);
				await pressCode(code, false);
			});
		} else if (e.type === 'keydown') {
			heldLocks.add(code);
			sendLockKey(() => pressCode(code, true));
		} else if (heldLocks.delete(code)) {
			sendLockKey(() => pressCode(code, false));
		}
	}

	/** Whether a key event is one this desktop takes. */
	function isKey(e: KeyboardEvent): boolean {
		return isVnc() ? keysymForEvent(e) !== null || e.key === 'AltGraph' : !!SCANCODES[e.code];
	}

	function onVncKeyDown(e: KeyboardEvent): void {
		if (e.key === 'AltGraph') {
			// What AltGr types arrives as its own character; the remote needs
			// no modifier for it, and above all not the Ctrl Windows invented.
			e.preventDefault();
			const fake = heldKeysyms.get('ControlLeft');
			if (fake !== undefined && e.timeStamp - ctrlDownAt < 50) {
				heldKeysyms.delete('ControlLeft');
				sendInput(() => vncKey(id, fake, false));
			}
			return;
		}
		const keysym = keysymForEvent(e);
		if (keysym === null) return;
		e.preventDefault();
		if (e.code === 'ControlLeft') ctrlDownAt = e.timeStamp;
		heldKeysyms.set(e.code, keysym);
		sendInput(() => vncKey(id, keysym, true));
	}

	function onVncKeyUp(e: KeyboardEvent): void {
		const keysym = heldKeysyms.get(e.code);
		if (keysym === undefined) return;
		e.preventDefault();
		heldKeysyms.delete(e.code);
		sendInput(() => vncKey(id, keysym, false));
	}

	function onKeyDown(e: KeyboardEvent): void {
		if (e.ctrlKey && e.altKey && e.code === 'Enter') {
			e.preventDefault();
			void setFullscreen(!fullscreen);
			return;
		}
		if (phase !== 'connected') return;
		if (isVnc()) {
			// The VNC server repeats a held key itself; forwarding the
			// browser's repeats as well would double the rate.
			if (e.repeat) return;
			onVncKeyDown(e);
			return;
		}
		// SPICE is left as it was: its guest repeats a held key itself.
		if (isSpice() && e.repeat) return;
		if (!isSpice() && isLockKey(e.code)) {
			onLockKey(e);
			return;
		}
		// An RDP server does not repeat a held key: the client sends the key
		// down again for each repeat, as mstsc does, and the server counts each
		// one.

		const entry = SCANCODES[e.code];
		if (entry) {
			e.preventDefault();
			held.add(e.code);
			sendInput(() => remote.key(entry[0], entry[1], false));
			return;
		}
		// No position for this key here (a dead-key result, an IME commit):
		// send what it produced.
		if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
			e.preventDefault();
			sendInput(() => typeChar(e.key.charCodeAt(0)));
		}
	}

	function onKeyUp(e: KeyboardEvent): void {
		if (phase !== 'connected') return;
		if (isVnc()) {
			onVncKeyUp(e);
			return;
		}
		if (!isSpice() && isLockKey(e.code)) {
			onLockKey(e);
			return;
		}
		const entry = SCANCODES[e.code];
		if (!entry) return;
		e.preventDefault();
		held.delete(e.code);
		sendInput(() => remote.key(entry[0], entry[1], true));
	}

	function releaseAll(): void {
		for (const code of held) {
			const entry = SCANCODES[code];
			if (entry) sendInput(() => remote.key(entry[0], entry[1], true));
		}
		held.clear();
		for (const code of heldLocks) sendLockKey(() => pressCode(code, false));
		heldLocks.clear();
		for (const keysym of heldKeysyms.values()) sendInput(() => vncKey(id, keysym, false));
		heldKeysyms.clear();
	}

	/* --- lifecycle --------------------------------------------------------- */

	/**
	 * The largest even size the panel can show, in device pixels, for the
	 * server to render at. Device pixels rather than CSS pixels: on a scaled
	 * display the panel is smaller in CSS pixels than on the screen, and a
	 * remote rendered at that size is stretched to fit. Rendered at the
	 * screen's own resolution and scaled down by CSS, it is pixel for pixel.
	 */
	function fitSize(): { width: number; height: number } {
		// Seamless: the remote desktop lies under this screen, pixel for pixel.
		if (seamless && monitor) {
			const even = (v: number) => Math.min(8192, Math.max(200, v & ~1));
			return { width: even(monitor.size.width), height: even(monitor.size.height) };
		}
		// A session set to a fixed size keeps it; the canvas scales it to fit.
		const fixed = protocol === 'rdp' ? parseResolution((params as RdpConnectParams).options?.resolution) : null;
		if (fixed) return fixed;
		const r = (screen ?? host).getBoundingClientRect();
		const scale = window.devicePixelRatio || 1;
		const even = (v: number) => Math.min(8192, Math.max(200, Math.floor(v * scale) & ~1));
		return { width: even(r.width || 1024), height: even(r.height || 768) };
	}

	/**
	 * The remote's display scaling: the session's own, or on desktops the
	 * screen's, to the nearest quarter. A desktop rendered at device pixels on a 200%
	 * screen is otherwise drawn at 100% and its text is half the size.
	 */
	function rdpScale(): number {
		const own = (params as RdpConnectParams).options?.scale;
		if (own) return own;
		// Phones and tablets: a desktop at their screen's 300% would leave
		// next to no room on it; they keep 100% unless the session says.
		if (isMobile()) return 100;
		const ratio = (seamless && monitor?.scaleFactor) || window.devicePixelRatio || 1;
		return Math.min(500, Math.max(100, Math.round(ratio * 4) * 25));
	}

	/** Whether the current RDP attempt asked for the graphics pipeline, and at what size. */
	let graphicsPipeline = false;
	let connectSize = { width: 0, height: 0 };

	/**
	 * GNOME Remote Desktop serves only clients that offer the graphics
	 * pipeline. Without it the server hangs up during the capabilities
	 * exchange with a bare Disconnect Provider Ultimatum, which reaches us as
	 * a decode error before the session ever connected. When that happens on
	 * an attempt made without the pipeline, try once more with it; Windows
	 * and xrdp never get here, so the setting keeps its meaning for them.
	 * Returns true when a retry was started.
	 */
	function retryWithGraphicsPipeline(error: string): boolean {
		if (isVnc() || isSpice() || graphicsPipeline || phase !== 'connecting') return false;
		if (!/disconnect provider ultimatum/i.test(error)) return false;
		graphicsPipeline = true;
		rdpConnect({ ...(params as RdpConnectParams), ...connectSize, scale: rdpScale(), graphicsPipeline: true, olderGraphicsFormats: !getSettings().rdpNewGraphicsFormats }, paint).catch((err) => {
			phase = 'error';
			message = String(err);
		});
		return true;
	}

	// A screenshot of the remote desktop: the framebuffer at its own size.
	// The WebGL context keeps its drawing buffer, so the canvas reads back.
	let unregisterCapture: (() => void) | undefined;
	let unregisterKeys: (() => void) | undefined;

	/**
	 * A view: no connection of its own. It paints the frames the session's
	 * tab receives and sends input to the same session, and goes when its
	 * remote window does.
	 */
	/** A view: put its remote window in front there, unless it already is. */
	function activateRemote(): void {
		// Menus and tooltips are left alone: activating one there closes it.
		if (railView === undefined || !viewRect || viewRect.active || !viewRect.listed) return;
		sendInput(() => rdpRailCommand(id, railView, 'activate'));
	}

	async function mountView(): Promise<void> {
		initGl();
		// Focused here (clicked, Alt+Tab): focused there too, so keys go to it.
		unlistenFocus = await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
			if (focused) activateRemote();
		});
		const closeSelf = () => void getCurrentWindow().close().catch(() => {});
		unlistenWindows = await listen<RdpRailWindow[]>(`rdp-windows-${id}`, (event) => (railWindows = event.payload));
		unlisten = await listen<RdpStatus>(`rdp-status-${id}`, (event) => {
			if (event.payload.state === 'closed' || event.payload.state === 'error') closeSelf();
		});
		railWindows = await rdpRailWindows(id).catch(() => []);
		try {
			await rdpViewAttach(id, viewKey, paint);
		} catch {
			closeSelf();
			return;
		}
		phase = 'connected';
		sendLocks();
		canvas?.focus();
	}

	onMount(async () => {
		if (railView !== undefined) {
			await mountView();
			return;
		}
		unregisterCapture = registerCapture(id, () => (phase === 'connected' && canvas?.width ? canvas : null));
		// A seamless window's view shares the session's tab; the desktop pane registers.
		if (railView === undefined) {
			unregisterKeys = registerSendKeys(id, { ready: () => phase === 'connected', send: (codes) => sendInput(() => sendCombo(codes)) });
		}
		if (seamless) monitor = await currentMonitor().catch(() => null);
		const size = fitSize();

		unlisten = await listen<RdpStatus>(`${protocol}-status-${id}`, (event) => {
			const s = event.payload;
			if (s.state === 'connected' || s.state === 'loggedIn') {
				phase = 'connected';
				message = '';
				sentLocks = null;
				sendLocks();
				resizeNotBefore = Date.now() + SETTLE_MS;
			} else if (s.state === 'closed') {
				// Logged off (or disconnected from Windows' menu) after the
				// desktop was up: the tab goes, if Settings say so.
				if (s.clean && phase === 'connected' && getSettings().closeTabOnExit) {
					closeTab(id);
					return;
				}
				phase = 'closed';
				ended = s.clean;
				// A logoff is not an error: no server text to puzzle over.
				message = s.clean ? '' : s.reason;
				// Nothing to release on a desktop that has gone; just forget.
				held.clear();
				heldLocks.clear();
				heldKeysyms.clear();
			} else if (s.state === 'error') {
				if (retryWithGraphicsPipeline(s.message)) return;
				phase = 'error';
				message = s.message;
				askBmcVncPassword(s.message);
			}
		});

		if (remoteApp) {
			unlistenWindows = await listen<RdpRailWindow[]>(`rdp-windows-${id}`, (event) => {
				railWindows = event.payload;
				placeWindows(event.payload);
			});
			// A panel re-attached to a running RemoteApp: the row as it stands.
			railWindows = await rdpRailWindows(id).catch(() => []);
			placeWindows(railWindows);
		}

		if (!isVnc() && clipMode !== 'off') {
			unlistenClip = await listen<RdpClipEvent>(`rdp-clipboard-${id}`, (event) => onClipEvent(event.payload));
			if (!onPhone) await listenForDrops().catch(() => {});
		}

		try {
			initGl();
			if (isVnc()) {
				// A VNC desktop comes at the size its server has; the panel
				// asks for its own size once connected, which some servers allow.
				await vncConnect(params as VncConnectParams, paint);
			} else if (isSpice()) {
				// A virtual machine comes at the size its guest has; the
				// panel asks for its own once connected, which the guest's
				// SPICE agent does.
				await spiceConnect(params as SpiceConnectParams, paint);
			} else {
				graphicsPipeline = (params as RdpConnectParams).graphicsPipeline || getSettings().rdpGraphicsPipeline;
				connectSize = size;
				await rdpConnect({ ...(params as RdpConnectParams), ...size, scale: rdpScale(), graphicsPipeline, olderGraphicsFormats: !getSettings().rdpNewGraphicsFormats }, paint);
			}
		} catch (err) {
			phase = 'error';
			// Unapproved password manager entries are asked about from the
			// session list, which this panel is not.
			message = untrustedRefsFromError(err) ? t('pwmanager.untrusted_reopen') : String(err);
			return;
		}

		// Ask the server to follow the panel when it is resized. Debounced:
		// a drag fires this continuously and each one is a renegotiation.
		sizeObserver = new ResizeObserver(() => {
			if (resizeTimer) clearTimeout(resizeTimer);
			resizeTimer = setTimeout(() => {
				if (Date.now() < resizeNotBefore) {
					resizeTimer = setTimeout(() => sizeObserver && host && requestResize(), resizeNotBefore - Date.now());
					return;
				}
				requestResize();
			}, 300);
		});
		sizeObserver.observe(screen ?? host);
	});

	/** A server management session's VNC server refused the saved (or
	 *  typed) password: the session list asks for another one, and its tab
	 *  replaces this one. */
	function askBmcVncPassword(error: string): void {
		const bmcSessionId = isVnc() ? (params as VncConnectParams).bmcSessionId : undefined;
		if (!bmcSessionId || !/VNC password/.test(error)) return;
		requestSessionAction({ kind: 'bmc-vnc', sessionId: bmcSessionId, failedTabId: id });
	}

	/** Send the panel's current size, unless the desktop already has it. */
	function requestResize(): void {
		{
			if (phase !== 'connected') return;
			// A view follows the tab's size; a seamless desktop keeps the screen's.
			if (railView !== undefined || seamless) return;
			// A VNC desktop is usually shared and already laid out: asked to
			// take a phone's shape, the server cut its windows off at the new
			// edge, for everyone watching. It is scaled to fit here instead and
			// never resized, as noVNC does by default. RDP still resizes: each
			// RDP login is a session of its own.
			if (isVnc()) return;
			// A tab switched away from (or a page left) is display: none and
			// measures 0 x 0. That is not a size to render at: sent on, the
			// server shrank to the fallback size and grew back on return,
			// moving every window on the remote desktop. Keep the last size.
			const r = (screen ?? host).getBoundingClientRect();
			if (r.width < 1 || r.height < 1) return;
			const s = fitSize();
			// The size the desktop already has is not a resize. Asking for
			// it anyway leaves a request the server never answers.
			if (s.width === canvas.width && s.height === canvas.height) return;
			sendInput(() => remote.resize(s.width, s.height));
		}
	}

	onDestroy(() => {
		if (railView !== undefined) void rdpViewDetach(id, viewKey).catch(() => {});
		unregisterCapture?.();
		unregisterKeys?.();
		unlisten?.();
		unlistenLocks();
		clearTimeout(lockSyncTimer);
		unlistenClip?.();
		unlistenWindows?.();
		unlistenFocus?.();
		unlistenDrop?.();
		sizeObserver?.disconnect();
		if (resizeTimer) clearTimeout(resizeTimer);
		releaseAll();
		if (fullscreen) {
			lift(false);
			void rdpWindowFullscreen(false).catch(() => {});
		}
		if (barTimer) clearTimeout(barTimer);
		// Not a disconnect: the panel is re-created whenever the page it lives
		// on is left and returned to, and the session must outlive that. The
		// tab closing is what ends the session; see closeTab.
	});

	$effect(() => {
		if (active) canvas?.focus();
		// A tab-switching key (Ctrl+Tab, Ctrl+1) leaves this tab with Ctrl
		// still down here: the remote must not keep it held.
		else releaseAll();
	});
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="rdp" class:fullscreen bind:this={host} onmousemove={onHostMove} onpointerdowncapture={onHostDown}>
	<!-- The desktop's area: sized and measured on its own, so a phone's key
	     bar below it is never drawn over the remote (its taskbar included). -->
	{#if remoteApp && railView === undefined && phase === 'connected' && railWindows.length}
		<div class="rail-row" role="toolbar" aria-label={t('rdp.remote_app_windows')}>
			{#each railWindows as w (w.id)}
				<div class="rail-win" class:minimized={w.state === 'minimized'}>
					<button type="button" class="rail-title" title={w.title} onclick={() => railClick(w)}>{w.title || t('rdp.remote_app_untitled')}</button>
					<button type="button" class="rail-close" title={t('rdp.remote_app_close')} aria-label={t('rdp.remote_app_close')} onclick={() => railClose(w)}>×</button>
				</div>
			{/each}
		</div>
	{/if}
	<div class="screen" class:view={railView !== undefined} bind:this={screen}>
	<!-- The canvas is the desktop: it takes focus and every key. The wrapper
	     only sizes and centres it. -->
	<canvas
		bind:this={canvas}
		width="1024"
		height="768"
		tabindex="0"
		aria-label={t('settings.rdp_section')}
		onmousemove={onMove}
		onmousedown={(e) => onButton(e, 'down')}
		onmouseup={(e) => onButton(e, 'up')}
		onwheel={onWheel}
		onkeydown={onKeyDown}
		onkeyup={onKeyUp}
		onblur={releaseAll}
		onfocus={() => {
			sendLocks(true);
			if (phase === 'connected') void remote.clipboardSync().catch(() => {});
		}}
		oncontextmenu={onLongPress}
		class:dim={phase !== 'connected'}
		class:cut={railView !== undefined}
		style:width={railView !== undefined ? `${fbSize.width / dpr}px` : undefined}
		style:height={railView !== undefined ? `${fbSize.height / dpr}px` : undefined}
		style:left={viewRect ? `${-viewRect.x / dpr}px` : undefined}
		style:top={viewRect ? `${-viewRect.y / dpr}px` : undefined}
	></canvas>

	{#if seamless && phase === 'connected'}
		<div class="overlay seamless-note">
			<span>{t('rdp.remote_app_separate')}</span>
			<small>{t('rdp.remote_app_separate_hint')}</small>
		</div>
	{/if}

	{#if onPhone}
		<textarea
			bind:this={typeEl}
			class="type-sink"
			aria-hidden="true"
			tabindex="-1"
			autocomplete="off"
			autocapitalize="off"
			spellcheck="false"
			onbeforeinput={onSinkBeforeInput}
			oncompositionstart={onSinkComposition}
			oncompositionupdate={onSinkComposition}
			oncompositionend={onSinkComposition}
			onkeydown={onSinkKeyDown}
			onkeyup={onSinkKeyUp}
			onblur={releaseAll}
		></textarea>
	{/if}

	{#if railView === undefined && !seamless && phase === 'connected' && !fullscreen && !onPhone && getSettings().rdpFullscreenButton}
		<button
			type="button"
			class="fs-button"
			class:placed={fsAt}
			class:dragging={fsDragAt}
			style:left={fsAt ? `calc(${FS_MARGIN}px + (100% - ${2 * FS_MARGIN}px) * ${fsAt.x})` : undefined}
			style:top={fsAt ? `calc(${FS_MARGIN}px + (100% - ${2 * FS_MARGIN}px) * ${fsAt.y})` : undefined}
			style:transform={fsAt ? `translate(${-fsAt.x * 100}%, ${-fsAt.y * 100}%)` : undefined}
			title="{t('rdp.fullscreen_hint')}. {t('rdp.fullscreen_drag')}"
			bind:this={fsButton}
			onpointerdown={fsDown}
			onpointermove={fsMove}
			onpointerup={fsUp}
			onpointercancel={fsUp}
			onclick={fsClick}
		>
			<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path fill="currentColor" d="M2 6V2h4v1.5H3.5V6zm8-4h4v4h-1.5V3.5H10zM2 10h1.5v2.5H6V14H2zm10.5 0H14v4h-4v-1.5h2.5z"/></svg>
			{t('rdp.fullscreen')}
		</button>
	{/if}

	{#if fullscreen && !onPhone}
		<div class="fs-bar" class:shown={barShown || barPinned || keysOpen}>
			<button
				type="button"
				class="fs-icon"
				class:on={barPinned}
				aria-pressed={barPinned}
				title={barPinned ? t('rdp.bar_unpin') : t('rdp.bar_pin')}
				aria-label={barPinned ? t('rdp.bar_unpin') : t('rdp.bar_pin')}
				onclick={togglePin}
			>
				<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
					<path d="M12 17v5" /><path d="M9 4h6l-1 6 3 3v2H7v-2l3-3-1-6z" />
				</svg>
			</button>
			<span class="fs-host">{label}</span>
			<div class="fs-keys">
				<button type="button" class="fs-btn" aria-haspopup="menu" aria-expanded={keysOpen} disabled={phase !== 'connected'} onclick={() => (keysOpen = !keysOpen)}>
					{t('rdp.send_keys')} ▾
				</button>
				{#if keysOpen}
					<div class="fs-menu" role="menu">
						{#each KEY_COMBOS as combo (combo.id)}
							<button type="button" role="menuitem" onclick={() => pickCombo(combo)}>{combo.label}</button>
						{/each}
					</div>
				{/if}
			</div>
			<button type="button" class="fs-btn" title={t('rdp.minimize')} onclick={() => void minimizeWindow()}>{t('rdp.minimize')}</button>
			<button type="button" class="fs-exit" onclick={() => setFullscreen(false)}>{t('rdp.exit_fullscreen')}</button>
		</div>
	{/if}

	{#if dropping}
		<div class="drop-zone" aria-hidden="true">
			<span>{clipMode === 'all' ? t('rdp.drop_hint') : t('rdp.drop_files_off')}</span>
		</div>
	{/if}

	{#if railView === undefined && phase === 'connected' && (transfer || heldCopy || toSave)}
		<div class="clip-bar" role="status">
			{#if transfer}
				<span class="clip-text">
					{transfer.incoming ? t('rdp.clip_receiving') : t('rdp.clip_sending')}
					{formatBytes(transfer.done)} / {formatBytes(transfer.total)}
				</span>
				<progress max={Math.max(1, transfer.total)} value={transfer.done}></progress>
				<button type="button" class="clip-btn" onclick={cancelTransfer}>{t('rdp.clip_cancel')}</button>
			{:else if heldCopy}
				<span class="clip-text">{t('rdp.clip_held', { count: heldCopy.files, size: formatBytes(heldCopy.total) })}</span>
				<button type="button" class="clip-btn primary" onclick={downloadHeld}>{t('rdp.clip_download')}</button>
				<button type="button" class="clip-btn" onclick={() => (heldCopy = null)}>{t('rdp.clip_ignore')}</button>
			{:else if toSave}
				<span class="clip-text">{t('rdp.save_prompt', { count: toSave })}</span>
				<button type="button" class="clip-btn primary" onclick={saveReceived}>{t('rdp.save')}</button>
				<button type="button" class="clip-btn" onclick={() => (toSave = null)}>{t('rdp.clip_ignore')}</button>
			{/if}
		</div>
	{/if}

	{#if railView === undefined && phase !== 'connected'}
		<div class="overlay" class:failed={phase === 'error'}>
			{#if phase === 'connecting'}
				<span class="spinner" aria-hidden="true"></span>
				<span>{t('rdp.connecting')}</span>
			{:else if phase === 'closed'}
				<span>{ended ? t('terminal.session_ended') : t('rdp.closed')}</span>
				{#if message}<small>{message}</small>{/if}
			{:else if untrustedCert && !certDeclined && (params as VncConnectParams).sessionId}
				<div class="cert-ask">
					<span>{t('ftp.cert_untrusted')}</span>
					<small>{untrustedCert}</small>
					<span class="cert-hint">{t('ftp.cert_hint')}</span>
					<div class="cert-actions">
						<button type="button" class="clip-btn" onclick={() => (certDeclined = true)}>{t('common.cancel')}</button>
						<button type="button" class="clip-btn primary" onclick={() => void trustCert(untrustedCert!)}>{t('ftp.cert_trust')}</button>
					</div>
				</div>
			{:else}
				<span>{t('rdp.error')}</span>
				{#if certDeclined || untrustedCert}
					<small>{t('ftp.cert_refused')}</small>
				{:else if message}<small>{message}</small>{/if}
			{/if}
		</div>
	{/if}
	</div>

	{#if onPhone && phase === 'connected'}
		<MobileKeyBar desktop bind:ctrl={barCtrl} bind:alt={barAlt} onkey={onBarKey}>
			{#snippet leading()}
				<button
					type="button"
					class="bar-button"
					tabindex="-1"
					aria-label={t('rdp.keyboard')}
					onpointerdown={(e) => {
						e.preventDefault();
						toggleKeyboard();
					}}>⌨</button
				>
				<button
					type="button"
					class="bar-button"
					tabindex="-1"
					aria-label={fullscreen ? t('rdp.exit_fullscreen') : t('rdp.fullscreen')}
					onpointerdown={(e) => {
						e.preventDefault();
						void setFullscreen(!fullscreen);
					}}>{fullscreen ? '⤡' : '⤢'}</button
				>
				{#if !isVnc() && clipMode === 'all'}
					<button
						type="button"
						class="bar-button"
						tabindex="-1"
						aria-label={t('rdp.send_files')}
						title={t('rdp.send_files')}
						onpointerdown={(e) => {
							e.preventDefault();
							pickFiles();
						}}
						><svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path fill="currentColor" d="M10.5 3.5v7a2.5 2.5 0 0 1-5 0V3a1.5 1.5 0 0 1 3 0v6.5a.5.5 0 0 1-1 0V4H6.5v5.5a1.5 1.5 0 0 0 3 0V3a2.5 2.5 0 0 0-5 0v7.5a3.5 3.5 0 0 0 7 0v-7z"/></svg></button
					>
				{/if}
			{/snippet}
		</MobileKeyBar>
	{/if}
</div>

<style>
	.rdp {
		position: relative;
		display: flex;
		flex-direction: column;
		width: 100%;
		height: 100%;
		overflow: hidden;
		background: #000;
	}

	.screen {
		position: relative;
		flex: 1;
		min-height: 0;
		width: 100%;
		display: flex;
		align-items: center;
		justify-content: center;
		overflow: hidden;
	}

	/* Where a phone's keyboard types: there, and invisible. 16px keeps the
	   webview from zooming in on it when it takes focus. */
	.type-sink {
		position: absolute;
		left: 0;
		bottom: 0;
		width: 1px;
		height: 1px;
		opacity: 0;
		font-size: 16px;
		border: none;
		padding: 0;
		resize: none;
		pointer-events: none;
	}

	/* Scaled to fit, never stretched: the intrinsic size is the server's
	   framebuffer and CSS only ever shrinks it uniformly. */
	canvas {
		display: block;
		max-width: 100%;
		max-height: 100%;
		width: auto;
		height: auto;
		object-fit: contain;
		cursor: default;
		image-rendering: auto;
		outline: none;
	}

	/* A view: the whole picture at its own size, moved so that this
	   window's part of it is what shows. */
	.screen.view {
		display: block;
	}

	canvas.cut {
		position: absolute;
		max-width: none;
		max-height: none;
	}

	.seamless-note {
		background: var(--color-bg-primary, #111);
	}

	canvas.dim {
		opacity: 0.35;
	}

	.overlay {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		color: var(--color-text-secondary);
		font-size: 0.8125rem;
		pointer-events: none;
	}

	.overlay small {
		max-width: 60ch;
		text-align: center;
		color: var(--color-text-tertiary);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		word-break: break-word;
	}

	.overlay.failed {
		color: var(--color-danger);
	}

	/* A VNC server's certificate to trust: the one part of the overlay
	   that takes clicks. */
	.cert-ask {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		max-width: 60ch;
		padding: 0 16px;
		text-align: center;
		color: var(--color-text-primary);
		pointer-events: auto;
	}

	.cert-hint {
		color: var(--color-text-secondary);
	}

	.cert-actions {
		display: flex;
		gap: 8px;
	}

	.spinner {
		width: 18px;
		height: 18px;
		border: 2px solid var(--color-border);
		border-top-color: var(--color-accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	/* Full screen: over everything, the whole window, black behind. */
	.rdp.fullscreen {
		position: fixed;
		inset: 0;
		z-index: 1000;
		background: #000;
		/* The desktop stays clear of a phone's status and navigation bars;
		   the screen area inside is what gets measured, so scaling follows. */
		padding: var(--safe-top) var(--safe-right) var(--safe-bottom) var(--safe-left);
	}

	.fs-button {
		position: absolute;
		top: 8px;
		right: 8px;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 5px 10px;
		font: inherit;
		font-size: 0.75rem;
		color: #fff;
		background: rgba(0, 0, 0, 0.55);
		border: 1px solid rgba(255, 255, 255, 0.25);
		border-radius: var(--radius-btn);
		cursor: pointer;
		touch-action: none;
		opacity: 0;
		transition: opacity 150ms ease;
	}

	.fs-button.placed {
		right: auto;
	}

	.fs-button.dragging {
		cursor: grabbing;
		opacity: 1;
	}

	.rdp:hover .fs-button,
	.fs-button:focus-visible {
		opacity: 1;
	}

	/* The way back: a bar tucked above the top edge, shown when asked for. */
	.fs-bar {
		position: absolute;
		top: 0;
		left: 50%;
		transform: translate(-50%, -100%);
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 6px 14px;
		font-size: 0.75rem;
		color: #fff;
		background: rgba(0, 0, 0, 0.75);
		border: 1px solid rgba(255, 255, 255, 0.2);
		border-top: none;
		border-radius: 0 0 var(--radius-btn) var(--radius-btn);
		transition: transform 180ms ease;
		z-index: 1;
	}

	.fs-bar.shown,
	.fs-bar:hover {
		transform: translate(-50%, 0);
	}

	.fs-host {
		opacity: 0.8;
	}

	.fs-icon,
	.fs-btn {
		display: inline-flex;
		align-items: center;
		padding: 3px 8px;
		font: inherit;
		font-size: 0.75rem;
		color: #fff;
		background: rgba(255, 255, 255, 0.12);
		border: none;
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	.fs-icon {
		padding: 3px 5px;
		opacity: 0.6;
	}

	.fs-icon.on {
		opacity: 1;
		background: var(--color-accent-fill);
	}

	.fs-btn:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.fs-keys {
		position: relative;
	}

	.fs-menu {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		display: flex;
		flex-direction: column;
		min-width: 160px;
		padding: 4px;
		background: rgba(0, 0, 0, 0.88);
		border: 1px solid rgba(255, 255, 255, 0.2);
		border-radius: var(--radius-btn);
	}

	.fs-menu button {
		padding: 5px 10px;
		font: inherit;
		font-size: 0.75rem;
		color: #fff;
		text-align: left;
		background: none;
		border: none;
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	.fs-menu button:hover,
	.fs-menu button:focus-visible {
		background: rgba(255, 255, 255, 0.15);
	}

	.fs-exit {
		padding: 3px 10px;
		font: inherit;
		font-size: 0.75rem;
		color: #fff;
		background: var(--color-accent-fill);
		border: none;
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	/* Files dragged over the desktop. */
	.drop-zone {
		position: absolute;
		inset: 8px;
		display: flex;
		align-items: center;
		justify-content: center;
		border: 2px dashed var(--color-accent);
		border-radius: var(--radius-btn);
		background: color-mix(in srgb, var(--color-bg-primary) 55%, transparent);
		color: var(--color-text-primary);
		font-size: 0.875rem;
		pointer-events: none;
		z-index: 4;
	}

	/* A clipboard transfer, or a large remote copy waiting for a yes. */
	.clip-bar {
		position: absolute;
		left: 50%;
		bottom: 12px;
		transform: translateX(-50%);
		display: flex;
		align-items: center;
		gap: 10px;
		max-width: calc(100% - 24px);
		padding: 6px 10px;
		font-size: 0.75rem;
		color: var(--color-text-primary);
		background: var(--color-bg-elevated, var(--color-bg-secondary));
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
		z-index: 5;
	}

	.clip-text {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.clip-bar progress {
		width: 120px;
		flex: none;
		accent-color: var(--color-accent);
	}

	.clip-btn {
		flex: none;
		padding: 3px 10px;
		font: inherit;
		color: var(--color-text-primary);
		background: transparent;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	/* RemoteApp: one button per program window, like a taskbar. */
	.rail-row {
		flex: none;
		display: flex;
		gap: 4px;
		padding: 4px 6px;
		overflow-x: auto;
		background: var(--color-bg-secondary);
		border-bottom: 1px solid var(--color-border);
	}

	.rail-win {
		flex: none;
		display: flex;
		align-items: center;
		max-width: 240px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		background: var(--color-bg-elevated, var(--color-bg-primary));
	}

	.rail-win.minimized {
		opacity: 0.6;
	}

	.rail-title,
	.rail-close {
		font: inherit;
		font-size: 0.75rem;
		color: var(--color-text-primary);
		background: transparent;
		border: none;
		cursor: pointer;
	}

	.rail-title {
		overflow: hidden;
		padding: 3px 8px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.rail-close {
		padding: 3px 6px;
		color: var(--color-text-secondary);
	}

	.rail-close:hover {
		color: var(--color-text-primary);
	}

	.clip-btn.primary {
		color: #fff;
		background: var(--color-accent-fill);
		border-color: transparent;
	}
</style>
