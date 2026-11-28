<script lang="ts">
	import { canZmodem, requestZmodem } from '#lib/state/zmodem.js';
	import {
		getTabs, getActiveTab, createTab, closeTab, activateTab, getSplit, splitWith, unsplit, removeFromSplit, getDraggedTab, setDraggedTab, type Tab,
		getGroups, createGroup, addToGroup, removeFromGroup, renameGroup, setGroupColor, toggleGroup, groupHidden, ungroup, closeGroup,
		GROUP_COLORS, type GroupColor, type TabGroup,
		tabsBeside, requestCloseBeside, getPendingClose, confirmPendingClose, cancelPendingClose, isDesktopTab
	} from '#lib/state/tabs.svelte.js';
	import { canDuplicate, duplicateTab } from '#lib/terminal/duplicate-tab.js';
	import { vncSecurity, type VncSecurity } from '#lib/ipc/vnc.js';
	import { sshConnectionAlgorithms, sshTmuxKill, sshTmuxList, sshTmuxWindows, sshTmuxWindow, type NegotiatedAlgorithms, type TmuxListed, type TmuxWindow, type TmuxWindowOp } from '#lib/ipc/ssh.js';
	import { validTmuxName } from '#lib/terminal/tmux.js';
	import { newTab, isTmuxViewTab, listedMeta, openTmuxWindowTab, switchTmuxSession, tmuxSessionOf } from '#lib/terminal/tmux-tabs.js';
	import { SPLIT_MIN_WIDTH, contains, leaves } from '#lib/terminal/split.js';
	import { isMobile } from '#lib/platform.js';
	import { getSettings } from '#lib/state/settings.svelte.js';
	import { actionLabel } from '#lib/state/shortcuts.svelte.js';
	import { canCapture, captureTab } from '#lib/state/capture.js';
	import { KEY_COMBOS, TAB_MENU_COMBOS, sendKeys, sendKeysReady } from '#lib/state/send-keys.js';
	import { CAMERA_ICON } from '#lib/terminal/snapshot.js';
	import {
		getBroadcastTargets, isTerminalTab, toggleBroadcastTarget, broadcastToAll, stopBroadcast
	} from '#lib/state/broadcast.svelte.js';
	import { BROADCAST_ICON } from '#lib/terminal/broadcast.js';
	import { openTabHistory, openTabCommandPicker } from '#lib/state/history.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { requestSessionAction } from '#lib/state/palette.svelte.js';
	import { writeText } from '@tauri-apps/plugin-clipboard-manager';
	import DbMenu, { type MenuItem } from '#lib/components/db/DbMenu.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import LayoutsDialog from './LayoutsDialog.svelte';
	import { getLayouts, openLayout } from '#lib/state/layouts.svelte.js';
	import Button from '#lib/components/shared/Button.svelte';
	import { DEVOPS_TOOLS } from '#lib/state/devops.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import DistroIcon from '#lib/components/sessions/DistroIcon.svelte';
	import * as mcp from '#lib/state/mcp.svelte.js';
	import { onMount } from 'svelte';
	import { canFloat, isFloatWindow, otherWindows, moveToNewWindow, moveToWindow, type WindowInfo } from '#lib/state/windows.svelte.js';

	/** A floating window (#lib/state/windows): its tabs only, no pages or saved layouts. */
	const floating = isFloatWindow();
	/** The other windows open, for "Move to window"; read when a tab's menu opens. */
	let windows = $state<WindowInfo[]>([]);
	const WINDOW_ICON = 'M4 5h16v14H4zM4 9h16';

	function windowName(w: WindowInfo): string {
		if (w.label === 'main') return t('windows.main');
		const n = w.label.replace(/^float-/, '');
		return w.title ? t('windows.numbered_titled', { n, title: w.title }) : t('windows.numbered', { n });
	}

	/** Moving a tab (or its whole split) to a window of its own, or to another one. */
	function windowItems(id: string): MenuItem[] {
		// A DevOps workspace keeps its state in this window.
		if (!canFloat() || tabs.find((x) => x.id === id)?.type === 'devops') return [];
		const items: MenuItem[] = [{ label: '', separator: true }];
		const alone = floating && tabs.length === 1;
		items.push({ label: t('windows.move_new'), icon: WINDOW_ICON, disabled: alone, action: () => void moveToNewWindow([id]) });
		if (split && contains(split, id)) {
			const panes = leaves(split);
			items.push({ label: t('windows.move_split_new'), icon: WINDOW_ICON, disabled: floating && panes.length === tabs.length, action: () => void moveToNewWindow(panes) });
		}
		for (const w of windows) items.push({ label: t('windows.move_to', { window: windowName(w) }), action: () => void moveToWindow([id], w.label) });
		return items;
	}

	let mcpStatus = $derived(mcp.getMcpStatus());

	// The share control is only rendered when the MCP server is running. A
	// button that cannot do anything is worse than no button: it invites a
	// click, fails, and teaches people the feature is broken.
	onMount(() => { mcp.refresh(); });

	/** The id the backend keys output on: the SSH connection, or the PTY id. */
	function shareIdOf(tab: { id: string; type: string; connectionId?: string }): string | null {
		if (tab.type === 'ssh') return tab.connectionId ?? null;
		if (tab.type === 'local') return tab.id;
		return null;
	}

	async function toggleShare(e: MouseEvent, tab: Tab): Promise<void> {
		e.stopPropagation();
		const id = shareIdOf(tab);
		if (!id) return;
		if (mcp.isShared(id)) {
			await mcp.unshare(id);
			return;
		}
		const isSsh = tab.type === 'ssh';
		await mcp.share(
			id,
			isSsh ? 'ssh' : 'local',
			tab.sshConnectParams?.host ?? 'localhost',
			tab.sshConnectParams?.username ?? ''
		);
	}

	let tabs = $derived(getTabs());
	let activeTab = $derived(getActiveTab());
	let broadcastTargets = $derived(getBroadcastTargets());

	/** Typing into several terminals at once: this tab in or out, a group,
	 *  every terminal, or none. */
	function broadcastItems(tab: Tab | undefined): MenuItem[] {
		if (!isTerminalTab(tab) || !tab) return [];
		const items: MenuItem[] = [
			{ label: '', separator: true },
			{
				label: t('broadcast.tab_toggle'),
				hint: `${mod}B`,
				icon: BROADCAST_ICON,
				checked: broadcastTargets.includes(tab.id),
				action: () => toggleBroadcastTarget(tab.id)
			}
		];
		if (tab.groupId) items.push({ label: t('broadcast.group'), action: () => broadcastToAll(tab.groupId) });
		items.push({ label: t('broadcast.all'), disabled: tabs.filter(isTerminalTab).length < 2, action: () => broadcastToAll() });
		if (broadcastTargets.length > 0) items.push({ label: t('broadcast.stop'), action: stopBroadcast });
		return items;
	}

	// A phone or tablet has no local shell: no + (it opened one), and a split
	// needs a second open tab instead of a new local terminal.
	const mobile = isMobile();
	let newTabTip = $derived.by(() => {
		const label = getSettings().tmuxNewTabWindow && isTmuxViewTab(activeTab) ? t('tmux.window_new_tab') : t('tabs.new_local');
		const key = actionLabel('tab.new');
		return key ? `${label} (${key})` : label;
	});

	function handleNewTab(): void {
		newTab();
	}

	function handleCloseTab(e: MouseEvent, id: string): void {
		e.stopPropagation();
		closeTab(id);
	}

	// Split view (#106): drag a tab onto a pane's side or into it, or use its menu.
	let split = $derived(getSplit());
	let dragged = $derived(getDraggedTab());
	let menu = $state<{ x: number; y: number; id: string } | null>(null);

	/** What each SSH connection's key exchange settled on, by connection id,
	 *  read once per connection for the tab's tooltip. */
	let algorithms = $state<Record<string, NegotiatedAlgorithms>>({});
	const asked = new Set<string>();
	$effect(() => {
		for (const tab of tabs) {
			const id = tab.connectionId;
			if (tab.type !== 'ssh' || !id || tab.link || asked.has(id)) continue;
			asked.add(id);
			sshConnectionAlgorithms(id)
				.then((a) => { if (a) algorithms[id] = a; })
				.catch(() => asked.delete(id));
		}
	});

	/** One direction's name, or both when they differ (out / in). */
	function bothWays(out: string, inn: string): string {
		return out === inn ? out : `${out} / ${inn}`;
	}

	/** How each VNC tab's connection is protected, read when the pointer
	 *  comes over it (it can connect, drop and reconnect meanwhile). */
	let vncSecurityByTab = $state<Record<string, VncSecurity | null>>({});
	function readVncSecurity(id: string): void {
		vncSecurity(id)
			.then((s) => (vncSecurityByTab[id] = s))
			.catch(() => (vncSecurityByTab[id] = null));
	}
	function vncSecurityLine(s: VncSecurity): string {
		if (s.kind === 'tls' || s.kind === 'vencrypt') {
			const cipher = [s.kind === 'vencrypt' ? 'VeNCrypt' : null, s.protocol, s.cipher, s.bits ? t('tabs.vnc_bits', { bits: s.bits }) : null].filter(Boolean).join(', ');
			return t('tabs.vnc_tls', { cipher });
		}
		return s.kind === 'ssh' ? t('tabs.vnc_ssh') : s.kind === 'proxmox' ? t('tabs.vnc_proxmox') : t('tabs.vnc_plain');
	}

	/** The tab's title, then, on a connected SSH tab, the algorithms in use;
	 *  on a VNC tab, its encryption. */
	function tabTooltip(tab: Tab): string {
		const v = tab.type === 'vnc' ? vncSecurityByTab[tab.id] : undefined;
		if (v) return `${tab.title}\n\n${vncSecurityLine(v)}`;
		const a = tab.type === 'ssh' && tab.connectionId && !tab.link ? algorithms[tab.connectionId] : undefined;
		if (!a) return tab.title;
		const mac = a.mac_out || a.mac_in ? bothWays(a.mac_out, a.mac_in) : t('tabs.algo_mac_aead');
		return [
			tab.title,
			'',
			t('tabs.algo_kex', { name: a.kex }),
			t('tabs.algo_host_key', { name: a.host_key }),
			t('tabs.algo_cipher', { name: a.cipher }),
			t('tabs.algo_mac', { name: mac }),
			t('tabs.algo_compression', { name: bothWays(a.compression_out, a.compression_in) })
		].join('\n');
	}
	/** Saved split layouts: the menu of the button beside +, and the dialog to save and manage them. */
	let layoutMenu = $state<{ x: number; y: number } | null>(null);
	let layoutsOpen = $state(false);
	const LAYOUT_ICON = 'M3 4h18v16H3zM12 4v16M12 12h9';

	function layoutItems(): MenuItem[] {
		const items: MenuItem[] = getLayouts().map((l) => ({ label: l.name, icon: LAYOUT_ICON, action: () => openLayout(l.id) }));
		if (items.length) items.push({ label: '', separator: true });
		items.push({ label: t('layouts.manage'), action: () => (layoutsOpen = true) });
		return items;
	}
	/** A drag ended on this tab's own pointerup: that is not a click. */
	let swallowClick = false;
	const isMac = typeof navigator !== 'undefined' && /Mac/.test(navigator.platform);
	const mod = isMac ? '⌘⇧' : 'Ctrl+Shift+';

	function beginDrag(e: PointerEvent, id: string): void {
		if (e.button !== 0 || (e.target as HTMLElement).closest('button')) return;
		const x0 = e.clientX;
		const y0 = e.clientY;
		const move = (ev: PointerEvent) => {
			if (!getDraggedTab() && Math.hypot(ev.clientX - x0, ev.clientY - y0) > 6) {
				setDraggedTab(id);
				swallowClick = true;
			}
		};
		const up = () => {
			window.removeEventListener('pointermove', move);
			window.removeEventListener('pointerup', up);
		};
		window.addEventListener('pointermove', move);
		window.addEventListener('pointerup', up);
	}

	function onTabClick(id: string): void {
		if (swallowClick) {
			swallowClick = false;
			return;
		}
		activateTab(id);
	}

	function inSplit(id: string): boolean {
		return contains(split, id);
	}

	function menuItems(id: string): MenuItem[] {
		// On a phone or tablet: nothing to split on a phone's narrow screen, and
		// no local terminal to pair a lone tab with.
		const noSplit = tabs.length < 2 && (mobile || id !== activeTab?.id);
		const tab = tabs.find((x) => x.id === id);
		const items: MenuItem[] = sessionItems(tab);
		if (!(mobile && window.innerWidth < SPLIT_MIN_WIDTH)) items.push(
			{ label: t('split.right'), hint: `${mod}E`, disabled: noSplit, action: () => splitWith(id, 'right') },
			{ label: t('split.down'), hint: `${mod}O`, disabled: noSplit, action: () => splitWith(id, 'down') }
		);
		if (split && contains(split, id)) items.push({ label: t('split.remove'), action: () => removeFromSplit(id) });
		if (split) items.push({ label: t('split.unsplit'), hint: `${mod}U`, action: unsplit });
		if (split && !floating) items.push({ label: t('layouts.save_menu'), action: () => (layoutsOpen = true) });

		// Groups: a new one, any other there is, or out of the one it is in.
		items.push(...(items.length && !items[items.length - 1].separator ? [{ label: '', separator: true }] : []), { label: t('tabs.group_new'), icon: GROUP_ICON, action: () => startGroupFrom(id) });
		for (const g of groups) {
			if (g.id === tab?.groupId) continue;
			items.push({ label: t('tabs.group_add', { name: g.name }), icon: DOT_ICON, iconColor: groupColor(g.color), action: () => addToGroup(id, g.id) });
		}
		if (tab?.groupId) items.push({ label: t('tabs.group_remove'), action: () => removeFromGroup(id) });

		items.push(...broadcastItems(tab));
		items.push(...windowItems(id));
		if (tab && !isDesktopTab(tab) && tab.type !== 'ftp' && tab.type !== 'bmc' && tab.type !== 'proxmox' && tab.type !== 's3') {
			items.push(
				{ label: '', separator: true },
				{ label: t('history.picker_menu'), hint: actionLabel('terminal.commandPicker'), action: () => openTabCommandPicker(tab) },
				{ label: t('history.search_menu'), hint: `${mod}H`, action: () => openTabHistory(tab) }
			);
		}

		// Duplicate and close, as a browser's tab menu has them (#160).
		items.push(
			{ label: '', separator: true },
			{ label: t('tabs.duplicate'), disabled: !canDuplicate(tab), action: () => duplicate(id) },
			...(tab && tmuxOf(tab) ? [{ label: t('tmux.switch'), disabled: !tab.connectionId || tab.link === 'down', action: () => void openSwitch(tab) }] : []),
			...(isTmuxViewTab(tab) ? [{ label: t('tmux.windows_menu'), action: () => void openWindows(tab) }] : []),
			...(isTmuxViewTab(tab) ? [{ label: t('tmux.window_new_tab'), action: () => void windowTab(tab, 'new') }] : []),
			...(tab && tmuxOf(tab) ? [{ label: t('tmux.kill'), disabled: !tab.connectionId || tab.link === 'down', action: () => (confirmKill = tab) }] : []),
			{ label: '', separator: true },
			{ label: t('common.close'), action: () => closeTab(id) },
			{ label: t('tabs.close_others'), disabled: tabsBeside(id, 'others').length === 0, action: () => requestCloseBeside(id, 'others') },
			{ label: t('tabs.close_left'), disabled: tabsBeside(id, 'left').length === 0, action: () => requestCloseBeside(id, 'left') },
			{ label: t('tabs.close_right'), disabled: tabsBeside(id, 'right').length === 0, action: () => requestCloseBeside(id, 'right') }
		);
		return items;
	}

	/** The host a tab is connected to; none for a local shell or a serial port. */
	function hostOf(tab: Tab): string | null {
		return tab.sshConnectParams?.host ?? tab.telnetConnectParams?.host ?? tab.ftpConnectParams?.host ?? tab.s3ConnectParams?.host ?? tab.bmcConnectParams?.host ?? tab.proxmoxConnectParams?.host ?? tab.rdpConnectParams?.host ?? tab.vncConnectParams?.host ?? tab.spiceConnectParams?.host ?? null;
	}

	/** A terminal connects again at any time (a hung connection, say); a
	 *  remote desktop once it has closed, as a running one would be logged
	 *  off by a second login. */
	function canReconnect(tab: Tab): boolean {
		if (tab.type === 'ssh') return !!tab.sshConnectParams;
		if (tab.type === 'telnet') return !!tab.telnetConnectParams;
		if (tab.type === 'serial') return !!tab.serialConnectParams;
		if (tab.type === 'ftp') return !!tab.ftpConnectParams;
		if (tab.type === 's3') return !!tab.s3ConnectParams;
		if (tab.type === 'bmc') return !!tab.bmcConnectParams;
		if (tab.type === 'proxmox') return !!tab.proxmoxConnectParams;
		if (isDesktopTab(tab)) return tab.link === 'down';
		return false;
	}

	function reconnect(tab: Tab): void {
		tab.reconnect = (tab.reconnect ?? 0) + 1;
		activateTab(tab.id);
	}

	async function copyHost(host: string): Promise<void> {
		try {
			await writeText(host);
			addToast(t('tabs.host_copied', { host }), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	/** The tab's own session: connect again, its address, and the saved
	 *  session it came from (shown in the list, or edited). */
	function sessionItems(tab: Tab | undefined): MenuItem[] {
		if (!tab) return [];
		const items: MenuItem[] = [];
		if (tab.type !== 'local') {
			items.push({ label: t('terminal.reconnect'), disabled: !canReconnect(tab) || tab.link === 'reconnecting', action: () => reconnect(tab) });
		}
		const host = hostOf(tab);
		if (host) items.push({ label: t('tabs.copy_host'), action: () => void copyHost(host) });
		if (canZmodem(tab.type) && !isMobile()) {
			const down = tab.link === 'down';
			items.push(
				{ label: t('zmodem.send_files'), disabled: down, action: () => requestZmodem(tab.id, 'files') },
				{ label: t('zmodem.send_folder'), disabled: down, action: () => requestZmodem(tab.id, 'folder') },
				{ label: t('zmodem.download_folder'), disabled: down, action: () => requestZmodem(tab.id, 'download-folder') }
			);
		}
		// A screenshot of the pane (a remote desktop has no terminal bar).
		if (canCapture(tab.id)) {
			const name = tab.sessionName || tab.title;
			items.push(
				{ label: t('capture.copy'), icon: CAMERA_ICON, hint: actionLabel('capture.copy'), action: () => void captureTab(tab.id, 'copy', name) },
				{ label: t('capture.save'), hint: actionLabel('capture.save'), action: () => void captureTab(tab.id, 'save', name) }
			);
		}
		// Keys this computer keeps for itself, sent to a remote desktop.
		const keysReady = sendKeysReady(tab.id);
		if (keysReady !== null) {
			for (const combo of KEY_COMBOS.filter((c) => TAB_MENU_COMBOS.includes(c.id))) {
				items.push({ label: t('rdp.send_combo', { keys: combo.label }), disabled: !keysReady, action: () => sendKeys(tab.id, combo) });
			}
		}
		const sessionId = tab.sessionId;
		// The session list is in the main window.
		if (sessionId && !floating) {
			items.push(
				{ label: t('tabs.locate_session'), action: () => requestSessionAction({ kind: 'locate', sessionId }) },
				{ label: t('tabs.edit_session'), action: () => requestSessionAction({ kind: 'edit', sessionId }) }
			);
		}
		if (items.length) items.push({ label: '', separator: true });
		return items;
	}

	const tmuxOf = tmuxSessionOf;

	/** Switch tmux session: the tab, the server's sessions, a new name. */
	let switchTab = $state<Tab | null>(null);
	let switchList = $state<TmuxListed[] | null>(null);
	let switchNew = $state('');

	async function openSwitch(tab: Tab): Promise<void> {
		if (!tab.connectionId) return;
		switchTab = tab;
		switchList = null;
		switchNew = '';
		try {
			const list = await sshTmuxList(tab.connectionId, tab.sshConnectParams?.tmux?.tool);
			if (switchTab === tab) switchList = list;
		} catch (err) {
			switchTab = null;
			addToast(String(err), 'error');
		}
	}

	function switchTo(name: string): void {
		const tab = switchTab;
		switchTab = null;
		if (tab) switchTmuxSession(tab, name);
	}

	/** tmux windows: the tab's session's windows, to switch, open in a tab,
	 *  rename or close (see #lib/terminal/tmux-tabs). */
	let windowsTab = $state<Tab | null>(null);
	let windowsList = $state<TmuxWindow[] | null>(null);
	let winRename = $state<{ id: string; name: string } | null>(null);
	let closing = $state<string | null>(null);

	async function loadWindows(tab: Tab): Promise<void> {
		const start = tab.sshConnectParams?.tmux;
		if (!tab.connectionId || !start) return;
		try {
			const list = await sshTmuxWindows(tab.connectionId, start);
			if (windowsTab === tab) windowsList = list;
		} catch (err) {
			windowsTab = null;
			addToast(t('tmux.window_failed', { error: String(err) }), 'error');
		}
	}

	async function openWindows(tab: Tab): Promise<void> {
		windowsTab = tab;
		windowsList = null;
		winRename = null;
		closing = null;
		await loadWindows(tab);
	}

	/** Something done to a window of the open dialog's tab; the list is read again. */
	async function windowOp(op: TmuxWindowOp, thenClose = false): Promise<void> {
		const tab = windowsTab;
		const start = tab?.sshConnectParams?.tmux;
		if (!tab?.connectionId || !start) return;
		try {
			await sshTmuxWindow(tab.connectionId, start, op, tab.sshConnectParams?.shell);
		} catch (err) {
			addToast(t('tmux.window_failed', { error: String(err) }), 'error');
		}
		if (thenClose) windowsTab = null;
		else await loadWindows(tab);
	}

	function windowTab(tab: Tab, window: string): void {
		windowsTab = null;
		openTmuxWindowTab(tab, window).catch((err) => addToast(t('tmux.window_failed', { error: String(err) }), 'error'));
	}

	/** A tab whose tmux session is about to be ended for good. */
	let confirmKill = $state<Tab | null>(null);

	async function killTmux(tab: Tab): Promise<void> {
		const name = tmuxOf(tab);
		if (!name || !tab.connectionId) return;
		try {
			// Its tmux client then exits, which ends the tab's session as an
			// exit typed would (and closes it, if Settings say so).
			await sshTmuxKill(tab.connectionId, name, tab.sshConnectParams?.tmux?.tool);
		} catch (err) {
			addToast(t('tmux.kill_failed', { error: String(err) }), 'error');
		}
	}

	function duplicate(id: string): void {
		duplicateTab(id).catch((err) => addToast(t('tabs.duplicate_failed', { error: String(err) }), 'error'));
	}

	let pendingClose = $derived(getPendingClose());

	// ---- Tab groups ---------------------------------------------------------

	let groups = $derived(getGroups());
	let groupMenu = $state<{ x: number; y: number; id: string } | null>(null);
	let renaming = $state<string | null>(null);
	let renameValue = $state('');
	let confirmClose = $state<TabGroup | null>(null);

	const FOLDER_ICON = 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z';
	/** A rack server with a pulse: a server management session. */
	/** Stacked boxes: a hypervisor's guests (a Proxmox session). */
	const PROXMOX_ICON = 'M3 5h18v4H3zM3 11h8v8H3zM13 11h8v8h-8z';
	/** A bucket: an S3 session. */
	const BUCKET_ICON = 'M4 6c0-1.1 3.6-2 8-2s8 .9 8 2-3.6 2-8 2-8-.9-8-2zM4 6l2 13c.2 1.1 2.9 2 6 2s5.8-.9 6-2l2-13';
	const BMC_ICON = 'M4 4h16v6H4zM4 14h16v6H4zM7 7h.01M7 17h.01M11 17h2l1-2 2 4 1-2h1';
	const GROUP_ICON = 'M4 6h7v5H4zM13 6h7v5h-7zM4 13h16v5H4z';
	const DOT_ICON = 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z';
	const HEX: Record<GroupColor, string> = {
		blue: '#3b82f6',
		green: '#22c55e',
		yellow: '#eab308',
		red: '#ef4444',
		purple: '#a855f7',
		cyan: '#06b6d4',
		orange: '#f97316',
		pink: '#ec4899',
		grey: '#9ca3af'
	};

	function groupColor(c: GroupColor): string {
		return HEX[c];
	}

	/** First tab of its group in the bar: the group's label goes before it. */
	function startsGroup(i: number): boolean {
		const g = tabs[i].groupId;
		return !!g && (i === 0 || tabs[i - 1].groupId !== g);
	}

	function startGroupFrom(tabId: string): void {
		const g = createGroup(tabId);
		if (g) beginRename(g);
	}

	function beginRename(g: TabGroup): void {
		renaming = g.id;
		renameValue = g.name;
	}

	function finishRename(): void {
		if (renaming) renameGroup(renaming, renameValue);
		renaming = null;
	}

	function groupMenuItems(id: string): MenuItem[] {
		const g = groups.find((x) => x.id === id);
		if (!g) return [];
		const count = tabs.filter((x) => x.groupId === id).length;
		return [
			{ label: g.collapsed ? t('tabs.group_expand') : t('tabs.group_collapse'), action: () => toggleGroup(id) },
			{ label: t('tabs.group_rename'), action: () => beginRename(g) },
			{ label: t('broadcast.group'), icon: BROADCAST_ICON, disabled: !tabs.some((x) => x.groupId === id && isTerminalTab(x)), action: () => broadcastToAll(id) },
			{ label: '', separator: true },
			...GROUP_COLORS.map((c) => ({
				label: t(`tabs.color_${c}`),
				icon: DOT_ICON,
				iconColor: HEX[c],
				checked: g.color === c,
				action: () => setGroupColor(id, c)
			})),
			{ label: '', separator: true },
			{ label: t('tabs.group_ungroup'), action: () => ungroup(id) },
			{
				label: t('tabs.group_close'),
				danger: true,
				action: () => (count > 1 ? (confirmClose = g) : closeGroup(id))
			}
		];
	}


	// SVG icon paths
	const terminalIcon = 'M4 17l6-5-6-5M12 19h8';
	/** A monitor: the tab shows a desktop, not a shell. */
	const rdpIcon = 'M4 5h16v11H4zM2 19h20M9 16v3M15 16v3';
	const sshIcon = 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 17.93c-3.95-.49-7-3.85-7-7.93 0-.62.08-1.21.21-1.79L9 15v1c0 1.1.9 2 2 2v1.93zm6.9-2.54c-.26-.81-1-1.39-1.9-1.39h-1v-3c0-.55-.45-1-1-1H8v-2h2c.55 0 1-.45 1-1V7h2c1.1 0 2-.9 2-2v-.41c2.93 1.19 5 4.06 5 7.41 0 2.08-.8 3.97-2.1 5.39z';
</script>

<nav class="tabbar">
	<!-- The DevOps workspaces (Ansible, Kubernetes...) used to be pages picked
	     here; they are tabs now, opened from the Sessions list (see
	     #lib/state/navigation). -->
	<div class="tabs-lead" class:floating></div>
	<div class="tabs-scroll">
		{#each tabs as tab, i (tab.id)}
			{#if startsGroup(i)}
				{@const g = groups.find((x) => x.id === tab.groupId)}
				{#if g}
					{#if renaming === g.id}
						<!-- svelte-ignore a11y_autofocus -->
						<input
							class="group-chip group-rename"
							style:--group={groupColor(g.color)}
							bind:value={renameValue}
							aria-label={t('tabs.group_name')}
							autofocus
							onblur={finishRename}
							onkeydown={(e) => { if (e.key === 'Enter') finishRename(); else if (e.key === 'Escape') renaming = null; }}
						/>
					{:else}
						<button
							class="group-chip"
							class:collapsed={g.collapsed}
							style:--group={groupColor(g.color)}
							aria-expanded={!g.collapsed}
							title={g.collapsed ? t('tabs.group_expand') : t('tabs.group_collapse')}
							onclick={() => toggleGroup(g.id)}
							ondblclick={() => beginRename(g)}
							oncontextmenu={(e) => { e.preventDefault(); groupMenu = { x: e.clientX, y: e.clientY, id: g.id }; }}
						>
							<span class="group-dot"></span>
							<span class="group-name">{g.name}</span>
							{#if g.collapsed}<span class="group-count">{tabs.filter((x) => x.groupId === g.id).length}</span>{/if}
						</button>
					{/if}
				{/if}
			{/if}
			{#if !groupHidden(tab)}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div
				class="tab"
				data-tab-id={tab.id}
				class:grouped={!!tab.groupId}
				style:--group={tab.groupId ? groupColor(groups.find((x) => x.id === tab.groupId)?.color ?? 'grey') : undefined}
				class:active={tab.id === activeTab?.id}
				class:in-split={inSplit(tab.id)}
				class:broadcast={broadcastTargets.includes(tab.id)}
				class:dragging={dragged === tab.id}
				onclick={() => onTabClick(tab.id)}
				onpointerdown={(e) => beginDrag(e, tab.id)}
				oncontextmenu={(e) => {
					e.preventDefault();
					menu = { x: e.clientX, y: e.clientY, id: tab.id };
					void otherWindows().then((w) => (windows = w));
				}}
				onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') activateTab(tab.id); }}
				role="tab"
				tabindex="0"
				aria-selected={tab.id === activeTab?.id}
				onpointerenter={() => { if (tab.type === 'vnc') readVncSecurity(tab.id); }}
				title={tabTooltip(tab)}
			>
				{#if tab.type === 'ssh' && tab.detectedOs}
					<DistroIcon osId={tab.detectedOs} size={14} />
				{:else}
					<svg class="tab-icon" width="14" height="14" viewBox="0 0 24 24" fill="none">
						{#if tab.type === 'local'}
							<path d={terminalIcon} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
						{:else if isDesktopTab(tab)}
							<path d={rdpIcon} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
						{:else if tab.type === 'proxmox'}
							<path d={PROXMOX_ICON} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
						{:else if tab.type === 'bmc'}
							<path d={BMC_ICON} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
						{:else if tab.type === 's3'}
							<path d={BUCKET_ICON} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
						{:else if tab.type === 'ftp'}
							<path d={FOLDER_ICON} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
						{:else if tab.type === 'devops'}
							<path d={DEVOPS_TOOLS.find((d) => d.id === tab.devopsTool)?.icon ?? terminalIcon} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
						{:else}
							<path d={sshIcon} fill="currentColor" />
						{/if}
					</svg>
				{/if}

				<span class="tab-title">{tab.sessionName || tab.title}{#if tab.tmuxWindow?.name}<span class="tab-tmux-window"> · {tab.tmuxWindow.name}</span>{/if}</span>
				{#if broadcastTargets.includes(tab.id)}
					<!-- Typing into one of the marked tabs goes to all of them. -->
					<span class="mark-tip" title={t('broadcast.tab_mark')}><svg class="broadcast-mark" width="13" height="13" viewBox="0 0 24 24" fill="none" role="img" aria-label={t('broadcast.tab_mark')}>
						<path d={BROADCAST_ICON} stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" />
					</svg></span>
				{/if}
				{#if tab.link}
					<!-- Seen from any tab: this session dropped, or is coming back. -->
					<span
						class="link-mark"
						class:reconnecting={tab.link === 'reconnecting'}
						role="img"
						aria-label={tab.link === 'reconnecting' ? t('terminal.reconnecting') : t('tabs.disconnected')}
						title={tab.link === 'reconnecting' ? t('terminal.reconnecting') : t('tabs.disconnected')}
					></span>
				{/if}

				{#if mcpStatus.enabled && shareIdOf(tab)}
					{@const sid = shareIdOf(tab) as string}
					{@const on = mcp.isShared(sid)}
					<button
						class="tab-share"
						class:shared={on}
						onclick={(e) => toggleShare(e, tab)}
						aria-pressed={on}
						title={on ? t('mcp.unshare_tab') : t('mcp.share_tab')}
						aria-label={on ? t('mcp.unshare_tab') : t('mcp.share_tab')}
					>
						<svg width="15" height="15" viewBox="0 0 24 24" fill="none" aria-hidden="true">
							{#if on}
								<!-- Live: a filled eye. Reads as "something is watching this" from
								     across the room, which is the whole point of this control. -->
								<path d="M2 12s3.6-6.5 10-6.5S22 12 22 12s-3.6 6.5-10 6.5S2 12 2 12z" fill="currentColor" opacity="0.9" />
								<circle cx="12" cy="12" r="2.6" fill="var(--color-bg-primary)" />
							{:else}
								<path d="M2 12s3.6-6.5 10-6.5S22 12 22 12s-3.6 6.5-10 6.5S2 12 2 12z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" />
								<circle cx="12" cy="12" r="2.8" stroke="currentColor" stroke-width="1.8" />
							{/if}
						</svg>
					</button>
				{/if}

				<button
					class="tab-close"
					onclick={(e) => handleCloseTab(e, tab.id)}
					aria-label={t('common.close_tab', { title: tab.sessionName || tab.title })}
				>
					<svg width="8" height="8" viewBox="0 0 8 8" fill="none">
						<path d="M1 1l6 6M7 1L1 7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
					</svg>
				</button>
			</div>
			{/if}
		{/each}
	</div>

	{#if !mobile && !floating}
		<button
			class="new-tab-btn"
			onclick={(e) => {
				const r = e.currentTarget.getBoundingClientRect();
				layoutMenu = { x: r.left, y: r.bottom + 4 };
			}}
			title={t('layouts.title')}
			aria-label={t('layouts.title')}
			aria-haspopup="menu"
		>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d={LAYOUT_ICON} stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" /></svg>
		</button>
		<button class="new-tab-btn" onclick={handleNewTab} title={newTabTip} aria-label={newTabTip}>
			<svg width="14" height="14" viewBox="0 0 14 14" fill="none">
				<path d="M7 1v12M1 7h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
			</svg>
		</button>
	{/if}
</nav>

{#if menu}
	<DbMenu x={menu.x} y={menu.y} items={menuItems(menu.id)} onclose={() => (menu = null)} />
{/if}
{#if layoutMenu}
	<DbMenu x={layoutMenu.x} y={layoutMenu.y} items={layoutItems()} onclose={() => (layoutMenu = null)} />
{/if}
<LayoutsDialog open={layoutsOpen} onclose={() => (layoutsOpen = false)} />
{#if groupMenu}
	<DbMenu x={groupMenu.x} y={groupMenu.y} items={groupMenuItems(groupMenu.id)} onclose={() => (groupMenu = null)} />
{/if}

<Modal open={confirmClose !== null} onclose={() => (confirmClose = null)} title={t('tabs.group_close')} maxWidth="420px">
	{#if confirmClose}
		<p class="confirm-text">
			{t('tabs.group_close_confirm', { name: confirmClose.name, count: String(tabs.filter((x) => x.groupId === confirmClose?.id).length) })}
		</p>
	{/if}
	{#snippet actions()}
		<Button variant="secondary" onclick={() => (confirmClose = null)}>{t('common.cancel')}</Button>
		<Button
			variant="danger"
			onclick={() => {
				if (confirmClose) closeGroup(confirmClose.id);
				confirmClose = null;
			}}>{t('tabs.group_close')}</Button
		>
	{/snippet}
</Modal>

<Modal open={switchTab !== null} onclose={() => (switchTab = null)} title={t('tmux.switch')} maxWidth="440px">
	{#if switchTab}
		{#if switchList === null}
			<p class="confirm-text">{t('tmux.listing')}</p>
		{:else}
			<div class="tmux-switch">
				{#each switchList as s (s.name)}
					<button type="button" class="tmux-switch-row" class:current={s.name === tmuxOf(switchTab)} disabled={!validTmuxName(s.name)} onclick={() => switchTo(s.name)}>
						<span class="tmux-switch-name">{s.name}</span>
						<span class="tmux-switch-meta">{listedMeta(s)}</span>
					</button>
				{/each}
			</div>
			<form class="tmux-switch-new" onsubmit={(e) => { e.preventDefault(); if (switchNew.trim()) switchTo(switchNew.trim()); }}>
				<input class="tmux-switch-input" bind:value={switchNew} placeholder={t('tmux.new_session')} spellcheck="false" autocomplete="off" />
				<Button variant="primary" type="submit" disabled={!switchNew.trim()}>{t('tmux.start_new')}</Button>
			</form>
		{/if}
	{/if}
</Modal>

<Modal open={windowsTab !== null} onclose={() => (windowsTab = null)} title={t('tmux.windows_title')} maxWidth="480px">
	{#if windowsTab}
		{#if windowsList === null}
			<p class="confirm-text">{t('tmux.listing')}</p>
		{:else}
			<div class="tmux-switch">
				{#if windowsList.length === 0}
					<p class="confirm-text">{t('tmux.no_windows')}</p>
				{/if}
				{#each windowsList as w (w.id)}
					<div class="tmux-window-row" class:current={w.active}>
						{#if winRename?.id === w.id}
							<form class="tmux-switch-new" onsubmit={(e) => { e.preventDefault(); const r = winRename; winRename = null; if (r && r.name.trim()) void windowOp({ kind: 'rename', window: w.id, name: r.name.trim() }); }}>
								<!-- svelte-ignore a11y_autofocus -->
								<input class="tmux-switch-input" bind:value={winRename.name} spellcheck="false" autocomplete="off" autofocus />
								<Button variant="primary" type="submit">{t('tmux.window_rename')}</Button>
							</form>
						{:else}
							<button type="button" class="tmux-switch-row" class:current={w.active} onclick={() => void windowOp({ kind: 'select', window: w.id }, true)}>
								<span class="tmux-switch-name">{w.index}: {w.name}</span>
								<span class="tmux-switch-meta">{w.command}{#if w.panes > 1} · {t('tmux.window_panes', { count: String(w.panes) })}{/if}</span>
							</button>
							<div class="tmux-window-actions">
								<button type="button" class="tmux-window-action" disabled={w.active} onclick={() => windowsTab && windowTab(windowsTab, w.id)}>{t('tmux.window_open_tab')}</button>
								<button type="button" class="tmux-window-action" onclick={() => (winRename = { id: w.id, name: w.name })}>{t('tmux.window_rename')}</button>
								<button type="button" class="tmux-window-action danger" onclick={() => { if (closing === w.id) { closing = null; void windowOp({ kind: 'close', window: w.id }); } else closing = w.id; }}>{closing === w.id ? t('tmux.window_close_confirm') : t('tmux.window_close')}</button>
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	{/if}
	{#snippet actions()}
		<Button variant="secondary" disabled={windowsList === null} onclick={() => windowsTab && windowTab(windowsTab, 'new')}>{t('tmux.window_new_tab')}</Button>
		<Button variant="primary" disabled={windowsList === null} onclick={() => void windowOp({ kind: 'new' }, true)}>{t('tmux.window_new')}</Button>
	{/snippet}
</Modal>

<Modal open={confirmKill !== null} onclose={() => (confirmKill = null)} title={t('tmux.kill')} maxWidth="420px">
	{#if confirmKill}
		<p class="confirm-text">{t('tmux.kill_confirm', { name: tmuxOf(confirmKill) ?? '' })}</p>
	{/if}
	{#snippet actions()}
		<Button variant="secondary" onclick={() => (confirmKill = null)}>{t('common.cancel')}</Button>
		<Button
			variant="danger"
			onclick={() => {
				if (confirmKill) void killTmux(confirmKill);
				confirmKill = null;
			}}>{t('tmux.kill')}</Button
		>
	{/snippet}
</Modal>

<Modal open={pendingClose !== null} onclose={cancelPendingClose} title={t('tabs.close_many_title')} maxWidth="420px">
	{#if pendingClose}
		<p class="confirm-text">{t('tabs.close_many_confirm', { count: String(pendingClose.ids.length) })}</p>
	{/if}
	{#snippet actions()}
		<Button variant="secondary" onclick={cancelPendingClose}>{t('common.cancel')}</Button>
		<Button variant="danger" onclick={confirmPendingClose}>{t('tabs.close_many_title')}</Button>
	{/snippet}
</Modal>

<style>
	/* ---- Tab groups ---- */

	.group-chip {
		display: flex;
		align-items: center;
		gap: 6px;
		flex-shrink: 0;
		align-self: center;
		max-width: 140px;
		height: 22px;
		margin: 0 2px 0 6px;
		padding: 0 8px;
		border: none;
		border-radius: 999px;
		background: color-mix(in srgb, var(--group) 18%, transparent);
		color: var(--group);
		font-family: var(--font-sans);
		font-size: var(--text-2xs);
		font-weight: 600;
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.group-chip:hover {
		background: color-mix(in srgb, var(--group) 28%, transparent);
	}

	/* Folded, the chip stands for its tabs: solid, with how many. */
	.group-chip.collapsed {
		background: var(--group);
		color: var(--color-bg-primary);
	}

	.group-dot {
		flex-shrink: 0;
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: currentColor;
	}

	.group-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.group-count {
		font-variant-numeric: tabular-nums;
		opacity: 0.85;
	}

	.group-rename {
		width: 120px;
		outline: 1px solid var(--group);
		cursor: text;
	}

	/* A grouped tab carries the group's colour along its top edge; the bottom
	   one is the selected tab's. */
	.tab.grouped {
		box-shadow: inset 0 2px 0 color-mix(in srgb, var(--group) 80%, transparent);
		background-color: color-mix(in srgb, var(--group) 5%, transparent);
	}

	.tmux-switch {
		display: flex;
		flex-direction: column;
		gap: 4px;
		max-height: 280px;
		overflow-y: auto;
		margin-bottom: 12px;
	}

	.tmux-switch-row {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 2px;
		padding: 8px 10px;
		font: inherit;
		text-align: left;
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	.tmux-switch-row:hover:not(:disabled),
	.tmux-switch-row.current {
		border-color: var(--color-accent);
	}

	.tmux-switch-row:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.tmux-switch-name {
		font-family: var(--font-mono, monospace);
		font-size: var(--text-sm);
	}

	.tmux-switch-meta {
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.tab-tmux-window {
		opacity: 0.7;
	}

	.tmux-window-row {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding-bottom: 4px;
		border-bottom: 1px solid var(--color-border);
	}

	.tmux-window-row:last-child {
		border-bottom: none;
	}

	.tmux-window-actions {
		display: flex;
		gap: 4px;
		padding: 0 8px;
	}

	.tmux-window-action {
		border: none;
		background: none;
		padding: 2px 6px;
		border-radius: var(--radius-sm, 4px);
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.tmux-window-action:hover:not(:disabled) {
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
	}

	.tmux-window-action:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.tmux-window-action.danger {
		color: var(--color-danger);
	}

	.tmux-switch-new {
		display: flex;
		gap: 8px;
	}

	.tmux-switch-input {
		flex: 1;
		min-width: 0;
		padding: 6px 10px;
		font: inherit;
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
	}

	.confirm-text {
		margin: 0;
		font-size: var(--text-sm);
		line-height: 1.5;
		color: var(--color-text-primary);
	}

	.tabbar {
		display: flex;
		align-items: stretch;
		height: var(--tabbar-h, 36px);
		min-height: var(--tabbar-h, 36px);
		/* The bar is the sidebar's shade and the active tab the content's, so
		   the open tab reads as the front of the page below it. */
		background-color: var(--color-bg-secondary);
		border-bottom: 1px solid var(--color-border);
		user-select: none;
	}

	/* A little room before the first tab, none in a floating window. */
	.tabs-lead {
		width: 6px;
		flex-shrink: 0;
	}

	.tabs-lead.floating {
		display: none;
	}

	/* Scrolls its tabs: without min-width 0 it grew to fit them instead and
	   made the whole window wider than a phone's screen. */
	.tabs-scroll {
		display: flex;
		flex: 1;
		min-width: 0;
		overflow-x: auto;
		overflow-y: hidden;
		scrollbar-width: none;
	}

	.tabs-scroll::-webkit-scrollbar {
		display: none;
	}

	.tab {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 120px;
		max-width: 200px;
		padding: 0 8px 0 12px;
		border: none;
		border-right: 1px solid var(--color-border);
		border-bottom: 2px solid transparent;
		background: transparent;
		color: var(--color-text-secondary);
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		cursor: pointer;
		white-space: nowrap;
		flex-shrink: 0;
		transition: background-color 150ms ease, color 150ms ease, border-color 150ms ease;
	}

	.tab:hover {
		background-color: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	/* Content-coloured, with an accent line on top and none below, so it
	   joins the terminal under it. The bottom edge stays free for the split
	   and broadcast marks. */
	.tab.active {
		background-color: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-weight: 500;
		box-shadow: inset 0 2px 0 var(--color-accent);
		margin-bottom: -1px;
	}

	/* The other tab on screen in a split: shown, without the keyboard. */
	.tab.in-split:not(.active) {
		color: var(--color-text-primary);
		border-bottom-color: color-mix(in srgb, var(--color-accent) 40%, transparent);
	}

	/* In a broadcast: tinted, with the waves mark, from anywhere in the bar. */
	.tab.broadcast {
		background-color: color-mix(in srgb, var(--color-warning) 14%, transparent);
		border-bottom-color: var(--color-warning);
	}

	.broadcast-mark {
		flex-shrink: 0;
		color: var(--color-warning);
	}

	.tab.dragging {
		opacity: 0.6;
		cursor: grabbing;
	}

	.tab-icon {
		flex-shrink: 0;
		opacity: 0.7;
	}

	.tab.active .tab-icon {
		opacity: 1;
	}

	/* Shape as well as colour: a solid red dot is a dropped connection, an
	   amber ring one being made again. */
	.link-mark {
		flex-shrink: 0;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--color-danger);
	}

	.link-mark.reconnecting {
		background: transparent;
		box-shadow: inset 0 0 0 1.5px var(--color-warning);
		animation: link-pulse 1.2s ease-in-out infinite;
	}

	@keyframes link-pulse {
		50% {
			opacity: 0.35;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.link-mark.reconnecting {
			animation: none;
		}
	}

	.tab-title {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		text-align: left;
	}

	/* Only rendered while the MCP server runs. When a session is shared this
	   stays visible at all times, unlike .tab-close which is hover-only: an
	   AI being able to read a terminal is a state that must never be hidden.
	   Amber rather than green, for the same reason as the settings dot. */
	.tab-share {
		display: flex;
		align-items: center;
		justify-content: center;
		flex-shrink: 0;
		width: 22px;
		height: 22px;
		padding: 0;
		border: none;
		border-radius: 50%;
		background: none;
		color: var(--color-text-tertiary);
		cursor: pointer;
		opacity: 0;
		position: relative;
	}

	.tab:hover .tab-share {
		opacity: 1;
	}

	/* Live. Deliberately loud: a filled amber eye on a tinted disc with a
	   pulsing ring, always visible and never hover-gated. Everything else on
	   a tab can afford to be subtle; a terminal an AI can read cannot be, and
	   the first attempt at 11px and hover-only was far too easy to miss. */
	.tab-share.shared {
		opacity: 1;
		color: var(--color-warning);
		background: color-mix(in srgb, var(--color-warning) 16%, transparent);
		box-shadow: 0 0 0 1px color-mix(in srgb, var(--color-warning) 45%, transparent);
	}

	.tab-share.shared::after {
		content: '';
		position: absolute;
		inset: -3px;
		border-radius: 50%;
		border: 1px solid var(--color-warning);
		animation: mcp-watch 2.4s var(--ease-default) infinite;
		pointer-events: none;
	}

	@keyframes mcp-watch {
		0% { transform: scale(0.85); opacity: 0.55; }
		70%, 100% { transform: scale(1.3); opacity: 0; }
	}

	/* The colour and ring carry the state on their own, so motion can go. */
	@media (prefers-reduced-motion: reduce) {
		.tab-share.shared::after { animation: none; opacity: 0.55; }
	}

	.tab-share:hover {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.tab-share.shared:hover {
		color: var(--color-warning);
	}

	.tab-close {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		flex-shrink: 0;
		opacity: 0;
		transition: opacity 150ms ease, background-color 150ms ease;
	}

	.tab:hover .tab-close {
		opacity: 1;
	}

	.tab-close:hover {
		background-color: var(--color-surface-active);
		color: var(--color-text-primary);
	}

	.new-tab-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 36px;
		min-width: 36px;
		border: none;
		border-left: 1px solid var(--color-border);
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition: background-color 150ms ease, color 150ms ease;
	}

	.new-tab-btn:hover {
		background-color: var(--color-bg-secondary);
		color: var(--color-text-primary);
	}

	/* 120px per tab overflows a phone at three tabs. Let them compress; the
	   label already truncates with an ellipsis. */
	@media (max-width: 600px) {
		.tab {
			min-width: 84px;
			max-width: 140px;
			padding: 0 6px 0 8px;
		}
	}

	@media (pointer: coarse) {
		.tab-close {
			width: 24px;
			height: 24px;
		}
	}

	.mark-tip {
		display: inline-flex;
		flex-shrink: 0;
	}
</style>
