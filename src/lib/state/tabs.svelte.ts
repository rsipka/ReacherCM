import type { SshConnectParams } from '#lib/ipc/ssh.js';
import { railWindowsSync, rdpDisconnect, type RdpConnectParams } from '#lib/ipc/rdp.js';
import { vncDisconnect, type VncConnectParams } from '#lib/ipc/vnc.js';
import { spiceDisconnect, type SpiceConnectParams } from '#lib/ipc/spice.js';
import type { TelnetConnectParams } from '#lib/ipc/telnet.js';
import type { SerialConnectParams } from '#lib/ipc/serial.js';
import { ftpDisconnect } from '#lib/ipc/ftp.js';
import { dropQueue } from '#lib/explorer/transfer-queue.svelte.js';
import type { FilesProtocol } from '#lib/ipc/sessions.js';
import type { HistoryMode } from '#lib/ipc/history.js';
import * as splits from '#lib/terminal/split.js';
import { isMobile } from '#lib/platform.js';
import type { Direction, Layout, Side } from '#lib/terminal/split.js';

export type TabType = 'local' | 'ssh' | 'rdp' | 'vnc' | 'spice' | 'telnet' | 'serial' | 'ftp';

/** A tab that shows a remote desktop rather than a terminal. */
export function isDesktopTab(tab: Pick<Tab, 'type'> | undefined): boolean {
	return tab?.type === 'rdp' || tab?.type === 'vnc' || tab?.type === 'spice';
}

export interface Tab {
	id: string;
	title: string;
	type: TabType;
	connectionId?: string;
	active: boolean;
	/** Session display name (e.g. "MiniPC") */
	sessionName?: string;
	/** Detected OS ID for distro icon (e.g. "ubuntu", "debian") */
	detectedOs?: string | null;
	/** SSH connect params for reconnection */
	sshConnectParams?: SshConnectParams;
	/** What an RDP tab connects to; the panel connects itself once it knows its size. */
	rdpConnectParams?: RdpConnectParams;
	/** What a VNC tab connects to. */
	vncConnectParams?: VncConnectParams;
	/** What a SPICE tab connects to. */
	spiceConnectParams?: SpiceConnectParams;
	/** What a Telnet tab connects to; its terminal connects once it listens. */
	telnetConnectParams?: TelnetConnectParams;
	/** A Telnet tab's open connection. Kept apart from `connectionId`, which
	 *  the file explorer, recipes and the rest read as an SSH connection. */
	telnetConnectionId?: string;
	/** What a serial tab opens; its terminal opens the port once it listens. */
	serialConnectParams?: SerialConnectParams;
	/** A serial tab's open connection, kept apart like `telnetConnectionId`. */
	serialConnectionId?: string;
	/** A local tab that runs this program instead of the default shell
	 *  (a shell inside a container on this computer, say). */
	localCommand?: { program: string; args: string[] };
	/** The WSL distribution name if this tab is a WSL session. */
	wslDistro?: string;
	/** The tab group it is in, if any (see TabGroup). */
	groupId?: string;
	/** An SSH or Telnet tab whose connection dropped (or ended), or is being made
	 *  again, or a desktop that closed: shown on the tab, so a dead session is
	 *  seen from any tab, not only its own. Closing the app does not count it. */
	link?: 'down' | 'reconnecting';
	/** The saved session this tab was opened from, if any. */
	sessionId?: string;
	/** That session's terminal theme id; unset: the default from Settings. */
	terminalTheme?: string | null;
	/** That session's cursor shape and blink; unset: the defaults from
	 *  Settings (see #lib/terminal/cursor). */
	cursorStyle?: string | null;
	cursorBlink?: boolean | null;
	/** That session's server monitoring choice; unset: the default from
	 *  Settings (see monitoringWanted). */
	monitoring?: boolean | null;
	/** Bumped to make an SSH tab connect again into the tmux session its
	 *  connect settings now name (Switch tmux session). */
	tmuxSwitch?: number;
	/** Bumped by the tab menu's Reconnect: a terminal connects again, a
	 *  remote desktop's panel is made again (see canReconnect). */
	reconnect?: number;
	/** The tmux window a tmux tab shows (its id and name), from the title
	 *  its view sets (see #lib/terminal/tmux-tabs). */
	tmuxWindow?: { id: string; name: string };
	/** That session's history choice; unset: the default from Settings. */
	historyMode?: HistoryMode | null;
	/** A tab just moved here from another window (see #lib/state/windows):
	 *  its terminal takes over the one there, screen and scrollback, instead of
	 *  starting anew. Cleared once it has. */
	handoff?: Handoff;
	/** An SSH tab showing the full-pane file manager (this computer beside
	 *  the server) in place of its terminal, which keeps running behind it. */
	filesView?: boolean;
	/** An SSH tab opened from an SFTP or SCP session: it starts in the
	 *  file manager, and SCP copies go through scp. */
	filesProtocol?: FilesProtocol;
	/** What an FTP tab connects to; its panel connects it. */
	ftpConnectParams?: FtpConnectParams;
	/** An FTP tab's open connection, kept apart like `telnetConnectionId`. */
	ftpConnectionId?: string;
	/** A remote desktop tab whose desktop is up (set by its panel); unset
	 *  while it connects, and once it closed. */
	desktopUp?: boolean;
}

/** An FTP tab's saved session; the backend takes the rest from it. */
export interface FtpConnectParams {
	sessionId: string;
	host: string;
	port: number;
	username: string;
}

export interface Handoff {
	/** The window giving the tab up. */
	from: string;
	moveId: string;
}

let tabs = $state<Tab[]>([]);

let activeTab = $derived<Tab | undefined>(tabs.find((t) => t.active));

/** Tabs side by side, split any way (#106); see #lib/terminal/split. */
let split = $state.raw<Layout>(null);
/** A tab being dragged (from the tab bar or a pane's bar), for the terminal area to drop. */
let draggedTab = $state<string | null>(null);

export function getSplit(): Layout {
	return split;
}

/** Is this tab one of the panes on screen? */
export function isInSplit(id: string): boolean {
	return splits.contains(split, id);
}

/** A new local terminal for a split, without it taking the focused pane. */
function freshLocal(): string | null {
	if (isMobile()) return null;
	const keep = split;
	const fresh = createTab('local');
	split = keep;
	return fresh.id;
}

/**
 * Show `tabId` on `side` of the focused pane, which keeps its place. Asked
 * for the focused tab itself (its menu, its bar's button), the next tab not on
 * screen comes in on that side instead; with none, a new local terminal (not
 * on a phone or tablet, which has no local shell: there nothing happens).
 */
export function splitWith(tabId: string, direction: Direction | Side): void {
	const side = direction === 'right' || direction === 'down' ? splits.sideOf(direction) : direction;
	const focused = activeTab?.id;
	if (!focused) return;
	let incoming = tabId;
	if (tabId === focused) {
		const other = splits.neighbour(tabs.map((t) => t.id), focused, splits.leaves(split)) ?? freshLocal();
		if (!other) return;
		incoming = other;
	}
	const next = splits.insert(split, focused, incoming, side);
	if (!next) return;
	split = next;
	activateTab(incoming);
}

/**
 * A dragged tab let go over the terminal area (see splits.dropAt): beside a
 * pane, along a whole side, or into a pane (two panes trade places).
 */
export function dropTab(tabId: string, drop: Pick<splits.Drop, 'target' | 'zone'>): void {
	const { target, zone } = drop;
	if (target === null) {
		if (zone !== 'center') split = splits.insertAtEdge(split, tabId, zone);
		activateTab(tabId);
		return;
	}
	if (zone === 'center') {
		if (split) split = splits.place(split, target, tabId);
		activateTab(tabId);
		return;
	}
	if (target === tabId) {
		// Its own pane: with no split, it goes to that side and the next tab
		// takes the rest; in a split it is already there.
		if (split) return;
		const other = splits.neighbour(tabs.map((t) => t.id), tabId) ?? freshLocal();
		if (!other) return;
		split = splits.insert(null, other, tabId, zone);
	} else {
		split = splits.insert(split, target, tabId, zone);
	}
	activateTab(tabId);
}

/** Show this split (a saved layout opening; see #lib/state/layouts). */
export function setLayout(layout: Layout): void {
	split = layout;
}

/** Back to one tab at a time. */
export function unsplit(): void {
	split = null;
}

/** Take one tab out of the split; it stays open as a tab. */
export function removeFromSplit(id: string): void {
	const wasFocused = activeTab?.id === id;
	const after = splits.remove(split, id);
	split = after.layout;
	if (wasFocused && after.focus) activateTab(after.focus);
}

/** Move one divider (by its node's path, see splits.setRatio). */
export function setSplitRatio(path: string, ratio: number): void {
	split = splits.setRatio(split, path, ratio);
}

export function getDraggedTab(): string | null {
	return draggedTab;
}

export function setDraggedTab(id: string | null): void {
	draggedTab = id;
}

export function getTabs(): Tab[] {
	return tabs;
}

export function getActiveTab(): Tab | undefined {
	return activeTab;
}

/** "Local", then "Local 2", "Local 3": three tabs all called Local could not
 *  be told apart in the tab bar, the palette or the status bar. */
function numbered(base: string): string {
	const taken = new Set(tabs.map((t) => t.title));
	if (!taken.has(base)) return base;
	for (let n = 2; ; n++) {
		if (!taken.has(`${base} ${n}`)) return `${base} ${n}`;
	}
}

export function createTab(type: TabType, title?: string, connectionId?: string, sessionName?: string, detectedOs?: string | null, localCommand?: { program: string; args: string[] }, wslDistro?: string): Tab {
	const id = crypto.randomUUID();

	// In a split, a new tab takes the focused pane, as choosing a tab does.
	split = splits.activate(split, activeTab?.id, id);

	// Deactivate all existing tabs
	for (const tab of tabs) {
		tab.active = false;
	}

	const tab: Tab = {
		id,
		title: title ?? numbered(type === 'local' ? 'Local' : type === 'rdp' ? 'RDP' : type === 'vnc' ? 'VNC' : type === 'spice' ? 'SPICE' : type === 'telnet' ? 'Telnet' : type === 'serial' ? 'Serial' : type === 'ftp' ? 'FTP' : 'SSH'),
		type,
		connectionId,
		active: true,
		sessionName,
		detectedOs,
		localCommand,
		wslDistro,
	};

	tabs.push(tab);
	return tab;
}

// ---- Moving tabs between windows (see #lib/state/windows) -----------------

/** Tab and connection ids handed to another window: their tab went, but
 *  their connection, PTY and monitoring live on there, so nothing here may
 *  end them. */
const released = new Set<string>();

function connectionIds(tab: Tab): string[] {
	return [tab.id, tab.connectionId, tab.telnetConnectionId, tab.serialConnectionId, tab.ftpConnectionId].filter((x): x is string => !!x);
}

/** Was this tab or connection handed to another window? */
export function isReleased(id: string): boolean {
	return released.has(id);
}

/** A tab now shown by another window goes from this one, its session untouched. */
export function releaseTab(id: string): void {
	const index = tabs.findIndex((t) => t.id === id);
	if (index === -1) return;
	for (const c of connectionIds(tabs[index])) released.add(c);
	const wasActive = tabs[index].active;
	const after = splits.remove(split, id);
	split = after.layout;
	tabs.splice(index, 1);
	dropEmptyGroups();
	if (wasActive && after.focus) activateTab(after.focus);
	else if (wasActive && tabs.length > 0) tabs[Math.min(index, tabs.length - 1)].active = true;
}

/**
 * Tabs moved here from another window: added after the ones here, `layout`
 * (their split there) shown when nothing here is split, `active` in front.
 */
export function adoptTabs(incoming: Tab[], layout: Layout, active?: string): void {
	for (const tab of incoming) {
		for (const c of connectionIds(tab)) released.delete(c);
		// Groups belong to the window they were made in.
		tabs.push({ ...tab, groupId: undefined, active: false });
	}
	if (layout && !split) split = layout;
	const front = active ?? incoming[0]?.id;
	if (front) activateTab(front);
}

/** The tab's terminal has taken over from the other window's. */
export function clearHandoff(id: string): void {
	const tab = tabs.find((t) => t.id === id);
	if (tab?.handoff) tab.handoff = undefined;
}

export function closeTab(id: string): void {
	const index = tabs.findIndex((t) => t.id === id);
	if (index === -1) return;

	// A desktop's session lives in the backend and outlives its panel; the
	// tab going is what ends it.
	if (tabs[index].type === 'rdp') {
		void rdpDisconnect(id).catch(() => {});
		// A RemoteApp's windows of their own go with it.
		void railWindowsSync(id, []).catch(() => {});
	} else if (tabs[index].type === 'vnc') {
		void vncDisconnect(id).catch(() => {});
	} else if (tabs[index].type === 'spice') {
		void spiceDisconnect(id).catch(() => {});
	} else if (tabs[index].type === 'ftp' && tabs[index].ftpConnectionId) {
		dropQueue(tabs[index].ftpConnectionId!);
		void ftpDisconnect(tabs[index].ftpConnectionId!).catch(() => {});
	}

	const wasActive = tabs[index].active;
	// A pane closing: the one beside it gets its room, and the focus.
	const after = splits.remove(split, id);
	split = after.layout;

	tabs.splice(index, 1);
	dropEmptyGroups();

	if (wasActive && after.focus) {
		activateTab(after.focus);
	} else if (wasActive && tabs.length > 0) {
		// If the closed tab was active and there are remaining tabs, activate an adjacent one
		const newIndex = Math.min(index, tabs.length - 1);
		tabs[newIndex].active = true;
	}
}

export function activateTab(id: string): void {
	// A tab shown is never hidden in a folded group.
	const g = groupOfTab(id);
	if (g?.collapsed) g.collapsed = false;
	// In a split, a tab not on screen takes the focused pane.
	split = splits.activate(split, activeTab?.id, id);
	for (const tab of tabs) {
		tab.active = tab.id === id;
	}
}

export function updateTabTitle(id: string, title: string): void {
	const tab = tabs.find((t) => t.id === id);
	if (tab) {
		tab.title = title;
	}
}

/** Set by the tab's terminal; undefined when the connection is up. */
/** Whether a tab can show the full-pane file manager: an SSH tab on a
 *  desktop, where there is a local side to show beside the server. */
export function canShowFilesView(tab: Tab | undefined): boolean {
	return !!tab && tab.type === 'ssh' && !isMobile();
}

/** Switch an SSH tab between its terminal and the full-pane file manager. */
export function toggleFilesView(id: string, on?: boolean): void {
	const tab = tabs.find((t) => t.id === id);
	if (!canShowFilesView(tab)) return;
	tab!.filesView = on ?? !tab!.filesView;
}

export function setTabLink(id: string, link: Tab['link']): void {
	const tab = tabs.find((t) => t.id === id);
	if (tab && tab.link !== link) tab.link = link;
}

/** Mark a remote desktop tab's desktop as up or not (see RdpPanel). */
export function setDesktopUp(id: string, up: boolean): void {
	const tab = tabs.find((t) => t.id === id);
	if (tab && !!tab.desktopUp !== up) tab.desktopUp = up;
}

/** The saved session a tab was opened from: remote desktops keep it in
 *  their connect params only. */
export function tabSessionId(tab: Tab): string | undefined {
	return tab.sessionId ?? tab.ftpConnectParams?.sessionId ?? tab.rdpConnectParams?.sessionId ?? tab.vncConnectParams?.sessionId ?? tab.spiceConnectParams?.sessionId ?? undefined;
}

/** Is the tab's connection up: not dropped, not still connecting? A local
 *  shell (WSL) counts while its tab is open. */
export function isTabConnected(tab: Tab): boolean {
	if (tab.link) return false;
	switch (tab.type) {
		case 'ssh':
			return !!tab.connectionId;
		case 'telnet':
			return !!tab.telnetConnectionId;
		case 'serial':
			return !!tab.serialConnectionId;
		case 'ftp':
			return !!tab.ftpConnectionId;
		case 'local':
			return true;
		default:
			return !!tab.desktopUp;
	}
}

export interface SessionTabState {
	/** The session of the tab on screen (the focused pane in a split). */
	current: boolean;
	/** At least one of its tabs is connected. */
	connected: boolean;
}

/** By saved session id: the sessions that have a tab open in this window. */
let sessionTabStates = $derived.by(() => {
	const states = new Map<string, SessionTabState>();
	for (const tab of tabs) {
		const id = tabSessionId(tab);
		if (!id) continue;
		const state = states.get(id) ?? { current: false, connected: false };
		if (tab.active) state.current = true;
		if (isTabConnected(tab)) state.connected = true;
		states.set(id, state);
	}
	return states;
});

export function getSessionTabStates(): Map<string, SessionTabState> {
	return sessionTabStates;
}

export function updateTabOs(connectionId: string, os: string): void {
	const tab = tabs.find((t) => t.connectionId === connectionId);
	if (tab) {
		tab.detectedOs = os;
	}
}

export function setTelnetConnection(tabId: string, connectionId: string): void {
	const tab = tabs.find((t) => t.id === tabId);
	if (tab) tab.telnetConnectionId = connectionId;
}

export function setSerialConnection(tabId: string, connectionId: string): void {
	const tab = tabs.find((t) => t.id === tabId);
	if (tab) tab.serialConnectionId = connectionId;
}

export function setFtpConnection(tabId: string, connectionId: string | undefined): void {
	const tab = tabs.find((t) => t.id === tabId);
	if (tab) tab.ftpConnectionId = connectionId;
}

export function updateTabConnection(tabId: string, newConnectionId: string): void {
	const tab = tabs.find((t) => t.id === tabId);
	if (tab) {
		tab.connectionId = newConnectionId;
	}
}

export function reorderTab(fromIndex: number, toIndex: number): void {
	if (
		fromIndex < 0 ||
		fromIndex >= tabs.length ||
		toIndex < 0 ||
		toIndex >= tabs.length ||
		fromIndex === toIndex
	) {
		return;
	}

	const [moved] = tabs.splice(fromIndex, 1);
	tabs.splice(toIndex, 0, moved);
}

// ---- Tab groups ---------------------------------------------------------
//
// A named, coloured run of tabs in the bar, as in a browser: its tabs stay
// side by side, and folding the group hides them (their sessions keep
// running). Groups last while their tabs are open; nothing is saved.

// Blue last: it is the selected tab's colour, and a group must not look selected.
export const GROUP_COLORS = ['green', 'purple', 'orange', 'cyan', 'pink', 'yellow', 'red', 'grey', 'blue'] as const;
export type GroupColor = (typeof GROUP_COLORS)[number];

export interface TabGroup {
	id: string;
	name: string;
	color: GroupColor;
	collapsed: boolean;
}

let groups = $state<TabGroup[]>([]);

export function getGroups(): TabGroup[] {
	return groups;
}

export function groupOfTab(tabId: string): TabGroup | undefined {
	const id = tabs.find((t) => t.id === tabId)?.groupId;
	return id ? groups.find((g) => g.id === id) : undefined;
}

/** Index just past the last tab of `groupId`, or -1 if it has none. */
function groupEnd(groupId: string): number {
	let end = -1;
	tabs.forEach((t, i) => {
		if (t.groupId === groupId) end = i + 1;
	});
	return end;
}

/** Move a tab to `to` (an index into the bar as it is before the move). */
function moveTab(tabId: string, to: number): void {
	const from = tabs.findIndex((t) => t.id === tabId);
	if (from === -1) return;
	const [tab] = tabs.splice(from, 1);
	tabs.splice(from < to ? to - 1 : to, 0, tab);
}

/** A new group holding `tabId`, named after it, in the next unused colour. */
export function createGroup(tabId: string, name?: string): TabGroup | undefined {
	const tab = tabs.find((t) => t.id === tabId);
	if (!tab) return undefined;
	if (tab.groupId) removeFromGroup(tabId);
	const used = new Set(groups.map((g) => g.color));
	const color = GROUP_COLORS.find((c) => !used.has(c)) ?? GROUP_COLORS[groups.length % GROUP_COLORS.length];
	const group: TabGroup = { id: crypto.randomUUID(), name: name ?? (tab.sessionName || tab.title), color, collapsed: false };
	groups.push(group);
	tab.groupId = group.id;
	return groups[groups.length - 1];
}

/** Put a tab in a group, after the group's last tab so its tabs stay together. */
export function addToGroup(tabId: string, groupId: string): void {
	const tab = tabs.find((t) => t.id === tabId);
	if (!tab || !groups.some((g) => g.id === groupId) || tab.groupId === groupId) return;
	const end = groupEnd(groupId);
	tab.groupId = groupId;
	if (end !== -1) moveTab(tabId, end);
	dropEmptyGroups();
}

/** Take a tab out of its group, placing it just after the group. */
export function removeFromGroup(tabId: string): void {
	const tab = tabs.find((t) => t.id === tabId);
	const groupId = tab?.groupId;
	if (!tab || !groupId) return;
	tab.groupId = undefined;
	const end = groupEnd(groupId);
	if (end !== -1) moveTab(tabId, end);
	dropEmptyGroups();
}

export function renameGroup(groupId: string, name: string): void {
	const g = groups.find((x) => x.id === groupId);
	if (g && name.trim()) g.name = name.trim();
}

export function setGroupColor(groupId: string, color: GroupColor): void {
	const g = groups.find((x) => x.id === groupId);
	if (g) g.color = color;
}

/** Fold or unfold a group. Folding the group of the tab in front shows the
 *  nearest tab outside it instead, if there is one. */
export function toggleGroup(groupId: string): void {
	const g = groups.find((x) => x.id === groupId);
	if (!g) return;
	g.collapsed = !g.collapsed;
	if (g.collapsed && activeTab?.groupId === groupId) {
		const outside = tabs.find((t) => t.groupId !== groupId && !groupHidden(t));
		if (outside) activateTab(outside.id);
	}
}

/** Whether a tab is hidden in a folded group. */
export function groupHidden(tab: Tab): boolean {
	if (!tab.groupId) return false;
	return groups.find((g) => g.id === tab.groupId)?.collapsed ?? false;
}

/** Undo a group; its tabs stay open where they are. */
export function ungroup(groupId: string): void {
	for (const t of tabs) if (t.groupId === groupId) t.groupId = undefined;
	groups = groups.filter((g) => g.id !== groupId);
}

/** Close every tab in a group. */
export function closeGroup(groupId: string): void {
	for (const t of tabs.filter((x) => x.groupId === groupId)) closeTab(t.id);
	groups = groups.filter((g) => g.id !== groupId);
}

function dropEmptyGroups(): void {
	const used = new Set(tabs.map((t) => t.groupId).filter(Boolean));
	if (groups.some((g) => !used.has(g.id))) groups = groups.filter((g) => used.has(g.id));
}

// ---- Closing several tabs, and placing a copy (#160) ---------------------

export type CloseScope = 'others' | 'left' | 'right';

/** The tabs "close others / to the left / to the right" of `id` would
 *  close, in bar order. Tabs folded away in a group count: they are still
 *  open, as in a browser. */
export function tabsBeside(id: string, scope: CloseScope): Tab[] {
	const i = tabs.findIndex((t) => t.id === id);
	if (i === -1) return [];
	if (scope === 'left') return tabs.slice(0, i);
	if (scope === 'right') return tabs.slice(i + 1);
	return tabs.filter((t) => t.id !== id);
}

/** Several tabs waiting on the user's yes; the tab bar asks. */
let pendingClose = $state<{ ids: string[]; keep: string } | null>(null);

export function getPendingClose(): { ids: string[]; keep: string } | null {
	return pendingClose;
}

/**
 * Close the tabs `scope` names around `id`, which stays and comes to the
 * front. One tab goes at once; more are asked about first, since every
 * session among them ends, as closing a group asks.
 */
export function requestCloseBeside(id: string, scope: CloseScope): void {
	const ids = tabsBeside(id, scope).map((t) => t.id);
	if (ids.length === 0) return;
	if (ids.length === 1) {
		closeTabs(ids, id);
		return;
	}
	pendingClose = { ids, keep: id };
}

export function confirmPendingClose(): void {
	const p = pendingClose;
	pendingClose = null;
	if (p) closeTabs(p.ids, p.keep);
}

export function cancelPendingClose(): void {
	pendingClose = null;
}

function closeTabs(ids: string[], keep: string): void {
	if (tabs.some((t) => t.id === keep)) activateTab(keep);
	for (const id of ids) closeTab(id);
}

/** Put `tabId` just after `refId`, in the same group: where a copy goes. */
export function placeAfter(tabId: string, refId: string): void {
	const ref = tabs.find((t) => t.id === refId);
	const tab = tabs.find((t) => t.id === tabId);
	if (!ref || !tab) return;
	tab.groupId = ref.groupId;
	moveTab(tabId, tabs.findIndex((t) => t.id === refId) + 1);
}

/**
 * A session's terminal theme changed: its open tabs take the new colours at
 * once. Only the theme; the rest of a tab stays as it was opened.
 */
/** A session's cursor choice changed: its open tabs follow at once. */
export function setSessionCursor(sessionId: string, style: string | null | undefined, blink: boolean | null | undefined): void {
	for (const tab of tabs) {
		if (tab.sessionId !== sessionId) continue;
		tab.cursorStyle = style ?? null;
		tab.cursorBlink = blink ?? null;
	}
}

/** A session's monitoring choice changed: its open tabs follow at once. */
export function setSessionMonitoring(sessionId: string, monitoring: boolean | null | undefined): void {
	for (const tab of tabs) {
		if (tab.sessionId === sessionId) tab.monitoring = monitoring ?? null;
	}
}

export function setSessionTerminalTheme(sessionId: string, themeId: string | null | undefined): void {
	for (const tab of tabs) {
		if (tab.sessionId === sessionId) tab.terminalTheme = themeId ?? null;
	}
}
