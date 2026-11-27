<script lang="ts">
	import { takeTmuxTitle } from '#lib/terminal/tmux-tabs.js';
	import { getTabs, getActiveTab, updateTabTitle, updateTabConnection, setTelnetConnection, setSerialConnection, createTab, activateTab, getSplit, setSplitRatio, getDraggedTab, setDraggedTab, splitWith, unsplit, dropTab, canShowFilesView, toggleFilesView, isReleased } from '#lib/state/tabs.svelte.js';
	import { canFloat, isFloatWindow, tearOff } from '#lib/state/windows.svelte.js';
	import { geometry, paneStyle, dividerStyle, ratioAt, dropAt as dropAtPoint, leaves, neighbour, SPLIT_MIN_WIDTH, type Drop, type DividerRect, type Zone } from '#lib/terminal/split.js';
	import { getActivePage } from '#lib/state/navigation.svelte.js';
	import { isDevopsEnabled } from '#lib/state/devops.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import StartScreen from '#lib/components/layout/StartScreen.svelte';
	import { ptySpawn, ptyClose } from '#lib/ipc/pty.js';
	import { sshDisconnect } from '#lib/ipc/ssh.js';
	import { telnetDisconnect } from '#lib/ipc/telnet.js';
	import { serialDisconnect } from '#lib/ipc/serial.js';
	import { monitoringStart, monitoringStop, monitoringGetStats, monitoringStartWsl } from '#lib/ipc/monitoring.js';
	import { updateStats, removeStats } from '#lib/state/monitoring.svelte.js';
	import Terminal from '#lib/components/terminal/Terminal.svelte';
	import DualPane from '#lib/components/explorer/DualPane.svelte';
	import FtpPanel from '#lib/components/explorer/FtpPanel.svelte';
	import BmcPanel from '#lib/components/bmc/BmcPanel.svelte';
	import ProxmoxPanel from '#lib/components/proxmox/ProxmoxPanel.svelte';
	import { stopExternalEdits } from '#lib/explorer/external-edit.js';
	import { dropQueue } from '#lib/explorer/transfer-queue.svelte.js';
	import RdpPanel from '#lib/components/rdp/RdpPanel.svelte';
	import BroadcastBar from '#lib/components/terminal/BroadcastBar.svelte';
	import { getBroadcastTargets, toggleBroadcastTarget } from '#lib/state/broadcast.svelte.js';
	import MonitoringBar from '#lib/components/terminal/MonitoringBar.svelte';
	import TerminalBar from '#lib/components/terminal/TerminalBar.svelte';
	import { getSettings } from '#lib/state/settings.svelte.js';
	import { matchesAction } from '#lib/state/shortcuts.svelte.js';
	import { freshFind, pendingFind } from '#lib/state/terminal-find.svelte.js';
	import RemoteTerminal from '#lib/components/share/RemoteTerminal.svelte';
	import ShareDialog from '#lib/components/share/ShareDialog.svelte';
	import * as share from '#lib/state/share.svelte.js';
	import { monitorBarShown, monitoringWanted } from '#lib/state/monitor-bar.svelte.js';
	import { vaultState } from '#lib/state/vault.svelte.js';
	import AnsiblePage from '#lib/components/ansible/AnsiblePage.svelte';
	import TofuPage from '#lib/components/tofu/TofuPage.svelte';
	import DbPage from '#lib/components/db/DbPage.svelte';
	import ContainersPage from '#lib/components/containers/ContainersPage.svelte';
	import K8sPage from '#lib/components/k8s/K8sPage.svelte';
	import EditorWindow from '#lib/components/editor/EditorWindow.svelte';
	import RailView from '#lib/components/rdp/RailView.svelte';

	const isEditorWindow = typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('editor');
	/** A seamless RemoteApp's window of its own. */
	const isRailView = typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('railview');

	/**
	 * Handle terminal title changes from OSC 2 escape sequences.
	 * When the shell sends a title like "root@hostname: /root",
	 * extract the username and update the tab title to reflect the
	 * effective user (e.g. after sudo su -).
	 */
	function handleTerminalTitleChange(tabId: string, terminalTitle: string): void {
		const tab = getTabs().find((t) => t.id === tabId);
		if (!tab || tab.type !== 'ssh') return;
		if (takeTmuxTitle(tab, terminalTitle)) return;

		// Terminal titles from bash are typically: "user@hostname: dir"
		// Extract the username from before the first @
		const atIndex = terminalTitle.indexOf('@');
		if (atIndex <= 0) return;

		const effectiveUser = terminalTitle.substring(0, atIndex).trim();
		if (!effectiveUser) return;

		// Get the original host (IP/hostname) from the current tab title
		const currentAtIndex = tab.title.indexOf('@');
		const originalHost = currentAtIndex >= 0 ? tab.title.substring(currentAtIndex + 1) : '';
		if (!originalHost) return;

		const newTitle = `${effectiveUser}@${originalHost}`;
		if (newTitle !== tab.title) {
			updateTabTitle(tabId, newTitle);
		}
	}

	let tabs = $derived(getTabs());
	const appSettings = getSettings();
	/** The tab whose bar has its find field open: a hover-only bar stays. */
	let findOpenIn = $state<string | null>(null);
	// With the bar hidden in Settings, Find (Ctrl+Shift+F, the terminal's
	// menu) shows it for the search and hides it again when the search closes.
	let lastFindAt = 0;
	$effect(() => {
		const r = pendingFind();
		if (!r || r.at === lastFindAt) return;
		lastFindAt = r.at;
		if (freshFind(r) && getSettings().terminalBar === 'hidden') findOpenIn = r.tabId;
	});
	let activeTab = $derived(getActiveTab());
	let activePage = $derived(getActivePage());
	let broadcastTargets = $derived(getBroadcastTargets());

	// Tabs side by side, split any way (#106); the rules are in #lib/terminal/split.
	let areaEl = $state<HTMLDivElement>();
	let innerWidth = $state(1200);
	let split = $derived(getSplit());
	let shownSplit = $derived(split && innerWidth >= SPLIT_MIN_WIDTH && leaves(split).every((id) => tabs.some((x) => x.id === id)) ? split : null);
	let geo = $derived(geometry(shownSplit));
	let paneRects = $derived(new Map(geo.panes.map((p) => [p.id, p])));
	let draggedTab = $derived(getDraggedTab());
	let drop = $state<Drop | null>(null);

	function startResize(e: PointerEvent, d: DividerRect): void {
		if (!areaEl) return;
		e.preventDefault();
		const handle = e.currentTarget as HTMLElement;
		handle.setPointerCapture(e.pointerId);
		const rect = areaEl.getBoundingClientRect();
		const move = (ev: PointerEvent) => setSplitRatio(d.path, ratioAt(d, (ev.clientX - rect.left) / rect.width, (ev.clientY - rect.top) / rect.height));
		const up = () => {
			handle.removeEventListener('pointermove', move);
			handle.removeEventListener('pointerup', up);
			handle.removeEventListener('pointercancel', up);
		};
		handle.addEventListener('pointermove', move);
		handle.addEventListener('pointerup', up);
		handle.addEventListener('pointercancel', up);
	}

	function onDividerKey(e: KeyboardEvent, d: DividerRect): void {
		const step = e.shiftKey ? 0.1 : 0.02;
		const [back, fwd] = d.dir === 'row' ? ['ArrowLeft', 'ArrowRight'] : ['ArrowUp', 'ArrowDown'];
		if (e.key === back) setSplitRatio(d.path, d.ratio - step);
		else if (e.key === fwd) setSplitRatio(d.path, d.ratio + step);
		else if (e.key === 'Home') setSplitRatio(d.path, 0);
		else if (e.key === 'End') setSplitRatio(d.path, 1);
		else return;
		e.preventDefault();
	}

	const DROP_LABEL: Record<Zone, string> = {
		left: 'split.drop_left',
		right: 'split.drop_right',
		top: 'split.drop_up',
		bottom: 'split.drop_down',
		center: 'split.drop_here'
	};

	// A tab dragged from the bar or by a pane's name: where it would land,
	// shown while it moves.
	$effect(() => {
		const id = draggedTab;
		if (!id) {
			drop = null;
			return;
		}
		const move = (ev: PointerEvent) => {
			const r = areaEl?.getBoundingClientRect();
			if (!r || innerWidth < SPLIT_MIN_WIDTH) {
				drop = null;
				return;
			}
			const shown = getActiveTab()?.id;
			const d = dropAtPoint(shownSplit, shown, ev.clientX - r.left, ev.clientY - r.top, r.width, r.height);
			// Onto its own pane: nothing to do, but for the one tab on screen
			// dragged to a side (it goes there, the next tab beside it).
			drop = d && d.target === id && (d.zone === 'center' || shownSplit) ? null : d;
		};
		const up = (ev: PointerEvent) => {
			const at = drop;
			setDraggedTab(null);
			if (at) dropTab(id, at);
			// Let go outside the window: the tab gets a window of its own there.
			else if (canFloat() && outsideWindow(ev)) void tearOff(id);
		};
		const key = (ev: KeyboardEvent) => {
			if (ev.key === 'Escape') setDraggedTab(null);
		};
		window.addEventListener('pointermove', move);
		window.addEventListener('pointerup', up);
		window.addEventListener('keydown', key);
		return () => {
			window.removeEventListener('pointermove', move);
			window.removeEventListener('pointerup', up);
			window.removeEventListener('keydown', key);
		};
	});

	/** Is the pointer outside this window (a tab dragged out of it)? */
	function outsideWindow(ev: PointerEvent): boolean {
		return ev.clientX < 0 || ev.clientY < 0 || ev.clientX > window.innerWidth || ev.clientY > window.innerHeight;
	}

	// Ctrl+Shift+E splits right, Ctrl+Shift+O down (as Terminator), Ctrl+Shift+U
	// back to one, Ctrl+Shift+B in or out of the broadcast, unless changed in
	// Settings > Keyboard shortcuts. Caught before the terminal sees them.
	$effect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (activePage !== 'terminal') return;
			const right = matchesAction(e, 'split.right');
			if (right || matchesAction(e, 'split.down')) {
				const focused = activeTab?.id;
				if (!focused) return;
				e.preventDefault();
				e.stopPropagation();
				const other = neighbour(tabs.map((x) => x.id), focused, leaves(split));
				splitWith(other ?? focused, right ? 'right' : 'down');
			} else if (split && matchesAction(e, 'split.unsplit')) {
				e.preventDefault();
				e.stopPropagation();
				unsplit();
			} else if (activeTab && matchesAction(e, 'broadcast.toggle')) {
				// In or out of the broadcast: typing into several terminals at once.
				e.preventDefault();
				e.stopPropagation();
				toggleBroadcastTarget(activeTab.id);
			}
		};
		window.addEventListener('keydown', onKey, { capture: true });
		return () => window.removeEventListener('keydown', onKey, { capture: true });
	});


	let spawnedPtys = $state(new Set<string>());
	let connectedSsh = $state(new Set<string>());
	let connectedTelnet = $state(new Set<string>());
	let connectedSerial = $state(new Set<string>());
	let monitoredConnections = $state(new Set<string>());
	let pollIntervals = $state(new Map<string, ReturnType<typeof setInterval>>());

	// A local tab's PTY starts once its terminal listens, so the first
	// output (a WSL distro's banner, a prompt) is not lost.
	function handleTerminalReady(tab: (typeof tabs)[number]) {
		if (tab.type === 'local' && !spawnedPtys.has(tab.id)) {
			spawnedPtys.add(tab.id);
			// Moved here from another window: its shell is already running.
			if (tab.handoff) return;
			ptySpawn(tab.id, tab.localCommand?.program, undefined, undefined, tab.localCommand?.args).catch((err) => {
				console.error(`Failed to spawn PTY for tab ${tab.id}:`, err);
			});
		}
	}

	// Track SSH connections opened in tabs
	$effect(() => {
		for (const tab of tabs) {
			if (tab.type === 'ssh' && tab.connectionId) {
				connectedSsh.add(tab.connectionId);
			}
			if (tab.type === 'telnet' && tab.telnetConnectionId) {
				connectedTelnet.add(tab.telnetConnectionId);
			}
			if (tab.type === 'serial' && tab.serialConnectionId) {
				connectedSerial.add(tab.serialConnectionId);
			}
		}
	});

	function stopMonitoring(connId: string) {
		monitoredConnections.delete(connId);
		// Moved to another window, which watches it now.
		if (!isReleased(connId)) {
			monitoringStop(connId).catch((err) => {
				console.error(`Failed to stop monitoring for ${connId}:`, err);
			});
		}
		const interval = pollIntervals.get(connId);
		if (interval) {
			clearInterval(interval);
			pollIntervals.delete(connId);
		}
		removeStats(connId);
	}

	// Start monitoring for new SSH connections (poll-based), unless their
	// session or Settings turned it off; stop it when it is turned off.
	$effect(() => {
		for (const tab of tabs) {
			if (tab.type !== 'ssh' || !tab.connectionId) continue;
			if (!monitoringWanted(tab)) {
				if (monitoredConnections.has(tab.connectionId)) stopMonitoring(tab.connectionId);
				continue;
			}
			if (!monitoredConnections.has(tab.connectionId)) {
				const connId = tab.connectionId;
				monitoredConnections.add(connId);

				monitoringStart(connId).catch((err) => {
					console.error(`Failed to start monitoring for ${connId}:`, err);
				});

				// Poll for stats every 3 seconds
				const poll = async () => {
					try {
						const stats = await monitoringGetStats(connId);
						updateStats(connId, stats);
					} catch {
						// Stats not available yet — ignore
					}
				};
				// Initial fetch after a short delay
				setTimeout(poll, 1500);
				const interval = setInterval(poll, 3000);
				pollIntervals.set(connId, interval);
			}
		}
	});

	// WSL monitoring: only poll while the WSL tab is in view to let the WSL VM go idle
	$effect(() => {
		const isWslInView =
			activePage === 'terminal' && activeTab?.wslDistro !== undefined && activeTab?.connectionId && monitoringWanted(activeTab);
		if (isWslInView) {
			const connId = activeTab!.connectionId!;
			const distro = activeTab!.wslDistro!;

			monitoringStartWsl(connId, distro).catch((err) => {
				console.error(`Failed to start WSL monitoring for ${connId}:`, err);
			});

			const poll = async () => {
				try {
					const stats = await monitoringGetStats(connId);
					updateStats(connId, stats);
				} catch {
					// Stats not available yet — ignore
				}
			};

			const initialTimer = setTimeout(poll, 1500);
			const interval = setInterval(poll, 3000);

			return () => {
				clearTimeout(initialTimer);
				clearInterval(interval);
				monitoringStop(connId).catch((err) => {
					console.error(`Failed to stop WSL monitoring for ${connId}:`, err);
				});
			};
		}
	});


	// Clean up PTYs, SSH connections, and monitoring for closed tabs
	$effect(() => {
		const tabIds = new Set(tabs.map((t) => t.id));
		const activeConnectionIds = new Set(
			tabs.filter((t) => t.connectionId).map((t) => t.connectionId!)
		);

		// A tab moved to another window took its session there: only forgotten here.
		for (const id of spawnedPtys) {
			if (!tabIds.has(id)) {
				spawnedPtys.delete(id);
				if (isReleased(id)) continue;
				ptyClose(id).catch((err) => {
					console.error(`Failed to close PTY ${id}:`, err);
				});
			}
		}

		for (const connId of connectedSsh) {
			if (!activeConnectionIds.has(connId)) {
				connectedSsh.delete(connId);
				if (isReleased(connId)) continue;
				stopExternalEdits(connId);
				dropQueue(connId);
				sshDisconnect(connId).catch((err) => {
					console.error(`Failed to disconnect SSH ${connId}:`, err);
				});
			}
		}

		// A Telnet tab closed, or connected again under a new id.
		const openTelnet = new Set(tabs.map((t) => t.telnetConnectionId).filter(Boolean));
		for (const connId of connectedTelnet) {
			if (!openTelnet.has(connId)) {
				connectedTelnet.delete(connId);
				if (isReleased(connId)) continue;
				telnetDisconnect(connId).catch(() => {});
			}
		}

		// A serial tab closed (the port is let go), or opened again.
		const openSerial = new Set(tabs.map((t) => t.serialConnectionId).filter(Boolean));
		for (const connId of connectedSerial) {
			if (!openSerial.has(connId)) {
				connectedSerial.delete(connId);
				if (isReleased(connId)) continue;
				serialDisconnect(connId).catch(() => {});
			}
		}

		for (const connId of monitoredConnections) {
			if (!activeConnectionIds.has(connId)) stopMonitoring(connId);
		}
	});
</script>

<svelte:window bind:innerWidth />

{#if isRailView}
	<RailView />
{:else if isEditorWindow}
	<EditorWindow />
{:else}
	<div class="page-container">
		<div class="page-view active">
			{#if tabs.length === 0}
				{#if !isFloatWindow()}<StartScreen />{/if}
			{:else}
				<!-- Side by side with the other person's terminal while a share is
				     connected and they are sharing one. Left is always mine. -->
				<BroadcastBar />
				<div class="terminal-split" class:sharing={share.isShowingRemote()}>
				<div class="terminal-area" bind:this={areaEl}>
					{#each tabs as tab (tab.id)}
						{@const pane = paneRects.get(tab.id) ?? null}
						{@const focused = tab.id === activeTab?.id}
						{@const filesShown = !!tab.filesView && canShowFilesView(tab)}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							class="terminal-wrapper"
							class:active={focused || pane !== null}
							class:pane={pane !== null}
							class:focused={pane !== null && focused}
							class:broadcast={broadcastTargets.includes(tab.id)}
							style={pane ? paneStyle(pane) : undefined}
							onpointerdowncapture={() => {
								if (pane && !focused) activateTab(tab.id);
							}}
						>
							{#if tab.type === 'rdp' && tab.rdpConnectParams}
								{#key tab.reconnect}
									<RdpPanel id={tab.id} params={tab.rdpConnectParams} active={focused} />
								{/key}
							{:else if tab.type === 'vnc' && tab.vncConnectParams}
								{#key tab.reconnect}
									<RdpPanel id={tab.id} params={tab.vncConnectParams} protocol="vnc" active={focused} />
								{/key}
							{:else if tab.type === 'spice' && tab.spiceConnectParams}
								{#key tab.reconnect}
									<RdpPanel id={tab.id} params={tab.spiceConnectParams} protocol="spice" active={focused} />
								{/key}
							{:else if tab.type === 'ftp'}
								<div class="pane-body">
									<FtpPanel {tab} active={focused} />
								</div>
							{:else if tab.type === 'proxmox'}
								<div class="pane-body">
									<ProxmoxPanel {tab} active={focused} />
								</div>
							{:else if tab.type === 'bmc'}
								<div class="pane-body">
									<BmcPanel {tab} active={focused} />
								</div>
							{:else if tab.type === 'devops'}
								<!-- A DevOps workspace: the page that used to fill the window,
								     now a tab like any session's panel. Kept mounted while
								     its tab is open, so a query or a log survives a trip to
								     a terminal. -->
								<div class="pane-body devops-body">
									<!-- A workspace that fails shows why in its own tab; the
									     terminals and the other tabs carry on. -->
									<svelte:boundary onerror={(e) => console.error('DevOps workspace failed:', e)}>
									{#if tab.devopsTool && isDevopsEnabled(tab.devopsTool)}
										{#if tab.devopsTool === 'kubernetes'}
											<K8sPage {tab} />
										{:else if tab.devopsTool === 'databases'}
											<DbPage />
										{:else if tab.devopsTool === 'containers'}
											<ContainersPage {tab} />
										{:else if !vaultState.locked}
											{#key vaultState.userUuid}
												{#if tab.devopsTool === 'ansible'}<AnsiblePage />{:else}<TofuPage />{/if}
											{/key}
										{/if}
									{/if}
									{#snippet failed(error, reset)}
										<div class="devops-failed" role="alert">
											<p>{t('devops.tab_failed')}</p>
											<code>{String((error as Error)?.message ?? error)}</code>
											<button class="devops-retry" onclick={reset}>{t('devops.tab_retry')}</button>
										</div>
									{/snippet}
									</svelte:boundary>
								</div>
							{:else if tab.type !== 'rdp' && tab.type !== 'vnc' && tab.type !== 'spice'}
								{#if appSettings.terminalBar !== 'hidden' || findOpenIn === tab.id}
									<div class="bar-slot" class:hover-only={appSettings.terminalBar === 'hover' && findOpenIn !== tab.id}>
										<TerminalBar {tab} onfindopen={(open) => (findOpenIn = open ? tab.id : findOpenIn === tab.id ? null : findOpenIn)} />
									</div>
								{/if}
								<div class="pane-body">
								<Terminal
									ptyId={tab.id}
									type={tab.type}
									connectionId={tab.type === 'telnet' ? tab.telnetConnectionId : tab.type === 'serial' ? tab.serialConnectionId : tab.connectionId}
									active={focused && !filesShown}
									visible={focused || pane !== null}
									onTitleChange={(title) => handleTerminalTitleChange(tab.id, title)}
									sshConnectParams={tab.sshConnectParams}
									tmuxSwitch={tab.tmuxSwitch}
									reconnectRequest={tab.reconnect}
									telnetConnectParams={tab.telnetConnectParams}
									serialConnectParams={tab.serialConnectParams}
									onReconnected={(newId) => (tab.type === 'telnet' ? setTelnetConnection(tab.id, newId) : tab.type === 'serial' ? setSerialConnection(tab.id, newId) : updateTabConnection(tab.id, newId))}
									onReady={() => handleTerminalReady(tab)}
									terminalTheme={tab.terminalTheme}
									cursorStyle={tab.cursorStyle}
									cursorBlink={tab.cursorBlink}
									sessionId={tab.sessionId}
									sessionName={tab.sessionName}
									historyMode={tab.historyMode}
									wslDistro={tab.wslDistro}
									handoff={tab.handoff}
									startDown={tab.link === 'down'}/>
								<!-- The full-pane file manager over the terminal, which keeps
								     running behind it; F10 or the bar's button goes back. -->
								{#if filesShown && tab.connectionId}
									<DualPane connectionId={tab.connectionId} active={focused} scp={tab.filesProtocol === 'scp'} onclose={() => toggleFilesView(tab.id, false)} />
								{/if}
								</div>
							{/if}
						</div>
					{/each}
					<!-- A focusable separator is ARIA's window splitter: drag it, or move it with the arrow keys. -->
					{#each geo.dividers as d (d.path)}
						<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
						<div
							class="split-divider"
							class:down={d.dir === 'column'}
							style={dividerStyle(d)}
							role="separator"
							tabindex="0"
							aria-orientation={d.dir === 'row' ? 'vertical' : 'horizontal'}
							aria-label={t('split.divider')}
							aria-valuemin={10}
							aria-valuemax={90}
							aria-valuenow={Math.round(d.ratio * 100)}
							onpointerdown={(e) => startResize(e, d)}
							onkeydown={(e) => onDividerKey(e, d)}
							ondblclick={() => setSplitRatio(d.path, 0.5)}
						></div>
					{/each}
					{#if draggedTab && drop}
						<div
							class="drop-zone"
							style="left:{drop.preview.x * 100}%;top:{drop.preview.y * 100}%;width:{drop.preview.w * 100}%;height:{drop.preview.h * 100}%"
							aria-hidden="true"
						>
							<span>{t(DROP_LABEL[drop.zone])}</span>
						</div>
					{/if}
				</div>
				{#if share.isShowingRemote()}
					<RemoteTerminal />
				{/if}
				</div>
				<!-- Server stats come from an SSH or WSL session: under a desktop or a
				     local shell the bar only said "Not connected" about something that
				     was. It stays where it has the sharing controls to hold. -->
				{#if monitorBarShown()}
					<MonitoringBar
						connectionId={activeTab?.connectionId}
						sshUser={activeTab?.title?.split('@')[0]}
						sessionId={activeTab?.type === 'ssh' ? activeTab?.connectionId : activeTab?.id}
						monitoringOff={!monitoringWanted(activeTab)}
					/>
				{/if}
			{/if}
		</div>
	</div>
	<!-- One Share dialog for the whole app: the bottom bar's Share chip and the
	     Sessions + menu both open it, and it must be reachable with no
	     terminal open, or a guest has nowhere to paste the code. -->
	{#if share.isDialogOpen()}
		<ShareDialog />
	{/if}
{/if}

<style>

	.page-container {
		display: flex;
		flex-direction: column;
		width: 100%;
		height: 100%;
		overflow: hidden;
		position: relative;
	}

	.page-view {
		position: absolute;
		inset: 0;
		display: none;
		flex-direction: column;
	}

	.page-view.active {
		display: flex;
	}

	.terminal-area {
		flex: 1;
		position: relative;
		/* Not hidden: that can still be scrolled, and a phone keyboard
		   opening scrolled the terminal's input into view, leaving the
		   terminal half off the top and a gap under it (#127). */
		overflow: clip;
	}

	.terminal-wrapper {
		--terminal-bar-height: 34px;
		position: absolute;
		inset: 0;
		display: none;
	}

	.terminal-wrapper.active {
		display: block;
	}

	/* The terminal bar on top, the terminal in what is left under it. */
	.bar-slot {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		z-index: 5;
	}

	.pane-body {
		position: absolute;
		inset: 0;
	}

	.bar-slot + .pane-body {
		top: var(--terminal-bar-height);
	}

	/* Laid out as the page it was: a column its workspace fills. */
	.devops-body {
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}

	.devops-failed {
		margin: auto;
		max-width: 520px;
		padding: 24px;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 10px;
		text-align: center;
		color: var(--color-text-secondary);
		font-size: var(--text-sm);
	}

	.devops-failed code {
		max-width: 100%;
		overflow-wrap: anywhere;
		color: var(--color-danger, #e5534b);
	}

	.devops-retry {
		padding: 6px 14px;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: var(--color-bg-secondary);
		color: var(--color-text-primary);
		font-family: var(--font-sans);
		cursor: pointer;
	}

	@media (pointer: coarse) {
		.terminal-wrapper {
			--terminal-bar-height: 40px;
		}
	}

	/* "On hover": the bar lies over the top of the terminal, shown while the
	   pointer is near, so the terminal keeps its full height. */
	@media (hover: hover) {
		.bar-slot.hover-only {
			opacity: 0;
			transition: opacity 120ms ease;
		}
		.bar-slot.hover-only + .pane-body {
			top: 0;
		}

		.terminal-wrapper:hover .bar-slot.hover-only,
		.bar-slot.hover-only:focus-within {
			opacity: 1;
		}
	}

	/* Split (#106): which pane has the keyboard is drawn, faintly, so typing
	   never goes to the wrong server. */
	/* Drawn above the pane's content (a terminal paints its whole box), and
	   never in the way of a click. */
	.terminal-wrapper.pane::after {
		content: '';
		position: absolute;
		inset: 0;
		z-index: 5;
		pointer-events: none;
		border: 1px solid var(--color-border);
	}

	.terminal-wrapper.pane.focused::after {
		border: 2px solid var(--color-accent);
	}

	/* In a broadcast: what is typed into one of these goes to all of them.
	   Drawn above the split's own border, in the broadcast bar's colour. */
	.terminal-wrapper.broadcast::before {
		content: '';
		position: absolute;
		inset: 0;
		z-index: 6;
		pointer-events: none;
		border: 2px dashed var(--color-warning);
	}

	.terminal-wrapper.broadcast.focused::before,
	.terminal-wrapper.broadcast:not(.pane)::before {
		border-style: solid;
	}

	.split-divider {
		position: absolute;
		z-index: 2;
		cursor: col-resize;
		background: transparent;
		touch-action: none;
	}

	.split-divider.down {
		cursor: row-resize;
	}

	.split-divider::after {
		content: '';
		position: absolute;
		inset: 0;
		margin: auto;
		background: var(--color-border);
	}

	.split-divider:not(.down)::after {
		width: 1px;
	}

	.split-divider.down::after {
		height: 1px;
	}

	.split-divider:hover::after,
	.split-divider:focus-visible::after {
		background: var(--color-accent-fill);
	}

	.split-divider:not(.down):hover::after,
	.split-divider:not(.down):focus-visible::after {
		width: 3px;
	}

	.split-divider.down:hover::after,
	.split-divider.down:focus-visible::after {
		height: 3px;
	}

	.split-divider:focus-visible {
		outline: none;
	}

	/* Where a dragged tab would land. */
	.drop-zone {
		position: absolute;
		z-index: 7;
		box-sizing: border-box;
		transition: left 90ms ease, top 90ms ease, width 90ms ease, height 90ms ease;
		display: flex;
		align-items: center;
		justify-content: center;
		background: color-mix(in srgb, var(--color-accent) 16%, transparent);
		border: 2px dashed color-mix(in srgb, var(--color-accent) 70%, transparent);
		border-radius: 6px;
		pointer-events: none;
	}

	.drop-zone span {
		padding: 6px 12px;
		border-radius: 999px;
		background: var(--color-accent-fill);
		color: #fff;
		font-size: var(--text-sm);
		font-weight: 600;
	}
	.terminal-split {
		flex: 1;
		display: flex;
		min-height: 0;
		min-width: 0;
	}

	.terminal-split > .terminal-area {
		flex: 1;
		min-width: 0;
	}

	/* Half each, and a hairline between: theirs is a mirror, not a tab. */
	.terminal-split.sharing > :global(.remote-pane) {
		flex: 1;
		min-width: 0;
		border-left: 1px solid var(--color-border);
	}
</style>
