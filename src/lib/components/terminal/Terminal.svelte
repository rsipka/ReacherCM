<script lang="ts">
	import { untrack } from 'svelte';
	import { Terminal } from '@xterm/xterm';
	import { FitAddon } from '@xterm/addon-fit';
	import { WebglAddon } from '@xterm/addon-webgl';
	import { WebLinksAddon } from '@xterm/addon-web-links';
	import { SearchAddon } from '@xterm/addon-search';
	import { registerFinder, requestFind } from '#lib/state/terminal-find.svelte.js';
	import { registerCapture } from '#lib/state/capture.js';
	import { snapshotTerminal } from '#lib/terminal/snapshot.js';
	import { Unicode11Addon } from '@xterm/addon-unicode11';
	import { SerializeAddon } from '@xterm/addon-serialize';
	import { registerHandoff, requestSnapshot } from '#lib/state/windows.svelte.js';
	import { chunkKey, overlap, TAIL_CHUNKS, type Chunk } from '#lib/terminal/float-windows.js';
	import '@xterm/xterm/css/xterm.css';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { ptyWrite, ptyResize } from '#lib/ipc/pty.js';
	import { sshSend, sshResize, sshConnect, sshReady, sshDisconnect, sshProbeAddresses, type SshConnectParams } from '#lib/ipc/ssh.js';
	import { addressKey, addressName, autoOrder, isUnreachable, plainOrder, rememberAddress } from '#lib/sessions/addresses.js';
	import { telnetConnect, telnetDisconnect, telnetSend, telnetResize, type TelnetConnectParams } from '#lib/ipc/telnet.js';
	import { serialConnect, serialDisconnect, serialSend, serialBreak, serialSummary as serialLabel, type SerialConnectParams } from '#lib/ipc/serial.js';
	import { untrustedRefsFromError } from '#lib/ipc/sessions.js';
	import { noteLineEntered } from '#lib/state/explorer.svelte.js';
	import { registerBufferReader, unregisterBufferReader } from '#lib/state/terminal-buffer.svelte.js';
	import { terminalPalette } from '#lib/state/terminal-themes.svelte.js';
	import { getSettings, updateSetting, sessionLogConfig } from '#lib/state/settings.svelte.js';
	import { historyStart, type HistoryMode } from '#lib/ipc/history.js';
	import { setTabHistory, openHistoryPanel, openCommandPicker } from '#lib/state/history.svelte.js';
	import { countPasteLines, shouldWarnOnPaste } from '#lib/terminal/paste.js';
	import { autoReconnectDelay, needsUser } from '#lib/terminal/tmux.js';
	import { osc52Text } from '#lib/terminal/osc52.js';
	import { rightClickAction, wordSeparator, MIDDLE_CLICK_PASTE_MS } from '#lib/terminal/mouse.js';
	import { shouldRearmIme, nudgeCaret, REARM_SETTLE_MS } from '#lib/terminal/ime.js';
	import { installWebkitInputFix } from '#lib/terminal/webkit-input-fix.js';
	import { deferLinuxSelectionSync } from '#lib/terminal/selection-sync.js';
	import { isMac, isMobile, isWebKit, hasWebgl2 } from '#lib/platform.js';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { invoke } from '@tauri-apps/api/core';
	import { trieMatch, registerTerminalInput, unregisterTerminalInput, type TerminalInput } from '#lib/state/snippets.svelte.js';
	import { terminalFocusRequests } from '#lib/state/palette.svelte.js';
	import { setTabLink, closeTab, clearHandoff, type Handoff } from '#lib/state/tabs.svelte.js';
	import { broadcastInput, getBroadcastTargets } from '#lib/state/broadcast.svelte.js';
	import { isTerminalReply, TYPED_WINDOW_MS } from '#lib/terminal/broadcast.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { readText as clipboardReadText, writeText as clipboardWriteText } from '@tauri-apps/plugin-clipboard-manager';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { URL_PATTERN } from '#lib/terminal/links.js';
	import { bellActions, playBell, lastBellSound, windowSeen, notifyBell } from '#lib/terminal/bell.js';
	import Modal from '#lib/components/shared/Modal.svelte';
	import { openSettings, matchesAction, actionLabel } from '#lib/state/shortcuts.svelte.js';
	import MobileKeyBar, { type BarKey } from '#lib/components/shared/MobileKeyBar.svelte';
	import ZmodemBar from './ZmodemBar.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import DbMenu, { type MenuItem } from '#lib/components/db/DbMenu.svelte';
	import { cssFontFamily } from '#lib/utils/font.js';
	import { cursorOptions } from '#lib/terminal/cursor.js';
	import { scrollbackLines } from '#lib/terminal/scrollback.js';
	import { LineClock, gutterTime, numberWidth } from '#lib/terminal/gutter.js';
	import { Highlighter } from '#lib/terminal/highlighter.js';
	import { highlightColors, highlightKinds, parseCustomRules } from '#lib/terminal/highlight.js';

	interface Props {
		ptyId: string;
		type: 'local' | 'ssh' | 'telnet' | 'serial';
		connectionId?: string;
		/** Has the keyboard. */
		active: boolean;
		/** Shown, which in a split (#106) both panes are; only one is active. Active when not given. */
		visible?: boolean;
		onTitleChange?: (title: string) => void;
		sshConnectParams?: SshConnectParams;
		/** Bumped by Switch tmux session: connect again, into the tmux
		 *  session sshConnectParams now names. */
		tmuxSwitch?: number;
		/** Bumped by the tab menu's Reconnect: drop the connection and make it
		 *  again (SSH, Telnet, serial). */
		reconnectRequest?: number;
		/** Telnet: where to connect. The terminal connects once it listens. */
		telnetConnectParams?: TelnetConnectParams;
		/** Serial: the port to open. The terminal opens it once it listens. */
		serialConnectParams?: SerialConnectParams;
		onReconnected?: (newConnectionId: string) => void;
		onReady?: () => void;
		/** The session's terminal theme id; unset: the default from Settings. */
		terminalTheme?: string | null;
		/** The session's cursor shape and blink; unset: the defaults from Settings. */
		cursorStyle?: string | null;
		cursorBlink?: boolean | null;
		/** The saved session the tab came from, for its history. */
		sessionId?: string | null;
		sessionName?: string | null;
		/** The session's history choice; unset: the default from Settings. */
		historyMode?: HistoryMode | null;
		/** A WSL tab's distribution. */
		wslDistro?: string | null;
		/** Moved here from another window: take over the terminal there
		 *  (see #lib/state/windows) instead of starting empty. */
		handoff?: Handoff;
		/** Moved here with its connection already down: show Reconnect. */
		startDown?: boolean;
	}

	let { ptyId, type: termType, connectionId, active, visible, onTitleChange, sshConnectParams, tmuxSwitch, reconnectRequest, telnetConnectParams, serialConnectParams, onReconnected, onReady, terminalTheme, cursorStyle, cursorBlink, sessionId, sessionName, historyMode, wslDistro, handoff, startDown }: Props = $props();

	/**
	 * Start recording this tab's stream into the session history (and, for
	 * terminals other than SSH, into the file log), before its data flows:
	 * an SSH session holds its output until `ssh_ready`, a Telnet one
	 * connects after its listeners are up. Asking again for a stream
	 * already recorded keeps what is there.
	 */
	async function registerHistory(stream: string): Promise<void> {
		const s = getSettings();
		const mode = historyMode ?? s.historyMode;
		const ssh = termType === 'ssh';
		const secrets = [sshConnectParams?.password, sshConnectParams?.keyPassphrase, telnetConnectParams?.password].filter(
			(x): x is string => !!x
		);
		const host = sshConnectParams?.host ?? telnetConnectParams?.host ?? serialConnectParams?.port ?? wslDistro ?? 'localhost';
		try {
			const status = await historyStart(stream, {
				meta: {
					sessionId: sessionId ?? null,
					name: sessionName ?? '',
					protocol: wslDistro ? 'wsl' : termType,
					host,
					user: sshConnectParams?.username ?? telnetConnectParams?.username ?? ''
				},
				level: mode === 'off' ? null : mode,
				secrets,
				patterns: s.historyPatterns.split('\n').map((p) => p.trim()).filter(Boolean),
				// SSH writes its own file log, asked for at connect.
				log: ssh ? null : sessionLogConfig(sessionName ?? '')
			});
			setTabHistory(ptyId, stream, status, ssh && sessionLogConfig() !== null);
		} catch (e) {
			console.warn('Session history:', e);
		}
	}

	/** The colours this terminal paints with. */
	let palette = $derived(terminalPalette(terminalTheme));
	/** The cursor it draws: the session's choice over Settings'. */
	let cursorLook = $derived(cursorOptions(getSettings(), { style: cursorStyle, blink: cursorBlink }));
	/** A connection made by Reach (SSH, Telnet, a serial port) that can drop and be made again. */
	let remote = $derived(termType === 'ssh' || termType === 'telnet' || termType === 'serial');
	/** Telnet and serial: the terminal makes the connection itself, once it listens. */
	let selfConnecting = $derived(termType === 'telnet' || termType === 'serial');
	let shown = $derived(visible ?? active);

	// Local state seeded from the `connectionId` prop and overridden in-place on
	// reconnect (handleReconnect assigns a new UUID and notifies the parent via
	// onReconnected). Use $effect.pre so the prop value is captured before the
	// first render — avoids the state_referenced_locally warning that fires when
	// reading a reactive prop directly inside $state(...).
	let currentConnectionId = $state<string | undefined>();
	let disconnected = $state(untrack(() => !!handoff && !!startDown));
	let reconnecting = $state(false);
	/** The remote shell ended on its own rather than the connection dropping. */
	let cleanEnd = $state(false);

	$effect.pre(() => {
		currentConnectionId = connectionId;
	});

	// tmux tabs (see #lib/terminal/tmux): a dropped connection costs
	// nothing on the server, so the tab reconnects by itself, at once when
	// the network comes back or the app is shown again (a phone app that
	// was in the background).
	let inTmux = $derived(termType === 'ssh' && !!sshConnectParams?.tmux);
	let autoReconnecting = $state(false);
	let autoAttempt = 0;
	let autoSince = 0;
	let autoTimer: ReturnType<typeof setTimeout> | undefined;

	function startAutoReconnect(): void {
		if (!inTmux || autoReconnecting) return;
		autoReconnecting = true;
		autoAttempt = 0;
		autoSince = Date.now();
		scheduleAutoReconnect();
	}

	function scheduleAutoReconnect(): void {
		clearTimeout(autoTimer);
		if (!autoReconnecting) return;
		const wait = autoReconnectDelay(autoAttempt, Date.now() - autoSince);
		if (wait === null) {
			stopAutoReconnect();
			return;
		}
		autoTimer = setTimeout(() => void autoReconnectNow(), wait);
	}

	function stopAutoReconnect(): void {
		autoReconnecting = false;
		clearTimeout(autoTimer);
	}

	async function autoReconnectNow(): Promise<void> {
		if (!autoReconnecting || reconnecting || !disconnected) return;
		clearTimeout(autoTimer);
		autoAttempt += 1;
		const error = await handleReconnect();
		if (error === null || needsUser(error)) stopAutoReconnect();
		else scheduleAutoReconnect();
	}

	// Switch tmux session: this connection is closed on purpose (its exit
	// is not listened to, so nothing reconnects it) and a new one opens
	// into the session now named.
	let lastSwitch: number | undefined;
	$effect(() => {
		const n = tmuxSwitch;
		if (lastSwitch === undefined || n === undefined || n === lastSwitch) {
			lastSwitch = n ?? 0;
			return;
		}
		lastSwitch = n;
		untrack(() => void switchTmux());
	});

	// Reconnect from the tab menu: the same, on a connection that may still
	// be up (a hung one, say) or already down.
	let lastReconnect: number | undefined;
	$effect(() => {
		const n = reconnectRequest;
		if (lastReconnect === undefined || n === undefined || n === lastReconnect) {
			lastReconnect = n ?? 0;
			return;
		}
		lastReconnect = n;
		untrack(() => void reconnectNow());
	});

	async function reconnectNow(): Promise<void> {
		if (!terminal || reconnecting) return;
		if (termType === 'ssh') {
			await switchTmux();
		} else if (termType === 'telnet') {
			// Not listened to any more, so its end does not close the tab.
			const old = currentConnectionId;
			unlistenData?.();
			unlistenExit?.();
			if (old && !disconnected) await telnetDisconnect(old).catch(() => {});
			await connectTelnet(terminal);
		} else if (termType === 'serial') {
			await connectSerial(terminal);
		}
	}

	async function switchTmux(): Promise<void> {
		if (!terminal || termType !== 'ssh' || reconnecting) return;
		stopAutoReconnect();
		const old = currentConnectionId;
		unlistenData?.();
		unlistenExit?.();
		if (old) await sshDisconnect(old).catch(() => {});
		disconnected = true;
		cleanEnd = false;
		await handleReconnect();
	}

	$effect(() => {
		if (!inTmux) return;
		const wake = () => {
			if (document.visibilityState === 'visible') void autoReconnectNow();
		};
		window.addEventListener('online', wake);
		document.addEventListener('visibilitychange', wake);
		return () => {
			window.removeEventListener('online', wake);
			document.removeEventListener('visibilitychange', wake);
			stopAutoReconnect();
		};
	});

	let containerEl: HTMLDivElement | undefined = $state();
	let terminal: Terminal | undefined = $state();

	// A phone: the extra-keys bar, and the sticky Ctrl and Alt it holds.
	const onPhone = isMobile();
	let barCtrl = $state(false);
	let barAlt = $state(false);

	/** The font family the terminal draws with, for the one in settings.
	 *  On a phone, the default `monospace` becomes the JetBrains Mono Reach
	 *  ships: phones that let the user change the system font (ColorOS, MIUI,
	 *  One UI) can hand `monospace` a proportional font, whose narrow letters
	 *  then sit spread out in cells sized for its widest one (#127).
	 *  On a Mac (and an iPad), `monospace` alone is resolved differently by
	 *  WebKit's DOM and its canvas, so the WebGL renderer drew some glyphs in
	 *  one font and the ones it drew later (a selection, a new colour) in
	 *  another: selected text grew. It is named as the system monospace
	 *  font (SF Mono, then Menlo) instead. */
	function terminalFont(family: string | undefined): string {
		const f = family || 'monospace';
		if (f !== 'monospace') return cssFontFamily(f);
		if (onPhone) return '"JetBrains Mono", monospace';
		return isMac() ? 'ui-monospace, Menlo, monospace' : f;
	}

	/** Make the terminal's text a size larger or smaller, from 8 to 32 px,
	 *  and keep it for every terminal. */
	function zoomBy(step: 1 | -1): void {
		const term = terminal;
		if (!term || !fitAddon || !containerEl) return;
		const current = term.options.fontSize ?? 14;
		const next = Math.min(Math.max(current + step, 8), 32);
		if (next === current) return;
		term.options.fontSize = next;
		term.clearTextureAtlas();
		safeFitAndResize(term, fitAddon, containerEl);
		updateSetting('fontSize', next);
	}

	/** Load a terminal font before xterm measures it. A single name is
	 *  quoted, as a family may have spaces; a list already is, and quoting
	 *  the generic `monospace` would name a font called "monospace". */
	function loadFont(family: string, size: number): Promise<unknown> {
		const name = family === 'monospace' || family.includes(',') ? family : `"${family}"`;
		return Promise.all([
			document.fonts.load(`${size}px ${name}`),
			document.fonts.load(`bold ${size}px ${name}`)
		]);
	}

	/** Apply a sticky Ctrl or Alt from the key bar to text the phone keyboard
	 *  typed, then let go of them: they apply to one key, as on Termux. */
	function applyBarModifiers(data: string): string {
		if (!barCtrl && !barAlt) return data;
		let out = data;
		if (barCtrl && data.length === 1) {
			const c = data.toLowerCase().charCodeAt(0);
			if (c >= 97 && c <= 122) out = String.fromCharCode(c - 96);
			else if (data === ' ' || data === '@') out = '\x00';
			else if ('[\\]^_'.includes(data)) out = String.fromCharCode(data.charCodeAt(0) & 0x1f);
			else if (data === '?') out = '\x7f';
		}
		if (barAlt) out = '\x1b' + out;
		barCtrl = false;
		barAlt = false;
		return out;
	}

	/** A key from the bar, as the bytes a real keyboard's key would send. */
	function onBarKey(key: BarKey): void {
		if (key === 'smaller' || key === 'larger') {
			zoomBy(key === 'larger' ? 1 : -1);
			return;
		}
		const appCursor = terminal?.modes.applicationCursorKeysMode ?? false;
		// xterm's modifier parameter: 3 is Alt, 5 is Ctrl, 7 is both.
		const mod = (barAlt ? 2 : 0) + (barCtrl ? 4 : 0);
		const cursor = (letter: string): string =>
			mod ? `\x1b[1;${mod + 1}${letter}` : appCursor ? `\x1bO${letter}` : `\x1b[${letter}`;
		const tilde = (n: number): string => (mod ? `\x1b[${n};${mod + 1}~` : `\x1b[${n}~`);
		let seq: string;
		if (typeof key === 'object' && 'fn' in key) {
			// F1-F4 are SS3 P-S (CSI 1;m P-S with a modifier); F5 and up are
			// CSI n ~ with xterm's numbers, which skip 16 and 22.
			const n = key.fn;
			seq = n <= 4
				? mod ? `\x1b[1;${mod + 1}${'PQRS'[n - 1]}` : `\x1bO${'PQRS'[n - 1]}`
				: tilde([15, 17, 18, 19, 20, 21, 23, 24][n - 5]);
			barCtrl = false;
			barAlt = false;
		} else if (typeof key === 'object') {
			seq = applyBarModifiers(key.char);
		} else {
			switch (key) {
				case 'esc': seq = '\x1b'; break;
				case 'tab': seq = barAlt ? '\x1b\t' : '\t'; break;
				case 'up': seq = cursor('A'); break;
				case 'down': seq = cursor('B'); break;
				case 'right': seq = cursor('C'); break;
				case 'left': seq = cursor('D'); break;
				case 'home': seq = cursor('H'); break;
				case 'end': seq = cursor('F'); break;
				case 'pgup': seq = tilde(5); break;
				case 'pgdn': seq = tilde(6); break;
				case 'del': seq = tilde(3); break;
				default: return;
			}
			barCtrl = false;
			barAlt = false;
		}
		typed(Array.from(new TextEncoder().encode(seq)));
		terminal?.focus();
	}
	let fitAddon: FitAddon | undefined = $state();
	let snippetInput: TerminalInput | undefined;

	// Pending multiline paste awaiting user confirmation (guards against
	// accidentally executing pasted multi-line commands). Whether it asks
	// at all, and from how many lines, is the user's to decide — see
	// #lib/terminal/paste.
	let pastePreview = $state<string | null>(null);
	let dontAskAgain = $state(false);
	let pasteLineCount = $derived(pastePreview ? countPasteLines(pastePreview) : 0);

	/** Copy the current selection to the OS clipboard via the Tauri plugin.
	 *  A copy the user asked for lets go of the selection; one made by
	 *  selecting (copy on select) keeps it on screen. */
	async function copySelection(term: Terminal, keep = false): Promise<void> {
		// An empty selection (a double click on a blank) is not worth the
		// clipboard's contents.
		const text = term.hasSelection() ? term.getSelection() : '';
		if (!text) return;
		try {
			await clipboardWriteText(text);
		} catch (e) {
			console.error('Clipboard write failed:', e);
		}
		if (!keep) term.clearSelection();
	}

	// The right-click menu, when that is what a right click opens
	// (#lib/terminal/mouse).
	let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

	function openMenu(term: Terminal, x: number, y: number): void {
		const mod = isMac() ? '⌘' : 'Ctrl+';
		// Escape closes the menu; it is not for the shell as well.
		term.blur();
		menu = {
			x,
			y,
			items: [
				{ label: t('textmenu.copy'), hint: `${mod}C`, disabled: !term.hasSelection(), action: () => void copySelection(term) },
				{ label: t('textmenu.paste'), hint: `${mod}V`, action: () => void pasteFromClipboard(term) },
				{ label: '', separator: true },
				{
					label: t('terminal.menu_find'),
					hint: `${isMac() ? '⌘⇧' : 'Ctrl+Shift+'}F`,
					action: () => requestFind(ptyId, findText(term))
				},
				{ label: t('textmenu.select_all'), action: () => term.selectAll() },
				{ label: t('terminal.menu_clear'), action: () => term.clear() },
				...(termType === 'serial'
					? [
							{ label: '', separator: true },
							{ label: t('serial.send_break'), disabled: disconnected, action: sendBreak }
						]
					: []),
				{ label: '', separator: true },
				{
					label: t('history.picker_menu'),
					hint: actionLabel('terminal.commandPicker'),
					action: () => openCommandPicker({ text: findText(term), sessionId, sessionName, tabId: ptyId })
				},
				{
					label: t('history.search_menu'),
					hint: `${isMac() ? '⌘⇧' : 'Ctrl+Shift+'}H`,
					action: () => openHistoryPanel({ text: term.getSelection().trim().slice(0, 200), sessionId, sessionName, tabId: ptyId })
				}
			]
		};
	}

	/** A one-line selection is what to look for; longer ones are not. */
	function findText(term: Terminal): string | undefined {
		const text = term.getSelection().trim();
		return text && !text.includes('\n') ? text.slice(0, 200) : undefined;
	}

	function closeMenu(): void {
		menu = null;
		terminal?.focus();
	}

	/** Read the OS clipboard via the Tauri plugin and paste — single-line goes
	 *  straight in; multi-line opens a confirmation first. */
	async function pasteFromClipboard(term: Terminal): Promise<void> {
		let text = '';
		try {
			text = (await clipboardReadText()) ?? '';
		} catch (e) {
			console.error('Clipboard read failed:', e);
			return;
		}
		pasteText(term, text);
	}

	/**
	 * Paste text the user has produced, wherever it came from: the clipboard
	 * plugin on Ctrl+V, or the native paste event macOS delivers for Cmd+V
	 * and Edit → Paste. One entry point, so the multi-line confirmation is
	 * never bypassed by one of the routes.
	 */
	function pasteText(term: Terminal, text: string): void {
		if (!text) return;
		const settings = getSettings();
		const warn = shouldWarnOnPaste(text, {
			enabled: settings.warnOnMultilinePaste,
			threshold: settings.multilinePasteThreshold
		});
		if (warn) {
			pastePreview = text;
		} else {
			pasteTyped(term, text);
		}
	}

	/** A paste the user asked for, which goes where typing goes. */
	function pasteTyped(term: Terminal, text: string): void {
		pasting = true;
		try {
			term.paste(text);
		} finally {
			pasting = false;
		}
	}

	// What xterm sends its program is only passed on to the terminals this
	// one broadcasts to when the user typed or pasted it, not a mouse report
	// or an answer to the program (#lib/terminal/broadcast).
	let pasting = false;
	let userInputAt = -Infinity;
	function markUserInput(): void {
		userInputAt = performance.now();
	}
	function isTyped(data: string): boolean {
		return pasting || (performance.now() - userInputAt < TYPED_WINDOW_MS && !isTerminalReply(data));
	}

	/** How many other terminals what is typed here goes to as well. */
	let broadcastCount = $derived.by(() => {
		const ids = getBroadcastTargets();
		return ids.includes(ptyId) ? ids.length - 1 : 0;
	});

	/**
	 * Pasting with "do not ask again" ticked turns the setting off for
	 * good, not only for this paste. Settings is where it comes back on,
	 * and the checkbox label says so rather than leaving it to be guessed.
	 */
	function confirmPaste(): void {
		if (dontAskAgain) updateSetting('warnOnMultilinePaste', false);
		if (pastePreview && terminal) pasteTyped(terminal, pastePreview);
		pastePreview = null;
		dontAskAgain = false;
		terminal?.focus();
	}

	/**
	 * Give the input method a fresh caret position after the window moved.
	 *
	 * A drag changes the window's position but not its layout, so the caret's
	 * rectangle never changes and nothing new is pushed to the input method —
	 * which goes on using a position it cached in screen coordinates before
	 * the window moved (issue #49). A resize does not suffer this because it
	 * re-lays-out. Moving the caret a pixel and putting it back is what makes
	 * the position be recomputed. See #lib/terminal/ime for what was measured
	 * and ruled out, including the blur-and-focus this replaces.
	 */
	function rearmIme(): void {
		const ta = terminal?.textarea;
		if (!ta) return;
		if (!shouldRearmIme({ focused: document.activeElement === ta, composing })) return;

		// The thing that actually fixes it, found by the reporter of #49:
		// clicking another window and back. That is the native window losing
		// and regaining focus, which makes wry call MoveFocus on the WebView2
		// controller — and the controller is what holds the stale caret
		// position. A title-bar drag never produces that, so we do it
		// deliberately. Two earlier attempts worked at the wrong level: the
		// DOM textarea's focus, and the caret's rectangle. Neither reaches
		// the controller.
		void invoke('webview_refocus').catch(() => {
			// Not on Windows, or no controller. Nothing is lost by trying.
		});

		nudgeCaret(ta);
	}

	function cancelPaste(): void {
		pastePreview = null;
		dontAskAgain = false;
		terminal?.focus();
	}

	let unlistenData: UnlistenFn | undefined;
	let unlistenExit: UnlistenFn | undefined;

	// ---- Moving between windows (#lib/state/windows) ----
	/** The last output chunks shown, for the window taking this tab over. */
	let recent: Chunk[] = [];
	/** While taking over from another window: output held until its screen is in. */
	let holding: Chunk[] | null = null;
	let serializer: SerializeAddon | undefined;
	/** Given to another window: nothing here touches the connection again. */
	let handedOff = false;
	let stopHandoff: (() => void) | undefined;

	function writeChunk(term: Terminal, payload: Chunk): void {
		if (payload instanceof Uint8Array) term.write(payload);
		else if (Array.isArray(payload)) term.write(new Uint8Array(payload));
		else if (typeof payload === 'string') term.write(payload);
	}

	/** The other window's terminal as it stood, then what came since. */
	async function takeOver(term: Terminal, h: Handoff): Promise<void> {
		const snap = await requestSnapshot(ptyId, h);
		const held = holding ?? [];
		holding = null;
		let from = 0;
		if (snap) {
			term.write(snap.data);
			from = overlap(snap.tail, held.map(chunkKey));
		}
		for (const c of held.slice(from)) writeChunk(term, c);
		recent = held.slice(-TAIL_CHUNKS);
		clearHandoff(ptyId);
	}

	/** Hand this terminal to the window taking the tab: its screen, and it stops listening. */
	function handOver(): { data: string; tail: string[] } | null {
		handedOff = true;
		unlistenData?.();
		unlistenExit?.();
		unlistenData = undefined;
		unlistenExit = undefined;
		const data = serializer?.serialize() ?? '';
		return { data, tail: recent.map(chunkKey) };
	}
	let unlistenMenuCopy: UnlistenFn | undefined;
	let unlistenMenuPaste: UnlistenFn | undefined;
	let unlistenMoved: UnlistenFn | undefined;
	let unlistenResized: UnlistenFn | undefined;
	let unlistenSelectEnd: (() => void) | undefined;
	// The last middle click, and whether a paste has answered it.
	let middleClickAt = -Infinity;
	let middlePasted = false;
	let middleBlocked = false;
	let disposeWebkitInputFix: (() => void) | undefined;
	/** True between compositionstart and compositionend — see #lib/terminal/ime. */
	let composing = false;
	let rearmTimer: ReturnType<typeof setTimeout> | undefined;
	let resizeObserver: ResizeObserver | undefined;

	// Snippet autocomplete (Trie-based, ghost text via xterm Decoration API)
	let inputBuffer = $state('');
	let suggestion = $state<{ command: string; name: string; ghost: string } | null>(null);
	let ghostDecoration: { dispose: () => void } | undefined;
	let ghostMarker: { dispose: () => void } | undefined;

	let ghostEl: HTMLDivElement | undefined;

	function showGhostDecoration(term: Terminal, ghost: string): void {
		clearGhostDecoration();
		if (!term.element) return;

		const cursorX = term.buffer.active.cursorX;
		const cursorY = term.buffer.active.cursorY - term.buffer.active.viewportY;

		// Get exact cell dimensions and canvas padding from xterm internals
		let cellW: number, cellH: number, padLeft = 0, padTop = 0;
		try {
			const dims = (term as any)._core._renderService.dimensions;
			cellW = dims.css.cell.width;
			cellH = dims.css.cell.height;
			padLeft = dims.css.canvas.left ?? 0;
			padTop = dims.css.canvas.top ?? 0;
		} catch {
			const screen = term.element.querySelector('.xterm-screen');
			if (!screen) return;
			cellW = screen.clientWidth / term.cols;
			cellH = screen.clientHeight / term.rows;
		}

		const screen = term.element.querySelector('.xterm-screen') as HTMLElement;
		if (!screen) return;

		const el = document.createElement('div');
		el.textContent = ghost;
		el.style.position = 'absolute';
		el.style.left = `${screen.offsetLeft + padLeft + cursorX * cellW}px`;
		el.style.top = `${screen.offsetTop + padTop + cursorY * cellH}px`;
		el.style.height = `${cellH}px`;
		el.style.lineHeight = `${cellH}px`;
		el.style.fontSize = `${term.options.fontSize ?? 14}px`;
		el.style.fontFamily = term.options.fontFamily || 'monospace';
		el.style.letterSpacing = '0px';
		// Set from JS, so it never followed the theme; read the token instead.
		el.style.color = 'var(--color-text-tertiary)';
		el.style.pointerEvents = 'none';
		el.style.whiteSpace = 'pre';
		el.style.zIndex = '10';
		el.style.overflow = 'visible';

		term.element.appendChild(el);
		ghostEl = el;
	}

	function clearGhostDecoration(): void {
		if (ghostEl) {
			ghostEl.remove();
			ghostEl = undefined;
		}
	}

	function createTerminal(): Terminal {
		const appSettings = getSettings();
		return new Terminal({
			fontFamily: terminalFont(appSettings.fontFamily),
			fontSize: appSettings.fontSize ?? 14,
			...cursorLook,
			scrollback: scrollbackLines(appSettings.terminalScrollback, onPhone),
			wordSeparator: wordSeparator(appSettings.terminalWordSeparators),
			allowProposedApi: true,
			theme: { ...palette }
		});
	}

	// xterm keeps its palette in JS, not CSS, so a theme change has to be pushed
	// into every live terminal. Without this the terminal kept whatever colours
	// it was constructed with — which is why it stayed black in light mode.
	// The palette is the session's terminal theme, the default one, or the app
	// theme's (#lib/state/terminal-themes), and follows a change to any.
	$effect(() => {
		const p = palette;
		if (terminal) {
			terminal.options.theme = { ...p };
		}
	});

	// The cursor follows Settings and the session's choice while open. A
	// program's own shape (vim's insert mode) lasts until it asks again.
	$effect(() => {
		const c = cursorLook;
		const term = terminal;
		if (!term) return;
		if (term.options.cursorStyle !== c.cursorStyle) term.options.cursorStyle = c.cursorStyle;
		if (term.options.cursorBlink !== c.cursorBlink) term.options.cursorBlink = c.cursorBlink;
		if (term.options.cursorInactiveStyle !== c.cursorInactiveStyle) term.options.cursorInactiveStyle = c.cursorInactiveStyle;
		if (term.options.cursorWidth !== c.cursorWidth) term.options.cursorWidth = c.cursorWidth;
	});

	// Errors, warnings and addresses in colour, when Settings or the
	// terminal bar turn it on (#lib/terminal/highlight). Colours come from
	// the terminal theme, so they follow a theme change.
	let highlighter: Highlighter | undefined;
	$effect(() => {
		const s = getSettings();
		const options = {
			enabled: s.terminalHighlight,
			kinds: highlightKinds(s.terminalHighlightKinds),
			custom: parseCustomRules(s.terminalHighlightCustom).rules,
			colors: highlightColors(palette)
		};
		const term = terminal;
		if (!term) return;
		if (!highlighter && !options.enabled) return;
		highlighter ??= new Highlighter(term);
		highlighter.setOptions(options);
	});

	/** A server tried OSC 52 while it is off: said once per tab. */
	let osc52Told = false;
	let unregisterFinder: (() => void) | undefined;
	let unregisterCapture: (() => void) | undefined;

	function loadAddons(term: Terminal, fit: FitAddon, host: HTMLElement): void {
		term.loadAddon(fit);
		serializer = new SerializeAddon();
		term.loadAddon(serializer);
		// OSC 52: the server copies to this computer's clipboard (tmux copy
		// mode, vim). Writes only, and only when Settings allow it.
		term.parser.registerOscHandler(52, (data) => {
			const text = osc52Text(data);
			if (text === null) return true;
			if (!getSettings().terminalOsc52) {
				if (!osc52Told) {
					osc52Told = true;
					addToast(t('osc52.blocked'), 'info', 8000, {
						label: t('settings.open_setting'),
						run: () => openSettings('settings.term_osc52')
					});
				}
				return true;
			}
			clipboardWriteText(text).catch((e) => console.error('Clipboard write failed:', e));
			return true;
		});
		// Open links in the OS default browser (Windows/macOS/Linux) via Tauri's
		// shell, not inside the WebView. Our pattern, not the addon's: its own
		// cut long links short at ! * ' ( ) (#lib/terminal/links).
		term.loadAddon(
			new WebLinksAddon(
				(_event, uri) => {
					openUrl(uri).catch((e) => console.error('Failed to open link:', e));
				},
				{ urlRegex: URL_PATTERN }
			)
		);

		// Find in the scrollback, for the terminal bar's search field.
		const search = new SearchAddon();
		term.loadAddon(search);
		const decorations = {
			matchBackground: '#e8b73066',
			activeMatchBackground: '#ff9f0a',
			matchOverviewRuler: '#e8b730',
			activeMatchColorOverviewRuler: '#ff9f0a'
		};
		unregisterFinder?.();
		unregisterFinder = registerFinder(ptyId, {
			next: (q, caseSensitive) => search.findNext(q, { caseSensitive, decorations }),
			previous: (q, caseSensitive) => search.findPrevious(q, { caseSensitive, decorations }),
			clear: () => {
				search.clearDecorations();
				term.clearSelection();
				term.focus();
			}
		});

		// A screenshot of what the terminal shows (the bar's camera button).
		unregisterCapture?.();
		unregisterCapture = registerCapture(ptyId, () => snapshotTerminal(term, host.querySelector<HTMLElement>('.xterm-screen')));

		const unicode11 = new Unicode11Addon();
		term.loadAddon(unicode11);
		term.unicode.activeVersion = '11';

		attachWebgl(term, host);
	}

	/**
	 * The WebGL renderer, with recovery that does not keep the user waiting.
	 *
	 * Heavy compositing — the Settings modal's backdrop blur during a theme
	 * repaint, on some GPUs — can take the WebGL context away. The canvas goes
	 * blank the instant that happens. The addon's own `onContextLoss` only
	 * fires after it has waited three seconds for the context to come back,
	 * and if the driver does hand it back inside that window the addon takes
	 * its restore path instead, which is the one that can stay blank for good.
	 * Three seconds of a dead-looking terminal reads as "stuck".
	 *
	 * So the `webglcontextlost` DOM event is caught here directly, the moment
	 * it fires — captured on the container, since it does not bubble — and the
	 * addon is replaced with a fresh one: new canvas, new context, no wait.
	 * After two losses in a row the DOM renderer stays; a GPU that keeps
	 * dropping contexts is not one to keep asking.
	 */
	function attachWebgl(term: Terminal, host: HTMLElement, attempt = 0): void {
		// No WebGL2 at all (a virtual machine without a GPU, say): the addon
		// throws when the terminal opens, and the terminal never started its
		// shell. The DOM renderer draws it instead.
		if (!hasWebgl2()) {
			console.warn('[Terminal] WebGL2 unavailable, using the DOM renderer');
			return;
		}
		let webgl: WebglAddon;
		try {
			// WebKit — WKWebView on macOS, WebKitGTK on Linux — presents a
			// WebGL canvas one frame behind unless the drawing buffer is
			// preserved, so a keystroke's echo waits for the next redraw: the
			// cursor blink, ~600ms later. Measured elsewhere at 528ms → 14ms
			// from this one flag. Chromium does not need it and pays a small
			// cost for it, so it stays off on WebView2 (issue #47).
			webgl = new WebglAddon(isWebKit());
		} catch {
			console.warn('[Terminal] WebGL addon unavailable, using the DOM renderer');
			return;
		}
		let replaced = false;
		const replace = () => {
			if (replaced) return;
			replaced = true;
			host.removeEventListener('webglcontextlost', onLost, true);
			try {
				webgl.dispose();
			} catch {
				// already gone
			}
			if (attempt < 2) attachWebgl(term, host, attempt + 1);
			try {
				term.refresh(0, term.rows - 1);
			} catch {
				// terminal may have been disposed mid-frame
			}
		};
		const onLost = (e: Event) => {
			if (e.target instanceof HTMLCanvasElement) replace();
		};
		host.addEventListener('webglcontextlost', onLost, true);
		webgl.onContextLoss(replace);
		term.loadAddon(webgl);
	}

	function sendData(data: number[]): void {
		if (handedOff) return;
		if (termType === 'ssh' && currentConnectionId) {
			// A line typed (Enter) may have changed the folder, for an
			// explorer following this terminal (#162).
			if (data.includes(13)) noteLineEntered(currentConnectionId);
			sshSend(currentConnectionId, data).catch((err) => {
				console.error('[Terminal] Failed to write to SSH:', err);
			});
		} else if (termType === 'telnet') {
			if (currentConnectionId && !disconnected) {
				telnetSend(currentConnectionId, data).catch((err) => {
					console.error('[Terminal] Failed to write to Telnet:', err);
				});
			}
		} else if (termType === 'serial') {
			if (currentConnectionId && !disconnected) {
				serialSend(currentConnectionId, data).catch((err) => {
					console.error('[Terminal] Failed to write to the serial port:', err);
				});
			}
		} else {
			ptyWrite(ptyId, data).catch((err) => {
				console.error('[Terminal] Failed to write to PTY:', err);
			});
		}
	}

	/** Input from the user here: into this terminal, and on to the others
	 *  it broadcasts to (#lib/state/broadcast). */
	function typed(data: number[]): void {
		sendData(data);
		broadcastInput(ptyId, data);
	}

	function sendResize(cols: number, rows: number): void {
		// Another window shows it now, at its own size.
		if (handedOff) return;
		if (termType === 'ssh' && currentConnectionId) {
			sshResize(currentConnectionId, cols, rows).catch((err) => {
				console.error('[Terminal] Failed to resize SSH:', err);
			});
		} else if (termType === 'telnet') {
			if (currentConnectionId && !disconnected) telnetResize(currentConnectionId, cols, rows).catch(() => {});
		} else if (termType === 'serial') {
			// A serial line has no window size to tell.
		} else {
			ptyResize(ptyId, cols, rows).catch((err) => {
				console.error('[Terminal] Failed to resize PTY:', err);
			});
		}
	}

	// Fit to container, then push the new geometry to the remote PTY — but only
	// when the container is actually visible. A hidden tab's wrapper has
	// display:none, so its container has 0×0 dimensions; FitAddon would clamp
	// to its 2×1 minimum and resize the remote PTY to a 2-column window,
	// permanently narrowing any output that arrives while the tab is hidden.
	function safeFitAndResize(term: Terminal, fit: FitAddon, el: HTMLDivElement): void {
		if (!term.element) return;
		if (el.clientWidth === 0 || el.clientHeight === 0) return;
		const prevCols = term.cols;
		const prevRows = term.rows;
		try {
			fit.fit();
			if (term.cols !== prevCols || term.rows !== prevRows) {
				sendResize(term.cols, term.rows);
			}
		} catch (err) {
			console.error('[Terminal] Fit error:', err);
		}
	}

	let suggestionTimer: ReturnType<typeof setTimeout> | undefined;
	let lastCursorX = -1;

	function updateSuggestion(): void {
		clearTimeout(suggestionTimer);
		if (inputBuffer.length < 2 || !terminal) {
			suggestion = null;
			clearGhostDecoration();
			return;
		}
		const match = trieMatch(inputBuffer);
		if (match) {
			const ghost = match.command.slice(inputBuffer.length);
			suggestion = { command: match.command, name: match.name, ghost };
			const term = terminal;
			// Wait for cursor to move (echo arrived) instead of fixed delay
			lastCursorX = term.buffer.active.cursorX;
			let attempts = 0;
			const poll = () => {
				attempts++;
				const nowX = term.buffer.active.cursorX;
				if (nowX !== lastCursorX || attempts > 20) {
					// Cursor moved = echo arrived, or max 500ms reached
					clearGhostDecoration();
					showGhostDecoration(term, ghost);
				} else {
					suggestionTimer = setTimeout(poll, 25);
				}
			};
			suggestionTimer = setTimeout(poll, 25);
		} else {
			suggestion = null;
			clearGhostDecoration();
		}
	}

	function acceptSuggestion(): boolean {
		if (!suggestion) return false;
		const remaining = suggestion.command.slice(inputBuffer.length);
		if (remaining.length > 0) {
			const encoded = Array.from(new TextEncoder().encode(remaining));
			typed(encoded);
		}
		inputBuffer = '';
		suggestion = null;
		clearGhostDecoration();
		return true;
	}

	function setupInputHandler(term: Terminal): void {
		// Before anything reads onData: on WebKit, xterm drops characters
		// outright when typing is fast, so without this the handler below
		// never sees them (issue #47). No-op on WebView2.
		disposeWebkitInputFix?.();
		disposeWebkitInputFix = installWebkitInputFix(term);

		term.onData((data: string) => {
			data = applyBarModifiers(data);
			// Track input buffer for snippet autocomplete
			if (data === '\r' || data === '\n') {
				inputBuffer = '';
				suggestion = null;
				clearGhostDecoration();
			} else if (data === '\x7f' || data === '\b') {
				inputBuffer = inputBuffer.slice(0, -1);
				updateSuggestion();
			} else if (data === '\x03') {
				inputBuffer = '';
				suggestion = null;
				clearGhostDecoration();
			} else if (data.length === 1 && data.charCodeAt(0) >= 32) {
				inputBuffer += data;
				updateSuggestion();
			} else {
				inputBuffer = '';
				suggestion = null;
				clearGhostDecoration();
			}

			const encoded = Array.from(new TextEncoder().encode(data));
			if (isTyped(data)) typed(encoded);
			else sendData(encoded);
		});

		term.onBinary((data: string) => {
			const bytes = Array.from(data, (c) => c.charCodeAt(0));
			if (isTyped(data)) typed(bytes);
			else sendData(bytes);
		});

		// Ctrl+C copies when text is selected, otherwise sends SIGINT as normal.
		// Ctrl+V pastes from clipboard.
		// Tab accepts snippet suggestion.
		term.attachCustomKeyEventHandler((event: KeyboardEvent) => {
			if (event.type !== 'keydown') return true;

			// Tab accepts snippet autocomplete
			if (event.key === 'Tab' && suggestion) {
				event.preventDefault();
				acceptSuggestion();
				return false;
			}

			// Escape dismisses suggestion
			if (event.key === 'Escape' && suggestion) {
				suggestion = null;
				inputBuffer = '';
				clearGhostDecoration();
				return true;
			}

			// Find in this terminal, in its bar (Ctrl+Shift+F unless changed
			// in Settings > Keyboard shortcuts, as are the keys below).
			if (matchesAction(event, 'terminal.find')) {
				event.preventDefault();
				requestFind(ptyId, findText(term));
				return false;
			}

			// Search history, from this session.
			if (matchesAction(event, 'terminal.history')) {
				event.preventDefault();
				openHistoryPanel({ text: term.getSelection().trim().slice(0, 200), sessionId, sessionName, tabId: ptyId });
				return false;
			}

			// Pick an earlier command to paste, from this session.
			if (matchesAction(event, 'terminal.commandPicker')) {
				event.preventDefault();
				openCommandPicker({ text: findText(term), sessionId, sessionName, tabId: ptyId });
				return false;
			}

			// Copy only while text is selected, so Ctrl+C still interrupts.
			if (term.hasSelection() && matchesAction(event, 'terminal.copy')) {
				event.preventDefault();
				void copySelection(term);
				return false;
			}

			if (matchesAction(event, 'terminal.paste')) {
				event.preventDefault();
				void pasteFromClipboard(term);
				return false;
			}

			return true;
		});
	}

	async function setupEventListeners(term: Terminal): Promise<void> {
		const eventId = remote && currentConnectionId ? currentConnectionId : ptyId;
		const prefix = termType === 'local' ? 'pty' : termType;
		const dataEventName = `${prefix}-data-${eventId}`;
		const exitEventName = `${prefix}-exit-${eventId}`;
		sessionStartedAt = Date.now();

		const taking = untrack(() => handoff);
		if (taking) holding = [];
		unlistenData = await listen<Chunk>(dataEventName, (event) => {
			const payload = event.payload;
			if (holding) {
				holding.push(payload);
				return;
			}
			writeChunk(term, payload);
			recent.push(payload);
			if (recent.length > TAIL_CHUNKS) recent.shift();
		});

		unlistenExit = await listen<{ code?: number | null; error?: string | null } | null>(exitEventName, (event) => {
			if (selfConnecting) {
				const error = event.payload?.error;
				if (!error && closeOnCleanExit()) return;
				cleanEnd = !error;
				const closed = termType === 'serial' ? t('serial.closed') : t('telnet.closed');
				term.write(`\r\n\x1b[90m[${error ? `${closed}: ${error}` : closed}]\x1b[0m\r\n`);
				disconnected = true;
				return;
			}
			// SSH: a code means the shell ended on its own (exit typed); none
			// means the connection dropped. A local shell always ends cleanly.
			const code = event.payload?.code;
			if ((termType !== 'ssh' || typeof code === 'number') && closeOnCleanExit(termType === 'ssh' && code === 0)) return;
			const note = typeof code === 'number' || termType !== 'ssh' ? `Process exited with code ${code ?? 0}` : t('terminal.connection_closed');
			term.write(`\r\n\x1b[90m[${note}]\x1b[0m\r\n`);
			if (termType === 'ssh') {
				cleanEnd = typeof code === 'number';
				disconnected = true;
				// Detached or ended in tmux is a clean end: nothing to go back to.
				if (!cleanEnd) startAutoReconnect();
			}
		});

		await registerHistory(eventId);

		onReady?.();
		if (taking) void takeOver(term, taking);

		// Listeners are attached — tell the SSH backend to flush any buffered
		// output (motd/banner) emitted before this terminal mounted.
		if (termType === 'ssh' && currentConnectionId) {
			sshReady(currentConnectionId).catch(() => {});
		}
	}

	/** When the current session's listeners went up. */
	let sessionStartedAt = 0;

	/** The session ended cleanly: close its tab if Settings say so. One that
	 *  ended within moments of starting more likely failed (a shell that
	 *  would not start, a server that refuses shells): it stays, to be read,
	 *  unless the shell said all was well (`success`: an SSH exit code of 0,
	 *  as Ctrl+D at a fresh prompt gives). */
	function closeOnCleanExit(success = false): boolean {
		if (!getSettings().closeTabOnExit) return false;
		if (!success && Date.now() - sessionStartedAt < 2000) return false;
		closeTab(ptyId);
		return true;
	}

	/**
	 * Open the Telnet connection: listen under a new id first, so nothing the
	 * device sends at once (a banner, a login prompt) is lost, then connect.
	 * Reconnecting is the same; the tab drops the old connection.
	 */
	async function connectTelnet(term: Terminal): Promise<void> {
		if (!telnetConnectParams || reconnecting) return;
		reconnecting = true;
		disconnected = false;
		cleanEnd = false;
		const newId = crypto.randomUUID();
		const { host, port } = telnetConnectParams;
		try {
			unlistenData?.();
			unlistenExit?.();
			currentConnectionId = newId;
			await setupEventListeners(term);
			term.write(`\x1b[90m[${t('telnet.connecting', { host: `${host}:${port}` })}]\x1b[0m\r\n`);
			await telnetConnect(newId, telnetConnectParams, term.cols, term.rows);
			// The tab was closed while connecting: nothing would ever close this.
			if (!terminal) {
				void telnetDisconnect(newId).catch(() => {});
				return;
			}
			onReconnected?.(newId);
		} catch (err) {
			const reason = untrustedRefsFromError(err) ? t('pwmanager.untrusted_reopen') : String(err);
			term.write(`\r\n\x1b[31m[${t('telnet.connect_failed')}: ${reason}]\x1b[0m\r\n`);
			disconnected = true;
		} finally {
			reconnecting = false;
		}
	}

	/**
	 * Open the serial port, listening under a new id first as Telnet does.
	 * Opening it again (reconnect) drops the tab's old connection.
	 */
	async function connectSerial(term: Terminal): Promise<void> {
		if (!serialConnectParams || reconnecting) return;
		reconnecting = true;
		disconnected = false;
		cleanEnd = false;
		const newId = crypto.randomUUID();
		const { port, options } = serialConnectParams;
		try {
			unlistenData?.();
			unlistenExit?.();
			// The old connection holds the port: let it go before opening it again.
			if (currentConnectionId) await serialDisconnect(currentConnectionId).catch(() => {});
			currentConnectionId = newId;
			await setupEventListeners(term);
			term.write(`\x1b[90m[${t('serial.opening', { port, settings: serialLabel(options) })}]\x1b[0m\r\n`);
			await serialConnect(newId, serialConnectParams);
			if (!terminal) {
				void serialDisconnect(newId).catch(() => {});
				return;
			}
			onReconnected?.(newId);
		} catch (err) {
			term.write(`\r\n\x1b[31m[${t('serial.open_failed')}: ${String(err)}]\x1b[0m\r\n`);
			disconnected = true;
		} finally {
			reconnecting = false;
		}
	}

	function sendBreak(): void {
		if (termType === 'serial' && currentConnectionId && !disconnected) {
			serialBreak(currentConnectionId).catch((err) => console.error('[Terminal] Break failed:', err));
		}
	}

	/** Connect again; the error, or null once connected. */
	async function handleReconnect(): Promise<string | null> {
		if (terminal && termType === 'telnet') {
			await connectTelnet(terminal);
			return null;
		}
		if (terminal && termType === 'serial') {
			await connectSerial(terminal);
			return null;
		}
		if (!terminal || !sshConnectParams || reconnecting) return 'busy';
		reconnecting = true;
		disconnected = false;
		cleanEnd = false;

		const term = terminal;
		const newId = crypto.randomUUID();

		try {
			term.write(`\r\n\x1b[33m[${t('terminal.reconnecting')}]\x1b[0m\r\n`);

			try {
				await sshConnect({
					...sshConnectParams,
					id: newId,
					cols: term.cols,
					rows: term.rows,
				});
			} catch (err) {
				// The address it used no longer answers (the laptop left the
				// office): the session's others, as Auto orders them.
				const fb = sshConnectParams.fallback;
				if (!fb || !isUnreachable(String(err))) throw err;
				const usedKey = addressKey(sshConnectParams);
				const rest = fb.addresses.filter((a) => addressKey(a) !== usedKey);
				// Silent mode: in order, nothing sent before an address is tried.
				const order = fb.check === false
					? plainOrder(rest)
					: autoOrder(rest, await sshProbeAddresses(rest.map((a) => (a.routed ? { host: '', port: 0 } : { host: a.host, port: a.port }))).catch(() => rest.map(() => null)));
				let last = err;
				let done = false;
				for (const a of order) {
					term.write(`\x1b[33m[${t('session.trying_address', { name: addressName(a, fb.sessionPort) })}]\x1b[0m\r\n`);
					const own = a.viaSessionId ? fb.viaChains?.[a.viaSessionId] : undefined;
					if (a.viaSessionId && !own) continue;
					const moved = { host: a.host, port: a.port, jumpChain: own ?? (a.routed ? fb.jumpChain : undefined), proxy: a.routed && !own ? fb.proxy : undefined };
					try {
						await sshConnect({ ...sshConnectParams, ...moved, id: newId, cols: term.cols, rows: term.rows });
						// Later reconnects, duplicates and tmux windows go there too.
						Object.assign(sshConnectParams, moved);
						if (sessionId) rememberAddress(sessionId, a);
						done = true;
						break;
					} catch (e) {
						last = e;
						if (!isUnreachable(String(e))) break;
					}
				}
				if (!done) throw last;
			}

			// Tear down old event listeners
			unlistenData?.();
			unlistenExit?.();

			// Switch to new connection
			currentConnectionId = newId;
			await setupEventListeners(term);

			term.write(`\r\n\x1b[32m[${t('terminal.reconnected')}]\x1b[0m\r\n`);
			onReconnected?.(newId);
			return null;
		} catch (err) {
			const reason = untrustedRefsFromError(err) ? t('pwmanager.untrusted_reopen') : String(err);
			term.write(`\r\n\x1b[31m[${t('terminal.reconnect_failed')}: ${reason}]\x1b[0m\r\n`);
			disconnected = true;
			return untrustedRefsFromError(err) ? 'approval needed' : String(err);
		} finally {
			reconnecting = false;
		}
	}

	function setupResizeObserver(term: Terminal, fit: FitAddon, el: HTMLDivElement): void {
		let resizeTimeout: ReturnType<typeof setTimeout> | undefined;

		resizeObserver = new ResizeObserver(() => {
			if (resizeTimeout) clearTimeout(resizeTimeout);
			resizeTimeout = setTimeout(() => safeFitAndResize(term, fit, el), 50);
		});

		resizeObserver.observe(el);
	}

	$effect(() => {
		if (!containerEl) return;

		// The settings are read untracked: as a dependency of this effect, a
		// new font size (Ctrl+wheel, a pinch, A−/A+) threw the terminal away
		// and opened an empty one in its place. Size, font and theme changes
		// reach the open terminal through their own paths instead.
		const term = untrack(createTerminal);
		const fit = new FitAddon();
		loadAddons(term, fit, containerEl);

		// Wait for font to load before opening (canvas needs the font ready)
		loadFont(term.options.fontFamily ?? 'monospace', term.options.fontSize ?? 14).catch(() => {}).finally(() => {
			if (!containerEl) return;
			term.open(containerEl);

			clock = new LineClock(term, term.options.scrollback ?? 0);
			clock.update(Date.now());
			term.onWriteParsed(() => {
				clock?.update(Date.now());
				scheduleGutter();
			});
			term.onRender(scheduleGutter);
			term.onScroll(scheduleGutter);
			term.onResize(scheduleGutter);

			requestAnimationFrame(() => {
				safeFitAndResize(term, fit, containerEl!);
			});

			setupInputHandler(term);
			// A Telnet tab connects from here, once; a terminal mounted again
			// for a tab already connected only listens.
			if (termType === 'telnet' && !currentConnectionId) void connectTelnet(term);
			else if (termType === 'serial' && !currentConnectionId) void connectSerial(term);
			else setupEventListeners(term);
			setupResizeObserver(term, fit, containerEl);

			// Detect user changes (e.g. sudo su -) via OSC 2 terminal title updates
			term.onTitleChange((title: string) => {
				lastTitle = title;
				onTitleChange?.(title);
			});

			// The bell (^G): a sound, a flash, a notification, as set in
			// Settings > Terminal > Bell (#lib/terminal/bell).
			term.onBell(onBell);

			// A right click opens a menu, pastes, or copies the selection:
			// the user's choice in Settings (#lib/terminal/mouse).
			const termEl = containerEl!;
			function onContextMenu(e: MouseEvent) {
				e.preventDefault();
				const action = rightClickAction({
					mode: getSettings().terminalRightClick,
					hasSelection: term.hasSelection(),
					mouseTracking: term.modes.mouseTrackingMode !== 'none',
					shift: e.shiftKey,
					ctrl: e.ctrlKey
				});
				if (action === 'menu') openMenu(term, e.clientX, e.clientY);
				else if (action === 'copy') void copySelection(term);
				else if (action === 'paste') void pasteFromClipboard(term);
			}
			termEl.addEventListener('contextmenu', onContextMenu);

			// Copy on select, for those who want it: when the button that
			// made or changed a selection is let go, which may be outside
			// the terminal. The selection stays on screen.
			function onSelectEnd() {
				window.removeEventListener('mouseup', onSelectEnd, true);
				// xterm settles the selection in its own mouseup handler.
				setTimeout(() => {
					if (getSettings().terminalCopyOnSelect) void copySelection(term, true);
				});
			}
			function onSelectStart(e: MouseEvent) {
				if (e.button !== 0) return;
				window.removeEventListener('mouseup', onSelectEnd, true);
				window.addEventListener('mouseup', onSelectEnd, true);
			}
			termEl.addEventListener('mousedown', onSelectStart, true);
			// Linux: the primary selection gets the text once the button is
			// up, not on every move of the drag (#lib/terminal/selection-sync).
			const undoSelectionSync = deferLinuxSelectionSync(term, { host: termEl, win: window });
			unlistenSelectEnd = () => {
				window.removeEventListener('mouseup', onSelectEnd, true);
				undoSelectionSync();
			};

			// A middle click pastes. On Linux the webview does it itself, with
			// the selection (X11's primary), as a paste event on xterm's
			// textarea, which onNativePaste below takes. Where no such event
			// comes (Windows, macOS, a Wayland session without a primary
			// selection) the clipboard is pasted instead. A program that
			// tracks the mouse gets the click; Shift takes it back.
			function onMiddleDown(e: MouseEvent) {
				if (e.button !== 1) return;
				middleClickAt = performance.now();
				middlePasted = false;
				middleBlocked =
					!getSettings().terminalMiddleClickPaste || (term.modes.mouseTrackingMode !== 'none' && !e.shiftKey);
				// Windows would start its scroll-anywhere mode.
				if (!isWebKit()) e.preventDefault();
			}
			function onMiddleClick(e: MouseEvent) {
				if (e.button !== 1 || middleBlocked) return;
				setTimeout(() => {
					if (!middlePasted) void pasteFromClipboard(term);
				}, 60);
			}
			termEl.addEventListener('mousedown', onMiddleDown, true);
			termEl.addEventListener('auxclick', onMiddleClick);

			// The moments the user typed, for what may be broadcast (isTyped).
			for (const type of ['keydown', 'keypress', 'beforeinput', 'input', 'compositionend']) {
				termEl.addEventListener(type, markUserInput, { capture: true });
			}

			// On macOS, Cmd+V and Edit → Paste arrive as the native paste: the
			// menu item runs the paste: selector and WebKit fires a DOM paste
			// event on the focused element, which is xterm's hidden textarea.
			// xterm would insert the text on its own, skipping the multi-line
			// confirmation, so it is caught first — capture phase, on the
			// textarea itself — and routed through the same path as Ctrl+V.
			function onNativePaste(e: ClipboardEvent) {
				const text = e.clipboardData?.getData('text/plain') ?? '';
				e.preventDefault();
				e.stopImmediatePropagation();
				// The webview's own middle-click paste (Linux): only when the
				// setting allows it and the click was not the program's, and
				// never doubled by the fallback above.
				if (performance.now() - middleClickAt < MIDDLE_CLICK_PASTE_MS) {
					if (middlePasted || middleBlocked) return;
					middlePasted = true;
				}
				pasteText(term, text);
			}
			term.textarea?.addEventListener('paste', onNativePaste, { capture: true });

			// Whether a character is part-way through being composed decides
			// whether the window-move handler below may touch focus at all.
			function onCompositionStart() { composing = true; }
			function onCompositionEnd() { composing = false; }
			term.textarea?.addEventListener('compositionstart', onCompositionStart);
			term.textarea?.addEventListener('compositionend', onCompositionEnd);

			// Moving the window emits a stream of events; the interesting moment
			// is when it stops, which is when the user let go of the title bar.
			getCurrentWindow()
				.onMoved(() => {
					clearTimeout(rearmTimer);
					rearmTimer = setTimeout(rearmIme, REARM_SETTLE_MS);
				})
				.then((fn) => { unlistenMoved = fn; })
				.catch(() => {
					// No Tauri window (browser preview) — nothing to re-arm.
				});
			// Resizing by a bottom or right edge leaves the window where it was,
			// so no move arrives, yet the candidate window is left just as
			// misplaced, and the same click away and back puts it right (#104).
			getCurrentWindow()
				.onResized(() => {
					clearTimeout(rearmTimer);
					rearmTimer = setTimeout(rearmIme, REARM_SETTLE_MS);
				})
				.then((fn) => { unlistenResized = fn; })
				.catch(() => {});

			// Ctrl+Wheel zooms terminal font size
			function onWheel(e: WheelEvent) {
				if (!e.ctrlKey) return;
				e.preventDefault();
				zoomBy(e.deltaY < 0 ? 1 : -1);
			}
			termEl.addEventListener('wheel', onWheel, { passive: false });

			// So does a pinch, on a phone with no wheel (#127): a step each
			// time the fingers' distance grows or shrinks by a tenth. Two
			// fingers are taken from the page, which would zoom as a whole.
			let pinchFrom = 0;
			const spread = (e: TouchEvent): number =>
				Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
			termEl.addEventListener('touchstart', (e) => {
				if (e.touches.length === 2) pinchFrom = spread(e);
			}, { passive: true });
			termEl.addEventListener('touchmove', (e) => {
				if (e.touches.length !== 2 || !pinchFrom) return;
				e.preventDefault();
				const ratio = spread(e) / pinchFrom;
				if (ratio > 1.1 || ratio < 1 / 1.1) {
					zoomBy(ratio > 1 ? 1 : -1);
					pinchFrom = spread(e);
				}
			}, { passive: false });
			termEl.addEventListener('touchend', (e) => {
				if (e.touches.length < 2) pinchFrom = 0;
			}, { passive: true });

			// Listen for menu-bar Copy/Paste events (Rust emits these when the
			// user clicks Edit > Copy / Paste since the menu items have no
			// keyboard accelerators — Cmd+C / Cmd+V are handled by xterm's
			// attachCustomKeyEventHandler below).
			listen('menu-copy', () => {
				if (term.hasSelection()) void copySelection(term);
			}).then((fn) => { unlistenMenuCopy = fn; });
			listen('menu-paste', () => {
				void pasteFromClipboard(term);
			}).then((fn) => { unlistenMenuPaste = fn; });

			// Note: selection does not copy unless "copy on select" is on in
			// Settings, so drag-selecting never clobbers the clipboard by
			// surprise. Copy is otherwise explicit: Ctrl+C with a selection,
			// or the right-click menu.

			terminal = term;
			fitAddon = fit;

			// Where a snippet types in: this terminal's own channel, and whether
			// its shell takes a bracketed paste (#lib/terminal/snippet).
			snippetInput = {
				bracketed: () => term.modes.bracketedPasteMode,
				send: sendData,
				type: typed
			};
			registerTerminalInput(ptyId, snippetInput);

			stopHandoff = registerHandoff(ptyId, handOver);

			const bufferId = termType === 'ssh' && currentConnectionId ? currentConnectionId : ptyId;
			registerBufferReader(bufferId, {
				size: () => ({ cols: term.cols, rows: term.rows }),
				read: (startLine?: number, maxLines?: number) => {
					const buf = term.buffer.active;
					const start = startLine ?? Math.max(0, buf.length - (maxLines ?? 50));
					const end = maxLines && startLine != null ? Math.min(buf.length, start + maxLines) : buf.length;
					const lines: string[] = [];
					for (let i = start; i < end; i++) {
						const line = buf.getLine(i);
						if (line) lines.push(line.translateToString(true));
					}
					return lines.join('\n').trim();
				},
				lineCount: () => term.buffer.active.length
			});
		});

		return () => {
			stopHandoff?.();
			stopHandoff = undefined;
			const bufferId = termType === 'ssh' && currentConnectionId ? currentConnectionId : ptyId;
			unregisterBufferReader(bufferId);
			if (snippetInput) unregisterTerminalInput(ptyId, snippetInput);
			unlistenData?.();
			unlistenExit?.();
			unlistenMenuCopy?.();
			unlistenMenuPaste?.();
			unlistenMoved?.();
			unlistenResized?.();
			unlistenSelectEnd?.();
			disposeWebkitInputFix?.();
			disposeWebkitInputFix = undefined;
			clearTimeout(rearmTimer);
			resizeObserver?.disconnect();
			unregisterFinder?.();
			unregisterFinder = undefined;
			unregisterCapture?.();
			unregisterCapture = undefined;
			if (gutterFrame) cancelAnimationFrame(gutterFrame);
			gutterFrame = 0;
			clock?.dispose();
			clock = undefined;
			highlighter?.dispose();
			highlighter = undefined;
			term.dispose();
			terminal = undefined;
			fitAddon = undefined;
		};
	});

	// Live font family updates from settings
	const appSettings = getSettings();
	$effect(() => {
		const term = terminal;
		const fit = fitAddon;
		const el = containerEl;
		if (!term || !fit || !el || !appSettings.fontFamily) return;
		const family = terminalFont(appSettings.fontFamily);
		if (term.options.fontFamily === family) return;

		loadFont(family, term.options.fontSize ?? 14).catch(() => {}).finally(() => {
			term.options.fontFamily = family;
			term.clearTextureAtlas();
			safeFitAndResize(term, fit, el);
		});
	});

	// And the size, as one terminal's zoom (or a synced vault) sets it for
	// all; the terminal zoomed has it already. A hidden tab is fitted when
	// it is shown again.
	$effect(() => {
		const size = appSettings.fontSize;
		const term = terminal;
		const fit = fitAddon;
		const el = containerEl;
		if (!term || !fit || !el || !size || term.options.fontSize === size) return;
		term.options.fontSize = size;
		term.clearTextureAtlas();
		safeFitAndResize(term, fit, el);
	});

	// Where a double click's word ends, as Settings has it.
	$effect(() => {
		const separator = wordSeparator(appSettings.terminalWordSeparators);
		if (terminal && terminal.options.wordSeparator !== separator) terminal.options.wordSeparator = separator;
	});

	// The scrollback follows Settings; xterm.js keeps what fits the new size.
	$effect(() => {
		const lines = scrollbackLines(appSettings.terminalScrollback, onPhone);
		clock?.setKeep(lines);
		if (terminal && terminal.options.scrollback !== lines) terminal.options.scrollback = lines;
	});

	// ---- Line numbers and timestamps (#lib/terminal/gutter) -----------------
	// Times are taken for every line from the start, so turning the gutter on
	// later still shows when earlier lines came.
	let clock: LineClock | undefined;
	let gutterNumbers = $derived(appSettings.terminalLineNumbers);
	let gutterTimes = $derived(appSettings.terminalTimestamps);
	let gutterOn = $derived(gutterNumbers || gutterTimes);
	let gutterRows = $state<{ num: string; time: string }[]>([]);
	let gutterRowHeight = $state(17);
	let gutterTop = $state(8);
	let gutterFontSize = $state(14);
	let gutterNumWidth = $state(5);
	let gutterFrame = 0;

	function scheduleGutter(): void {
		if (!gutterOn || gutterFrame) return;
		gutterFrame = requestAnimationFrame(() => {
			gutterFrame = 0;
			drawGutter();
		});
	}

	function drawGutter(): void {
		const term = terminal;
		if (!term || !clock || !containerEl) return;
		const screen = term.element?.querySelector<HTMLElement>('.xterm-screen');
		if (screen && term.rows > 0) {
			const box = screen.getBoundingClientRect();
			if (box.height > 0) {
				gutterRowHeight = box.height / term.rows;
				gutterTop = box.top - containerEl.getBoundingClientRect().top;
			}
		}
		gutterFontSize = term.options.fontSize ?? 14;
		gutterNumWidth = numberWidth(clock.lastLine);
		gutterRows = clock.rows().map((r) => ({
			num: r ? String(r.line) : '',
			time: r && r.time !== undefined ? gutterTime(r.time) : ''
		}));
	}

	$effect(() => {
		if (gutterOn && terminal) scheduleGutter();
	});

	// Auto-focus xterm AND re-fit when this tab becomes active (Ctrl+Tab, click, sidebar switch).
	// Focus: xterm needs an explicit `focus()` call — toggling `display: none` → `display: block`
	// doesn't make the inner textarea capture keystrokes on its own. Three focus timings
	// defeat races with click handlers and Tauri WebView2 layout-not-ready quirks.
	// Re-fit: while the tab was hidden, the container measured 0×0 so the ResizeObserver
	// could not track window/sidebar resizes. We re-fit on the next frame (after layout)
	// instead of waiting for the observer's 50ms debounce — keeps the switch feeling instant.
	// In a split both panes are shown and fitted; only the active one takes
	// the keyboard, so clicking the other pane is what moves it.
	$effect(() => {
		if (!shown || !terminal) return;
		const term = terminal;
		const fit = fitAddon;
		const el = containerEl;
		const takeKeys = active;
		const focusNow = () => {
			if (!takeKeys) return;
			try {
				term.focus();
			} catch {
				// Terminal may have been disposed mid-frame
			}
		};
		focusNow();
		requestAnimationFrame(() => {
			if (fit && el) safeFitAndResize(term, fit, el);
			focusNow();
			queueMicrotask(focusNow);
		});
	});

	// The tab shows whether this SSH or Telnet session is still connected.
	$effect(() => {
		if (!remote) return;
		setTabLink(ptyId, reconnecting ? 'reconnecting' : disconnected ? 'down' : undefined);
	});

	// The command palette typed into this terminal: it takes the keyboard back.
	$effect(() => {
		if (terminalFocusRequests() === 0 || !active || !shown) return;
		untrack(() => terminal?.focus());
	});

	let lastTitle = '';
	let lastNotified = 0;
	let flashing = $state(false);
	let flashTimer: ReturnType<typeof setTimeout> | undefined;

	function onBell(): void {
		const s = getSettings();
		const now = Date.now();
		const act = bellActions(
			{ sound: s.terminalBellSound, flash: s.terminalBellFlash, notify: s.terminalBellNotify },
			{ seen: !!active && shown && windowSeen(), lastNotified, lastSound: lastBellSound(), now, soundsOn: s.playSounds }
		);
		if (act.sound) playBell(now);
		if (act.flash && !flashing) {
			flashing = true;
			clearTimeout(flashTimer);
			flashTimer = setTimeout(() => (flashing = false), 150);
		}
		if (act.notify) {
			lastNotified = now;
			const name = sessionName || lastTitle || t('terminal.bell_terminal');
			void notifyBell('Reacher CM', t('terminal.bell_notify', { name }));
		}
	}

</script>

<div
	class="terminal-wrapper"
	style:display={shown ? 'flex' : 'none'}
	style:--term-bg={palette.background}
>
	<div class="terminal-row">
		{#if gutterOn}
			<!-- Line numbers and times, row for row with the terminal's screen. -->
			<div
				class="terminal-gutter"
				aria-hidden="true"
				style:padding-top="{gutterTop}px"
				style:font-family={terminalFont(appSettings.fontFamily)}
				style:font-size="{Math.max(8, gutterFontSize - 2)}px"
				style:--gutter-row="{gutterRowHeight}px"
				style:color={palette.foreground}
			>
				{#each gutterRows as row, i (i)}
					<div class="gutter-row">
						{#if gutterNumbers}<span class="gutter-num" style:width="{gutterNumWidth}ch">{row.num}</span>{/if}
						{#if gutterTimes}<span class="gutter-time">{row.time}</span>{/if}
					</div>
				{/each}
			</div>
		{/if}
		<div bind:this={containerEl} class="terminal-container" class:bell-flash={flashing}></div>
	</div>
	{#if onPhone && !disconnected}
		<MobileKeyBar bind:ctrl={barCtrl} bind:alt={barAlt} zoom onkey={onBarKey} />
	{/if}
	{#if remote}
		<ZmodemBar
			connectionId={currentConnectionId}
			tabId={ptyId}
			send={(data) => {
				if (!currentConnectionId || disconnected) return false;
				sendData(data);
				return true;
			}}
		/>
	{/if}
	{#if disconnected && remote}
		<div class="reconnect-overlay">
			<div class="reconnect-card">
				<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
					<path d="M2.5 2v6h6" />
					<path d="M2.5 8a10 10 0 0 1 17.13-4" />
					<path d="M21.5 22v-6h-6" />
					<path d="M21.5 16a10 10 0 0 1-17.13 4" />
				</svg>
				<span class="reconnect-text">{t(cleanEnd ? 'terminal.session_ended' : 'terminal.connection_lost')}</span>
				{#if autoReconnecting}
					<span class="reconnect-auto">{t('tmux.auto_reconnecting')}</span>
				{/if}
				<button class="reconnect-btn" onclick={() => (autoReconnecting ? autoReconnectNow() : handleReconnect())} disabled={reconnecting}>
					{reconnecting ? t('terminal.reconnecting') : t('terminal.reconnect')}
				</button>
				{#if autoReconnecting}
					<button class="reconnect-cancel" onclick={stopAutoReconnect} type="button">{t('common.cancel')}</button>
				{/if}
				{#if cleanEnd}
					<!-- The shell ended on its own: closing is the likely next
					     step, and a phone has no quick way to close a tab. -->
					<button class="reconnect-close" onclick={() => closeTab(ptyId)} type="button">{t('tabs.close_tab')}</button>
					<label class="reconnect-always">
						<input
							type="checkbox"
							checked={getSettings().closeTabOnExit}
							onchange={(e) => updateSetting('closeTabOnExit', e.currentTarget.checked)}
						/>
						<span>{t('terminal.always_close_tab')}</span>
					</label>
				{/if}
			</div>
		</div>
	{/if}
</div>

{#if menu}
	<DbMenu x={menu.x} y={menu.y} items={menu.items} onclose={closeMenu} />
{/if}

<Modal
	open={pastePreview !== null}
	onclose={cancelPaste}
	title={t('terminal.paste_confirm_title')}
	maxWidth="560px"
>
	<p class="paste-msg">{t('terminal.paste_confirm_message', { lines: String(pasteLineCount) })}</p>
	{#if broadcastCount > 0}
		<p class="paste-msg paste-broadcast">{t('broadcast.paste_note', { count: String(broadcastCount) })}</p>
	{/if}
	<pre class="paste-preview">{pastePreview}</pre>

	<label class="paste-dont-ask">
		<input type="checkbox" bind:checked={dontAskAgain} />
		<span>{t('terminal.paste_dont_ask')}</span>
	</label>

	{#snippet actions()}
		<Button variant="secondary" onclick={cancelPaste}>{t('common.cancel')}</Button>
		<Button variant="primary" onclick={confirmPaste}>{t('terminal.paste_confirm_action')}</Button>
	{/snippet}
</Modal>

<style>
	/* The visual bell: a short wash over the terminal, not an inverted
	   screen, which is hard on the eyes in a dark room. */
	.terminal-container.bell-flash {
		position: relative;
	}

	.terminal-container.bell-flash::after {
		content: '';
		position: absolute;
		inset: 0;
		z-index: 10;
		pointer-events: none;
		box-shadow: inset 0 0 0 2px var(--color-accent), inset 0 0 48px color-mix(in srgb, var(--color-accent) 35%, transparent);
	}

	.paste-dont-ask {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-top: 12px;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
		cursor: pointer;
		user-select: none;
	}

	.paste-dont-ask input {
		accent-color: var(--color-accent);
		margin: 0;
		cursor: pointer;
	}

	.paste-msg {
		margin: 0 0 10px;
		font-size: 0.875rem;
		line-height: 1.5;
		color: var(--color-text-primary);
	}

	.paste-broadcast {
		color: var(--color-warning, #f59e0b);
		font-weight: 600;
	}

	.paste-preview {
		margin: 0;
		max-height: 220px;
		overflow: auto;
		padding: 10px 12px;
		font-family: var(--font-mono, monospace);
		font-size: 0.75rem;
		line-height: 1.5;
		white-space: pre;
		color: var(--color-text-secondary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
	}

	.terminal-wrapper {
		flex-direction: column;
		width: 100%;
		height: 100%;
		background: var(--bg-primary, var(--color-bg-primary));
		position: relative;
	}

	/* Takes what the key bar leaves, on a phone; all of it elsewhere. */
	.terminal-row {
		display: flex;
		width: 100%;
		flex: 1;
		min-height: 0;
	}

	.terminal-container {
		flex: 1;
		min-width: 0;
		height: 100%;
	}

	/* Line numbers and times: dim, out of the way of a selection, and as wide
	   whatever they show, so the terminal is not refitted as they change. */
	.terminal-gutter {
		flex: none;
		height: 100%;
		overflow: hidden;
		box-sizing: border-box;
		padding-left: 10px;
		background: var(--term-bg);
		opacity: 0.45;
		user-select: none;
		pointer-events: none;
		font-variant-numeric: tabular-nums;
		border-right: 1px solid color-mix(in srgb, currentColor 25%, transparent);
	}

	.gutter-row {
		display: flex;
		gap: 1ch;
		height: var(--gutter-row);
		line-height: var(--gutter-row);
		padding-right: 6px;
		white-space: pre;
	}

	.gutter-num {
		display: inline-block;
		text-align: right;
	}

	.gutter-time {
		display: inline-block;
		width: 8ch;
	}

	@media (max-width: 640px) {
		.terminal-gutter {
			padding-left: 4px;
		}
	}


	/* Painted with the terminal's own background: the padding and the strip
	   left below the last whole row showed xterm.css's black viewport, a
	   black frame around a light theme (#105). */
	.terminal-container {
		background: var(--term-bg);
	}

	.terminal-container :global(.xterm) {
		height: 100%;
		/* Room between the text and the sidebar and window edges, as a terminal
		   app gives it; the fit addon takes the padding off the grid it sizes. */
		padding: 8px 12px;
	}

	/* A phone keeps every column it can. */
	@media (max-width: 640px) {
		.terminal-container :global(.xterm) {
			padding: 4px;
		}
	}

	.terminal-container :global(.xterm-viewport) {
		background-color: var(--term-bg);
	}

	.terminal-container :global(.xterm-viewport::-webkit-scrollbar) {
		width: 6px;
	}

	.terminal-container :global(.xterm-viewport::-webkit-scrollbar-track) {
		background: transparent;
	}

	.terminal-container :global(.xterm-viewport::-webkit-scrollbar-thumb) {
		background-color: var(--color-surface-strong);
		border-radius: 3px;
	}

	.terminal-container :global(.xterm-viewport::-webkit-scrollbar-thumb:hover) {
		background-color: var(--color-surface-strong);
	}

	.reconnect-overlay {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-surface-sunken);
		backdrop-filter: blur(4px);
		z-index: 20;
	}

	.reconnect-card {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 12px;
		padding: 24px 32px;
		border-radius: 12px;
		background: rgba(30, 30, 30, 0.95);
		border: 1px solid var(--color-border);
		color: var(--color-text-secondary, #a1a1a6);
	}

	.reconnect-card svg {
		opacity: 0.6;
	}

	.reconnect-text {
		font-size: 0.85rem;
		font-weight: 500;
	}

	.reconnect-auto {
		font-size: 0.8rem;
		color: var(--color-text-secondary);
	}

	.reconnect-cancel {
		padding: 4px 12px;
		border: none;
		background: none;
		color: var(--color-text-secondary);
		font-size: 0.8rem;
		cursor: pointer;
	}

	.reconnect-cancel:hover {
		color: var(--color-text-primary);
	}

	.reconnect-close {
		padding: 6px 20px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: none;
		color: inherit;
		font-size: 0.8rem;
		cursor: pointer;
	}

	.reconnect-close:hover {
		background: rgba(255, 255, 255, 0.08);
	}

	.reconnect-always {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.reconnect-btn {
		padding: 6px 20px;
		border-radius: 6px;
		border: none;
		background: var(--color-accent, var(--color-accent));
		color: #fff;
		font-size: 0.8rem;
		font-weight: 500;
		cursor: pointer;
		transition: opacity 0.15s;
	}

	.reconnect-btn:hover:not(:disabled) {
		opacity: 0.85;
	}

	.reconnect-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
</style>
