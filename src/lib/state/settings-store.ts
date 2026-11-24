/**
 * The local settings as stored: their shape, their defaults, and how a stored
 * copy is read back. No Svelte here, so a Node test can check that every
 * setting survives a save and a restart (scripts/settings-persist-test.mjs).
 */

import { isMobile } from '#lib/platform.js';
import { DEFAULT_PASTE_THRESHOLD } from '#lib/terminal/paste.js';
import { DEFAULT_SCROLLBACK, scrollbackSetting } from '#lib/terminal/scrollback.js';
import { DEFAULT_RIGHT_CLICK, DEFAULT_WORD_SEPARATORS, rightClickMode, type RightClickMode } from '#lib/terminal/mouse.js';
import {
	cursorStyle,
	inactiveCursorStyle,
	cursorWidth,
	DEFAULT_CURSOR_STYLE,
	DEFAULT_CURSOR_BLINK,
	DEFAULT_INACTIVE_CURSOR,
	DEFAULT_CURSOR_WIDTH,
	type CursorStyle,
	type InactiveCursorStyle
} from '#lib/terminal/cursor.js';
import { readOverrides, type KeyOverrides } from '#lib/state/keybindings.js';
import type { WinboxPasswordMode } from '#lib/ipc/winbox.js';
import { DEFAULT_HIGHLIGHT, DEFAULT_HIGHLIGHT_KINDS, highlightKinds, type HighlightKind } from '#lib/terminal/highlight.js';

/** The localStorage key. Kept from Reach, so an update finds what was saved. */
export const STORAGE_KEY = 'reach-settings';

export interface Settings {
	theme: 'dark' | 'light' | 'system';
	/** Id of an installed theme. Overrides `theme` when set. */
	themeId?: string;
	/** With `theme: 'system'` and no `themeId`: the theme used while the OS is
	 *  light, and the one while it is dark. Unset: Reacher CM Light / Dark. */
	themeLight?: string;
	themeDark?: string;
	/** The terminal colours, by terminal theme id (#lib/state/terminal-themes);
	 *  empty: the app theme's own. A session can name its own instead. */
	terminalThemeId: string;
	/** The terminal cursor's shape, unless the session chooses its own
	 *  (see #lib/terminal/cursor). */
	terminalCursorStyle: CursorStyle;
	/** The cursor blinks, unless the session chooses otherwise. */
	terminalCursorBlink: boolean;
	/** The cursor in a terminal without the keyboard. */
	terminalCursorInactive: InactiveCursorStyle;
	/** A bar cursor's thickness, in CSS pixels. */
	terminalCursorWidth: number;
	/** Lines kept above the screen; 0 is Unlimited (#lib/terminal/scrollback). */
	terminalScrollback: number;
	/** Line numbers in a gutter left of the terminal (#lib/terminal/gutter). */
	terminalLineNumbers: boolean;
	/** When each line was written, in the same gutter. */
	terminalTimestamps: boolean;
	fontSize: number;
	/** The whole interface's size on a desktop, as a browser's zoom: 1 is 100%.
	 *  See #lib/state/ui-zoom. */
	uiZoom: number;
	/** Tighter rows (a session on one line): space over air. Text stays its size. */
	compactLayout: boolean;
	/** The bar over each terminal: always, when the pointer is over it, or never. */
	terminalBar: 'always' | 'hover' | 'hidden';
	/** The extra-keys bar above a phone's keyboard, in order (see
	 *  #lib/terminal/bar-keys); null is the built-in set. */
	mobileBarKeys: string[] | null;
	/** Sessions in the list open on a double click; a single click only
	 *  focuses the row. Off: one click connects, as before. */
	doubleClickToOpen: boolean;
	/** A session that ends on its own (exit typed, the shell finished) closes
	 *  its tab rather than offering Reconnect. A dropped connection still does. */
	closeTabOnExit: boolean;
	/** Floating session windows open when the app closed open again at the
	 *  next start, where they were, with their saved sessions. */
	reopenFloatWindows: boolean;
	/** Server monitoring (CPU, memory, disk, users under an SSH or WSL
	 *  terminal), unless the session chooses otherwise. */
	serverMonitoring: boolean;
	/** Before connecting a session with several addresses, check which of
	 *  them answer (a plain TCP connect to each), unless the session says
	 *  otherwise. Off: silent, they are tried in order without checking. */
	checkAddresses: boolean;
	/** Look for a new release at launch and every 45 minutes. Off: only
	 *  Settings > General > Check now asks, so nothing goes out on its own. */
	checkForUpdates: boolean;
	/** The chime when a notification (toast) appears. Off by default on phones
	 *  and tablets, where the system's own sounds already say enough. */
	playSounds: boolean;
	/** The terminal bell (^G) plays a short sound (and Play sounds is on).
	 *  On by default on desktop. See #lib/terminal/bell. */
	terminalBellSound: boolean;
	/** The bell flashes the terminal. On by default on phones and tablets. */
	terminalBellFlash: boolean;
	/** The bell sends a system notification when its tab is not in view or
	 *  the app is in the background. Off by default: it asks permission. */
	terminalBellNotify: boolean;
	/** The speaker button in the top bar: silences everything the app plays
	 *  (the chime, the bell, a remote desktop's sound) until turned off,
	 *  whatever Play sounds and the bell's settings say. */
	muted: boolean;
	fontFamily: string;
	defaultShell: string;
	openLastSession: boolean;
	locale: string;
	minimizeToTray: boolean;
	startWithSystem: boolean;
	/** Type a colour and prompt setup into the remote shell after login. Off
	 *  unless the user turns it on: it is typed, so the shell records it in
	 *  its history, and servers colour themselves for an xterm-256color
	 *  terminal anyway, as they do for PuTTY. Saved under a new name so the
	 *  old default of "on" is not carried over. */
	typeShellSetup: boolean;
	/** Keep the server's login message (MOTD) on screen after connecting. */
	showLoginMessage: boolean;
	/** Write SSH sessions to a text file, as PuTTY's Logging panel does.
	 *  Off unless turned on: a log holds whatever the server printed. */
	sessionLogMode: 'off' | 'printable' | 'all';
	/** Folder for session logs; empty means the default (Documents/Reach Logs). */
	sessionLogFolder: string;
	/** File name with PuTTY's placeholders: &H host, &P port, &Y &M &D date, &T time. */
	sessionLogName: string;
	/** Add to an existing file rather than replace it. */
	sessionLogAppend: boolean;
	/** Start each log with a date line. */
	sessionLogHeader: boolean;
	/** Session history: what terminal tabs record into the encrypted,
	 *  searchable history, unless their session chooses otherwise. Off
	 *  unless turned on, like the file log. */
	historyMode: 'off' | 'commands' | 'output';
	/** Output older than this many days is deleted (0: kept). */
	historyOutputDays: number;
	/** Commands older than this many days are deleted (0: kept). */
	historyCommandDays: number;
	/** The oldest output goes once history takes more than this (0: no cap). */
	historyMaxMb: number;
	/** The user's own patterns to mask before saving, one regex per line. */
	historyPatterns: string;
	/** Remote desktop: paint through WebGL (the GPU) rather than the 2D canvas. */
	rdpHardwareRendering: boolean;
	freerdpPath: string;
	/** Remote desktop: advertise the graphics pipeline, for hosts that encode H.264. */
	rdpGraphicsPipeline: boolean;
	/** Let the graphics pipeline use AVC444 and RemoteFX progressive. */
	rdpNewGraphicsFormats: boolean;
	/** Remote desktop: show the Full screen button over the desktop. */
	rdpFullscreenButton: boolean;
	/**
	 * Where the Full screen button was dragged to, as fractions of the room
	 * it has across and down; null for its place in the top right corner.
	 */
	rdpFullscreenButtonAt: { x: number; y: number } | null;
	/** Remote desktop: keep the full-screen bar shown instead of tucking it away. */
	rdpBarPinned: boolean;

	/**
	 * Confirm a multi-line paste before it reaches the shell.
	 *
	 * A newline runs what precedes it the moment it arrives, so a stray copy
	 * can execute on a production host before anyone has read it. On by
	 * default; off for people who paste blocks all day and have already
	 * answered the question.
	 */
	warnOnMultilinePaste: boolean;
	/** Warn once a paste reaches this many lines. See #lib/terminal/paste. */
	multilinePasteThreshold: number;

	/** What a right click in the terminal does. See #lib/terminal/mouse. */
	terminalRightClick: RightClickMode;
	/** Selecting text in the terminal copies it, as in PuTTY. */
	terminalCopyOnSelect: boolean;
	/** A program on the server may put text on the clipboard (OSC 52:
	 *  tmux copy mode, vim). Never read. See #lib/terminal/osc52. */
	terminalOsc52: boolean;
	/** On a tmux tab, Ctrl+T and + open a new tmux window in a new tab. */
	tmuxNewTabWindow: boolean;
	/** Connecting to a tmux session opens the last used window, or every
	 *  window in a tab of its own. See #lib/terminal/tmux-tabs. */
	tmuxConnectWindows: 'last' | 'all';
	/** tmux's status line off in tmux tabs. */
	tmuxHideStatus: boolean;
	/** A middle click in the terminal pastes. */
	terminalMiddleClickPaste: boolean;
	/** The characters that end a word on a double click, besides the space. */
	terminalWordSeparators: string;
	/** Errors, warnings and addresses in the terminal shown in colour. */
	terminalHighlight: boolean;
	/** Which of them. See #lib/terminal/highlight. */
	terminalHighlightKinds: HighlightKind[];
	/** The user's own patterns, one regular expression per line. */
	terminalHighlightCustom: string;
	/** The program the full-pane file manager's Edit (F4) opens files in:
	 *  '' for the system's default, a detected editor's id, or 'custom'. */
	fileEditor: string;
	/** With 'custom': the command, the file added at the end. */
	fileEditorCommand: string;
	/** With 'custom': it runs in a terminal tab (vim, nano...). */
	fileEditorTerminal: boolean;
	/** WinBox 4 sessions: the program; '' to look for it. */
	winboxPath: string;
	/** How a saved password reaches WinBox. See winbox_commands.rs. */
	winboxPassword: WinboxPasswordMode;

	setupComplete: boolean;
	pendingTursoOrg: string;
	/**
	 * Held in memory only, between the setup wizard and the Sync tab, which
	 * moves it into the encrypted vault. Never written to localStorage: a
	 * token there sits on disk in the clear.
	 */
	pendingTursoApiToken: string;

	/**
	 * State for the one-time nudge towards the Discord community.
	 *
	 * Three fields rather than one boolean, because a single "dismissed" flag
	 * can only express "never ask" or "ask on every single launch", and both
	 * are wrong. `launches` holds it back until someone has actually used the
	 * app, `communityPromptLastShown` turns "Not now" into a real deferral,
	 * and `communityPromptDismissed` is the checkbox that ends it for good.
	 */
	launches: number;
	communityPromptDismissed: boolean;
	/** Epoch ms of the last time the prompt was shown. 0 means never. */
	communityPromptLastShown: number;

	/**
	 * Ids from `#lib/data/whats-new.js` this user has already been shown.
	 *
	 * A fresh install starts with all of them: someone meeting Reach for the
	 * first time should not be told which three corners of it are recent.
	 */
	seenFeatures: string[];

	/**
	 * DevOps workspaces switched on, by id from `#lib/state/devops`.
	 *
	 * Per device rather than synced: the tools themselves are installed per
	 * machine, and a laptop without Ansible has no use for its workspace.
	 */
	devopsTools: string[];

	/** Lock after this many minutes without keyboard or mouse use; 0 never. */
	autoLockMinutes: number;
	/** Lock when the computer's own session locks or goes to sleep. */
	lockOnSystemLock: boolean;
	/** Ask for the passphrases of the saved credentials' keys once the vault
	 *  opens, and keep them in memory until it locks or the app quits. */
	unlockKeysAtStart: boolean;
	/** Phones: remember the passphrases typed at start behind Face ID or the
	 *  fingerprint, so the next start asks for the biometric once. */
	unlockKeysWithBiometrics: boolean;
	/** The user's own keyboard shortcuts, by action; only the ones changed
	 *  from the defaults. See #lib/state/keybindings. */
	keyboardShortcuts: KeyOverrides;
}

/** The file name a log gets unless Settings say otherwise: session name, host, date, time. */
export const DEFAULT_LOG_NAME = '&S-&H-&Y&M&D-&T.log';
/** The default before `&S` existed; a setting still holding it moves to the new one. */
const OLD_DEFAULT_LOG_NAME = '&H-&Y&M&D-&T.log';

export const defaults: Settings = {
	theme: 'dark',
	themeId: undefined,
	themeLight: undefined,
	themeDark: undefined,
	terminalThemeId: '',
	terminalCursorStyle: DEFAULT_CURSOR_STYLE,
	terminalCursorBlink: DEFAULT_CURSOR_BLINK,
	terminalCursorInactive: DEFAULT_INACTIVE_CURSOR,
	terminalCursorWidth: DEFAULT_CURSOR_WIDTH,
	terminalScrollback: DEFAULT_SCROLLBACK,
	terminalLineNumbers: false,
	terminalTimestamps: false,
	fontSize: 14,
	uiZoom: 1,
	compactLayout: false,
	terminalBar: 'always',
	mobileBarKeys: null,
	doubleClickToOpen: false,
	closeTabOnExit: false,
	reopenFloatWindows: false,
	serverMonitoring: true,
	checkAddresses: true,
	checkForUpdates: true,
	playSounds: !isMobile(),
	terminalBellSound: !isMobile(),
	terminalBellFlash: isMobile(),
	terminalBellNotify: false,
	muted: false,
	fontFamily: 'monospace',
	defaultShell: '/bin/bash',
	openLastSession: false,
	locale: 'en',
	minimizeToTray: false,
	startWithSystem: false,
	typeShellSetup: false,
	showLoginMessage: true,
	sessionLogMode: 'off',
	sessionLogFolder: '',
	sessionLogName: DEFAULT_LOG_NAME,
	sessionLogAppend: true,
	sessionLogHeader: true,
	historyMode: 'off',
	historyOutputDays: 90,
	historyCommandDays: 365,
	historyMaxMb: 2048,
	historyPatterns: '',
	rdpHardwareRendering: true,
	freerdpPath: '',
	rdpGraphicsPipeline: false,
	rdpNewGraphicsFormats: true,
	rdpFullscreenButton: true,
	rdpFullscreenButtonAt: null,
	rdpBarPinned: false,
	warnOnMultilinePaste: true,
	multilinePasteThreshold: DEFAULT_PASTE_THRESHOLD,
	terminalRightClick: DEFAULT_RIGHT_CLICK,
	terminalCopyOnSelect: false,
	terminalOsc52: false,
	tmuxNewTabWindow: true,
	tmuxConnectWindows: 'last',
	tmuxHideStatus: false,
	terminalMiddleClickPaste: true,
	terminalWordSeparators: DEFAULT_WORD_SEPARATORS,
	terminalHighlight: DEFAULT_HIGHLIGHT,
	terminalHighlightKinds: [...DEFAULT_HIGHLIGHT_KINDS],
	terminalHighlightCustom: '',
	fileEditor: '',
	fileEditorCommand: '',
	fileEditorTerminal: false,
	winboxPath: '',
	winboxPassword: 'none',
	setupComplete: false,
	pendingTursoOrg: '',
	pendingTursoApiToken: '',
	launches: 0,
	communityPromptDismissed: false,
	communityPromptLastShown: 0,
	seenFeatures: [],
	devopsTools: [],
	autoLockMinutes: 0,
	lockOnSystemLock: false,
	unlockKeysAtStart: false,
	unlockKeysWithBiometrics: false,
	keyboardShortcuts: {}
};

/** Settings held in memory only: never written, so never read back. */
export const MEMORY_ONLY: readonly (keyof Settings)[] = ['pendingTursoApiToken'];

/** What is written to localStorage. */
export function storeSettings(settings: Settings): string {
	return JSON.stringify({ ...settings, pendingTursoApiToken: undefined });
}

/**
 * A stored copy read back: each setting as saved, or its default when it is
 * missing (saved by an older version) or no longer valid. Every setting
 * must be read here, or it is lost at the next start.
 */
export function readStoredSettings(parsed: Partial<Settings>): Settings {
	const settings: Settings = { ...defaults };
	settings.theme = parsed.theme ?? defaults.theme;
	// An installed or built-in theme other than dark and light. It was
	// saved but never read back, so the app fell back to dark or light
	// on every start.
	settings.themeId = typeof parsed.themeId === 'string' && parsed.themeId ? parsed.themeId : undefined;
	settings.themeLight = typeof parsed.themeLight === 'string' && parsed.themeLight ? parsed.themeLight : undefined;
	settings.themeDark = typeof parsed.themeDark === 'string' && parsed.themeDark ? parsed.themeDark : undefined;
	settings.terminalThemeId = parsed.terminalThemeId ?? defaults.terminalThemeId;
	settings.terminalCursorStyle = cursorStyle(parsed.terminalCursorStyle);
	settings.terminalCursorBlink = parsed.terminalCursorBlink ?? defaults.terminalCursorBlink;
	settings.terminalCursorInactive = inactiveCursorStyle(parsed.terminalCursorInactive);
	settings.terminalCursorWidth = cursorWidth(parsed.terminalCursorWidth);
	settings.terminalScrollback = scrollbackSetting(parsed.terminalScrollback);
	settings.terminalLineNumbers = parsed.terminalLineNumbers === true;
	settings.terminalTimestamps = parsed.terminalTimestamps === true;
	settings.fontSize = parsed.fontSize ?? defaults.fontSize;
	settings.uiZoom = parsed.uiZoom ?? defaults.uiZoom;
	settings.compactLayout = parsed.compactLayout ?? defaults.compactLayout;
	settings.mobileBarKeys = Array.isArray(parsed.mobileBarKeys)
		? parsed.mobileBarKeys.filter((k: unknown): k is string => typeof k === 'string')
		: null;
	settings.terminalBar = parsed.terminalBar === 'hover' || parsed.terminalBar === 'hidden' ? parsed.terminalBar : defaults.terminalBar;
	settings.doubleClickToOpen = parsed.doubleClickToOpen ?? defaults.doubleClickToOpen;
	settings.closeTabOnExit = parsed.closeTabOnExit ?? defaults.closeTabOnExit;
	settings.reopenFloatWindows = parsed.reopenFloatWindows === true;
	settings.serverMonitoring = parsed.serverMonitoring ?? defaults.serverMonitoring;
	settings.checkAddresses = parsed.checkAddresses ?? defaults.checkAddresses;
	settings.checkForUpdates = parsed.checkForUpdates ?? defaults.checkForUpdates;
	settings.playSounds = parsed.playSounds ?? defaults.playSounds;
	settings.terminalBellSound = parsed.terminalBellSound ?? defaults.terminalBellSound;
	settings.terminalBellFlash = parsed.terminalBellFlash ?? defaults.terminalBellFlash;
	settings.terminalBellNotify = parsed.terminalBellNotify ?? defaults.terminalBellNotify;
	settings.muted = parsed.muted === true;
	settings.fontFamily = parsed.fontFamily ?? defaults.fontFamily;
	settings.defaultShell = parsed.defaultShell ?? defaults.defaultShell;
	settings.openLastSession = parsed.openLastSession ?? defaults.openLastSession;
	settings.locale = parsed.locale ?? defaults.locale;
	settings.minimizeToTray = parsed.minimizeToTray ?? defaults.minimizeToTray;
	settings.startWithSystem = parsed.startWithSystem ?? defaults.startWithSystem;
	settings.typeShellSetup = parsed.typeShellSetup ?? defaults.typeShellSetup;
	settings.showLoginMessage = parsed.showLoginMessage ?? defaults.showLoginMessage;
	settings.sessionLogMode = parsed.sessionLogMode ?? defaults.sessionLogMode;
	settings.sessionLogFolder = parsed.sessionLogFolder ?? defaults.sessionLogFolder;
	settings.sessionLogName =
		!parsed.sessionLogName || parsed.sessionLogName === OLD_DEFAULT_LOG_NAME
			? defaults.sessionLogName
			: parsed.sessionLogName;
	settings.sessionLogAppend = parsed.sessionLogAppend ?? defaults.sessionLogAppend;
	settings.sessionLogHeader = parsed.sessionLogHeader ?? defaults.sessionLogHeader;
	settings.historyMode = parsed.historyMode ?? defaults.historyMode;
	settings.historyOutputDays = parsed.historyOutputDays ?? defaults.historyOutputDays;
	settings.historyCommandDays = parsed.historyCommandDays ?? defaults.historyCommandDays;
	settings.historyMaxMb = parsed.historyMaxMb ?? defaults.historyMaxMb;
	settings.historyPatterns = parsed.historyPatterns ?? defaults.historyPatterns;
	settings.rdpHardwareRendering = parsed.rdpHardwareRendering ?? defaults.rdpHardwareRendering;
	settings.freerdpPath = typeof parsed.freerdpPath === 'string' ? parsed.freerdpPath : defaults.freerdpPath;
	settings.rdpGraphicsPipeline = parsed.rdpGraphicsPipeline ?? defaults.rdpGraphicsPipeline;
	settings.rdpNewGraphicsFormats = parsed.rdpNewGraphicsFormats ?? defaults.rdpNewGraphicsFormats;
	settings.rdpFullscreenButton = parsed.rdpFullscreenButton ?? defaults.rdpFullscreenButton;
	settings.rdpFullscreenButtonAt = parsed.rdpFullscreenButtonAt ?? defaults.rdpFullscreenButtonAt;
	settings.rdpBarPinned = parsed.rdpBarPinned ?? defaults.rdpBarPinned;
	// These two were saved but never read back, so "don't ask again"
	// lasted only until the next start.
	settings.warnOnMultilinePaste = parsed.warnOnMultilinePaste ?? defaults.warnOnMultilinePaste;
	settings.multilinePasteThreshold = parsed.multilinePasteThreshold ?? defaults.multilinePasteThreshold;
	settings.terminalRightClick = rightClickMode(parsed.terminalRightClick);
	settings.terminalCopyOnSelect = parsed.terminalCopyOnSelect ?? defaults.terminalCopyOnSelect;
	settings.terminalOsc52 = parsed.terminalOsc52 ?? defaults.terminalOsc52;
	settings.tmuxNewTabWindow = parsed.tmuxNewTabWindow ?? defaults.tmuxNewTabWindow;
	settings.tmuxConnectWindows = parsed.tmuxConnectWindows === 'all' ? 'all' : 'last';
	settings.tmuxHideStatus = parsed.tmuxHideStatus ?? defaults.tmuxHideStatus;
	settings.terminalMiddleClickPaste = parsed.terminalMiddleClickPaste ?? defaults.terminalMiddleClickPaste;
	settings.terminalWordSeparators = parsed.terminalWordSeparators ?? defaults.terminalWordSeparators;
	settings.terminalHighlight = parsed.terminalHighlight ?? defaults.terminalHighlight;
	settings.terminalHighlightKinds = highlightKinds(parsed.terminalHighlightKinds);
	settings.terminalHighlightCustom = typeof parsed.terminalHighlightCustom === 'string' ? parsed.terminalHighlightCustom : defaults.terminalHighlightCustom;
	settings.fileEditor = typeof parsed.fileEditor === 'string' ? parsed.fileEditor : defaults.fileEditor;
	settings.fileEditorCommand = typeof parsed.fileEditorCommand === 'string' ? parsed.fileEditorCommand : defaults.fileEditorCommand;
	settings.fileEditorTerminal = parsed.fileEditorTerminal ?? defaults.fileEditorTerminal;
	settings.winboxPath = typeof parsed.winboxPath === 'string' ? parsed.winboxPath : defaults.winboxPath;
	if (parsed.winboxPassword === 'argument' || parsed.winboxPassword === 'clipboard' || parsed.winboxPassword === 'none') {
		settings.winboxPassword = parsed.winboxPassword;
	}
	settings.pendingTursoOrg = parsed.pendingTursoOrg ?? defaults.pendingTursoOrg;
	settings.launches = parsed.launches ?? defaults.launches;
	settings.communityPromptDismissed =
		parsed.communityPromptDismissed ?? defaults.communityPromptDismissed;
	settings.communityPromptLastShown =
		parsed.communityPromptLastShown ?? defaults.communityPromptLastShown;
	// Absent for anyone upgrading, which is exactly who should see the
	// badges — so it stays empty rather than being seeded.
	settings.seenFeatures = parsed.seenFeatures ?? defaults.seenFeatures;
	// A fresh install starts with every DevOps tool off, so Reach opens
	// as a remote-access app. Anyone upgrading has had Ansible and
	// OpenTofu in the tab bar all along, and an update should not take
	// them away, so they keep both until they choose otherwise.
	settings.devopsTools = parsed.devopsTools ?? ['ansible', 'tofu'];
	settings.autoLockMinutes = parsed.autoLockMinutes ?? defaults.autoLockMinutes;
	settings.lockOnSystemLock = parsed.lockOnSystemLock ?? defaults.lockOnSystemLock;
	settings.unlockKeysAtStart = parsed.unlockKeysAtStart ?? defaults.unlockKeysAtStart;
	settings.unlockKeysWithBiometrics = parsed.unlockKeysWithBiometrics ?? defaults.unlockKeysWithBiometrics;
	settings.keyboardShortcuts = readOverrides(parsed.keyboardShortcuts);
	// Migration: existing users who already have localStorage data get setupComplete: true
	settings.setupComplete = parsed.setupComplete ?? true;
	return settings;
}
