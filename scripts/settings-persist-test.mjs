/**
 * Every local setting survives a save and a restart: stored the way the app
 * stores it, read back the way it reads it at start. A new setting must be
 * added to CHANGED below, so one that is saved but never read back (as the
 * app theme and the paste warning once were) fails here.
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/settings-persist-test.mjs
 */

import { defaults, readStoredSettings, storeSettings, MEMORY_ONLY } from '#lib/state/settings-store.js';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name}${extra ? '\n        ' + extra : ''}`);
	}
};
const section = (s) => console.log(`\n${s}`);
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** A value other than the default for every setting. */
const CHANGED = {
	theme: 'light',
	themeId: 'tokyo-night',
	themeLight: 'catppuccin-latte',
	themeDark: 'dracula',
	terminalThemeId: 'homebrew',
	terminalCursorStyle: 'block',
	terminalCursorBlink: false,
	terminalCursorInactive: 'none',
	terminalCursorWidth: 3,
	terminalScrollback: 50000,
	terminalLineNumbers: true,
	terminalTimestamps: true,
	fontSize: 17,
	uiZoom: 1.25,
	compactLayout: true,
	terminalBar: 'hover',
	mobileBarKeys: ['esc', 'tab'],
	mobileBarShow: 'never',
	doubleClickToOpen: true,
	showUnsupportedSessions: false,
	closeTabOnExit: true,
	reopenFloatWindows: true,
	serverMonitoring: false,
	checkAddresses: false,
	checkForUpdates: false,
	playSounds: false,
	terminalBellSound: false,
	terminalBellFlash: true,
	terminalBellNotify: true,
	muted: true,
	toastPosition: 'bottom-right',
	notificationHistory: false,
	systemNotifications: 'background',
	fontFamily: 'JetBrains Mono',
	defaultShell: '/bin/zsh',
	openLastSession: true,
	locale: 'cs',
	minimizeToTray: true,
	startWithSystem: true,
	typeShellSetup: true,
	showLoginMessage: false,
	sessionLogMode: 'all',
	sessionLogFolder: '/home/me/logs',
	sessionLogName: '&H.log',
	sessionLogAppend: false,
	sessionLogHeader: false,
	historyMode: 'output',
	historyOutputDays: 7,
	historyCommandDays: 30,
	historyMaxMb: 100,
	historyPatterns: 'secret=\\S+',
	rdpHardwareRendering: false,
	rdpGraphicsPipeline: true,
	rdpNewGraphicsFormats: false,
	rdpFullscreenButton: false,
	rdpFullscreenButtonAt: { x: 0.2, y: 0.8 },
	rdpBarPinned: true,
	warnOnMultilinePaste: false,
	multilinePasteThreshold: 10,
	terminalRightClick: 'paste',
	terminalCopyOnSelect: true,
	terminalOsc52: true,
	tmuxNewTabWindow: false,
	tmuxConnectWindows: 'all',
	tmuxHideStatus: true,
	terminalMiddleClickPaste: false,
	terminalWordSeparators: '()',
	terminalHighlight: true,
	terminalHighlightKinds: ['success', 'mac'],
	terminalHighlightCustom: '/vlan \\d+/i',
	fileEditor: 'custom',
	fileEditorCommand: 'subl -w',
	fileEditorTerminal: true,
	freerdpPath: '/opt/freerdp/bin/sdl-freerdp',
	winboxPath: '/opt/winbox/WinBox',
	winboxPassword: 'clipboard',
	setupComplete: true,
	pendingTursoOrg: 'my-org',
	pendingTursoApiToken: 'token',
	launches: 12,
	communityPromptDismissed: true,
	communityPromptLastShown: 1700000000000,
	seenFeatures: ['devops'],
	devopsTools: ['ansible'],
	autoLockMinutes: 15,
	lockOnSystemLock: true,
	unlockKeysAtStart: true,
	unlockKeysWithBiometrics: true,
	keyboardShortcuts: { 'tab.new': ['Mod+Alt+T'], 'app.aiPanel': [] }
};

const restart = (s) => readStoredSettings(JSON.parse(storeSettings(s)));

section('every setting is covered');
for (const key of Object.keys(defaults)) {
	check(`${key} has a test value`, key in CHANGED);
	if (key in CHANGED) check(`${key} test value differs from the default`, !same(CHANGED[key], defaults[key]));
}

section('a changed setting survives a restart');
const back = restart({ ...defaults, ...CHANGED });
for (const key of Object.keys(CHANGED)) {
	if (MEMORY_ONLY.includes(key)) continue;
	check(key, same(back[key], CHANGED[key]), `saved ${JSON.stringify(CHANGED[key])}, read back ${JSON.stringify(back[key])}`);
}

section('memory-only settings are not written');
for (const key of MEMORY_ONLY) {
	check(`${key} is not stored`, !(key in JSON.parse(storeSettings({ ...defaults, ...CHANGED }))));
}

section('defaults survive a restart');
const fresh = restart({ ...defaults, setupComplete: true });
for (const key of Object.keys(defaults)) {
	if (key === 'setupComplete') continue;
	check(`${key} stays default`, same(fresh[key], defaults[key]), `read back ${JSON.stringify(fresh[key])}`);
}

section('an update from an older version');
const old = readStoredSettings({ theme: 'light', fontSize: 16 });
check('saved values kept', old.theme === 'light' && old.fontSize === 16);
check('missing ones are defaults', old.terminalCursorStyle === defaults.terminalCursorStyle && old.historyMode === 'off');
check('an older install counts as set up', old.setupComplete === true);
check('an older install keeps Ansible and OpenTofu', same(old.devopsTools, ['ansible', 'tofu']));
check('an empty theme id is no theme', readStoredSettings({ themeId: '' }).themeId === undefined);
const stockTheme = readStoredSettings({ theme: 'dark', themeId: 'reach-light' });
check('an older built-in theme choice becomes its base mode', stockTheme.themeId === undefined && stockTheme.theme === 'light');
check('WinBox starts without sharing passwords', old.winboxPassword === 'none');
check('WinBox keeps saved command-line login', restart({ ...defaults, winboxPassword: 'argument' }).winboxPassword === 'argument');

check('an unknown shortcut action is dropped', same(readStoredSettings({ keyboardShortcuts: { nope: ['Mod+X'], 'tab.new': ['Mod+Y', 3] } }).keyboardShortcuts, { 'tab.new': ['Mod+Y'] }));

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
