<script lang="ts">
	import { rdpDisconnectAll } from '#lib/ipc/rdp.js';
	import { vncDisconnectAll } from '#lib/ipc/vnc.js';
	import { spiceDisconnectAll } from '#lib/ipc/spice.js';
	import type { Snippet } from 'svelte';
	import { onMount, onDestroy } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import TitleBar from './TitleBar.svelte';
	import TabBar from './TabBar.svelte';
	import Sidebar from './Sidebar.svelte';
	import StatusBar from './StatusBar.svelte';
	import HistoryPanel from '#lib/components/terminal/HistoryPanel.svelte';
	import CommandPicker from '#lib/components/terminal/CommandPicker.svelte';
	import { monitorBarShown } from '#lib/state/monitor-bar.svelte.js';
	import Toast from '#lib/components/shared/Toast.svelte';
	import UpdateBanner from '#lib/components/shared/UpdateBanner.svelte';
	import UpdateDialog from '#lib/components/shared/UpdateDialog.svelte';
	import WhatsNewDialog from '#lib/components/shared/WhatsNewDialog.svelte';
	import ActiveSessionsDialog from '#lib/components/shared/ActiveSessionsDialog.svelte';
	import HostKeyDialog from '#lib/components/shared/HostKeyDialog.svelte';
	import AuthPromptDialog from '#lib/components/shared/AuthPromptDialog.svelte';
	import PassphrasePrompt from '#lib/components/pwmanager/PassphrasePrompt.svelte';
	import WebCertPrompt from '#lib/components/sessions/WebCertPrompt.svelte';
	import DuplicateVaultsDialog from '#lib/components/vault/DuplicateVaultsDialog.svelte';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import McpConfirmDialog from '#lib/components/shared/McpConfirmDialog.svelte';
	import { getUpdaterState, relaunchNow, postponeRelaunch } from '#lib/state/updater.svelte.js';
	import { getActiveTab, getTabs, isDesktopTab } from '#lib/state/tabs.svelte.js';
	import { getActivePage } from '#lib/state/navigation.svelte.js';
	import { getSettings } from '#lib/state/settings.svelte.js';
	import { sshListConnections } from '#lib/ipc/ssh.js';
	import { syncTrayVisibility } from '#lib/state/tray.svelte.js';
	import AIPanel from '#lib/components/ai/AIPanel.svelte';

	const updater = getUpdaterState();

	interface Props {
		children: Snippet;
	}

	let { children }: Props = $props();

	let sidebarCollapsed = $state(false);
	let activeTab = $derived(getActiveTab());
	let activeConnectionId = $derived(activeTab?.connectionId);

	// On a phone the sidebar is a full-screen drawer over the terminal: once a
	// tab opens or another is picked, close it so that tab is what you see.
	let lastTabId: string | undefined;
	$effect(() => {
		const id = activeTab?.id;
		if (id && id !== lastTabId && window.matchMedia('(max-width: 700px)').matches) {
			sidebarCollapsed = true;
		}
		lastTabId = id;
	});

	// The same for a workspace (Containers, Kubernetes, Databases…): picked
	// from the tab bar, it is what you came to see, not the session drawer.
	let lastPage: string | undefined;
	$effect(() => {
		const page = getActivePage();
		if (page !== lastPage && page !== 'terminal' && window.matchMedia('(max-width: 700px)').matches) {
			sidebarCollapsed = true;
		}
		lastPage = page;
	});

	// --- Active-session guards for window close / app quit + update relaunch ---
	let closeOpen = $state(false);
	let closeCount = $state(0);
	/** Of those, the tabs whose work lives on in tmux on their server. */
	let closeTmuxCount = $state(0);
	let closeMode = $state<'window' | 'quit'>('window');
	let updateOpen = $state(false);
	let updateCount = $state(0);

	/** Count live SSH connections (backend truth; falls back to connected tabs),
	 *  plus open Telnet and serial sessions and desktops. A tab whose session already
	 *  ended (it shows Reconnect) is not live: closing it loses nothing. */
	async function countActiveConnections(): Promise<number> {
		const tabs = getTabs();
		const ended = new Set(tabs.filter((tab) => tab.link === 'down' && tab.connectionId).map((tab) => tab.connectionId));
		const others = tabs.filter((tab) => tab.link !== 'down' && (isDesktopTab(tab) || (tab.type === 'telnet' && !!tab.telnetConnectionId) || (tab.type === 'serial' && !!tab.serialConnectionId) || (tab.type === 'ftp' && !!tab.ftpConnectionId) || (tab.type === 'bmc' && !!tab.bmcConnectionId) || (tab.type === 'proxmox' && !!tab.proxmoxConnectionId) || (tab.type === 'mqtt' && !!tab.mqttConnectionId) || (tab.type === 's3' && !!tab.s3ConnectionId))).length;
		try {
			return (await sshListConnections()).filter((c) => !ended.has(c.id)).length + others;
		} catch {
			return tabs.filter((tab) => tab.type === 'ssh' && !!tab.connectionId && tab.link !== 'down').length + others;
		}
	}

	/** Actually leave: quit the whole app (tray Quit) or just close the window. */
	async function doExit(mode: 'window' | 'quit'): Promise<void> {
		// Desktops first, so each server gets a disconnect rather than a
		// dropped socket, and no session thread outlives the window.
		try {
			await Promise.all([rdpDisconnectAll(), vncDisconnectAll(), spiceDisconnectAll()]);
		} catch {
			// Nothing open, or the backend is already gone: leave anyway.
		}
		try {
			if (mode === 'quit') {
				await invoke('quit_app');
			} else {
				await getCurrentWindow().destroy();
			}
		} catch (e) {
			console.error('Exit failed:', e);
		}
	}

	/** Confirm first if SSH sessions are live; otherwise exit straight away. */
	async function requestExit(mode: 'window' | 'quit'): Promise<void> {
		const count = await countActiveConnections();
		if (count === 0) {
			await doExit(mode);
			return;
		}
		closeMode = mode;
		closeCount = count;
		closeTmuxCount = getTabs().filter((tab) => tab.type === 'ssh' && tab.link !== 'down' && !!tab.sshConnectParams?.tmux).length;
		closeOpen = true;
	}

	let unlistenClose: (() => void) | undefined;
	let unlistenQuit: (() => void) | undefined;
	// A login that got in with the SSH agent or a password after the server
	// refused the session's own key. It works here, and fails on any device
	// without that agent (a phone): said now, rather than found out there.
	onMount(() => {
		// A session log that could not be opened: the session runs, but the
		// user is told at once that it is not being logged.
		const stopLogError = listen<{ host: string; message: string }>('ssh-log-error', (e) => {
			addToast(t('ssh.log_failed', { host: e.payload.host, message: e.payload.message }), 'error', 20000);
		});
		// A host key refused under ssh_config settings: the connection error
		// only says the key was not accepted, this says why.
		const stopHostKey = listen<{ host: string; port: number; reason: string }>('ssh-hostkey-refused', (e) => {
			addToast(t('hostkey.refused', { host: `${e.payload.host}:${e.payload.port}`, reason: e.payload.reason }), 'error', 20000);
		});
		const stopWeakKex = listen<{ host: string; port: number; kex: string }>('ssh-weak-kex', (e) => {
			addToast(t('ssh.weak_kex', { host: `${e.payload.host}:${e.payload.port}`, kex: e.payload.kex }), 'warning', 12000);
		});
		const unlisten = listen<{ host: string; fingerprint: string; via: 'agent' | 'password' }>('ssh-key-refused-notice', (e) => {
			const { host, fingerprint, via } = e.payload;
			addToast(t(via === 'agent' ? 'ssh.key_refused_agent' : 'ssh.key_refused_password', { host, fingerprint }), 'warning', 20000);
		});
		return () => {
			void unlisten.then((stop) => stop());
			void stopLogError.then((stop) => stop());
			void stopHostKey.then((stop) => stop());
			void stopWeakKex.then((stop) => stop());
		};
	});

	onMount(async () => {
		try {
			// Window close (X / Alt+F4 / Cmd+Q). The frontend owns the decision
			// (single source of truth) — always prevent, then either hide to tray
			// or run the active-session guard. This avoids relying on the Rust
			// close-to-tray flag staying in sync (which caused X to close instead
			// of hiding to tray).
			unlistenClose = await getCurrentWindow().onCloseRequested(async (event) => {
				event.preventDefault(); // hold synchronously; we decide what to do
				if (getSettings().minimizeToTray) {
					await getCurrentWindow().hide(); // minimize to tray, not a termination
					syncTrayVisibility();
					return;
				}
				await requestExit('window');
			});
			// Tray "Quit" routes here (instead of a hard exit) so it warns about
			// active SSH sessions too.
			unlistenQuit = await listen('app-quit-requested', () => {
				void requestExit('quit');
			});
		} catch (e) {
			console.error('Failed to register exit handlers:', e);
		}
	});
	onDestroy(() => {
		unlistenClose?.();
		unlistenQuit?.();
	});

	async function confirmClose(): Promise<void> {
		closeOpen = false;
		await doExit(closeMode);
	}
	function cancelClose(): void {
		closeOpen = false;
	}

	// When an update finishes installing, confirm before relaunching if sessions
	// are live; otherwise relaunch immediately (e.g. startup with no sessions).
	let orchestrating = false;
	$effect(() => {
		if (!updater.readyToRelaunch) return;
		void orchestrateRelaunch();
	});

	async function orchestrateRelaunch(): Promise<void> {
		if (orchestrating) return;
		orchestrating = true;
		try {
			const count = await countActiveConnections();
			if (count === 0) {
				await relaunchNow();
				return;
			}
			updateCount = count;
			updateOpen = true;
		} finally {
			orchestrating = false;
		}
	}

	async function confirmUpdate(): Promise<void> {
		updateOpen = false;
		await relaunchNow();
	}
	function postponeUpdate(): void {
		updateOpen = false;
		postponeRelaunch();
	}
</script>

<div class="app-shell" class:monitor-bottom={monitorBarShown()}>
	<TitleBar />
	<TabBar />

	<div class="app-body">
		<Sidebar bind:collapsed={sidebarCollapsed} connectionId={activeConnectionId} />
		<main class="main-content">
			{@render children()}
		</main>
		<!-- The assistant works on shells. A desktop tab has none for it to
		     read or type into, nor does a Telnet or serial device's console
		     (it runs no shell commands), so it is told there is no tab. -->
		<AIPanel
			connectionId={activeConnectionId}
			activeTabId={isDesktopTab(activeTab) || activeTab?.type === 'telnet' || activeTab?.type === 'serial' || activeTab?.type === 'ftp' || activeTab?.type === 'bmc' || activeTab?.type === 'proxmox' || activeTab?.type === 'mqtt' || activeTab?.type === 's3' ? undefined : activeTab?.id}
			activeTabType={activeTab?.type === 'local' || activeTab?.type === 'ssh' ? activeTab.type : undefined}
		/>
	</div>

	{#if !monitorBarShown()}
		<StatusBar />
	{/if}
	<!-- Outside the bar's condition: an SSH or WSL tab swaps the status bar
	     for the monitor bar, and the panel must open over those too. -->
	<HistoryPanel />
	<CommandPicker />
	<Toast />
	<UpdateBanner />
	<UpdateDialog open={updater.startupBlocking} />
	<WhatsNewDialog />
	<DuplicateVaultsDialog />

	<ActiveSessionsDialog
		open={closeOpen}
		variant="close"
		count={closeCount}
		tmuxCount={closeTmuxCount}
		onconfirm={confirmClose}
		oncancel={cancelClose}
	/>
	<ActiveSessionsDialog
		open={updateOpen}
		variant="update"
		count={updateCount}
		onconfirm={confirmUpdate}
		oncancel={postponeUpdate}
	/>
	<HostKeyDialog />
	<AuthPromptDialog />
	<PassphrasePrompt />
	<WebCertPrompt />
	<McpConfirmDialog />
</div>

<style>
	.app-shell {
		display: grid;
		grid-template-rows: var(--titlebar-h, 38px) var(--tabbar-h, 36px) minmax(0, 1fr) var(--statusbar-h, 24px);
		/* One column the window's width. Left implicit, it grew to the widest
		   row (a few tabs, a long status) and the shell cut the right side of
		   everything off on a narrow phone. */
		grid-template-columns: minmax(0, 1fr);
		width: 100vw;
		height: 100vh;
		/* dvh tracks the *visible* viewport, so the status bar is not pushed
		   under the on-screen keyboard or the mobile browser chrome. Declared
		   after the vh fallback for engines that do not support it. */
		height: 100dvh;
		/* Keep the title/status bars clear of the device status & navigation
		   bars on mobile. `env(safe-area-inset-*)` is 0 on desktop, so this is
		   a no-op there (requires viewport-fit=cover, set in app.html). */
		padding-top: var(--safe-top);
		padding-bottom: var(--safe-bottom);
		/* And the sides: turned on its side, a phone has its camera cutout
		   and navigation bar there, and Reach was drawn under them. */
		padding-left: var(--safe-left);
		padding-right: var(--safe-right);
		box-sizing: border-box;
		overflow: hidden;
		background-color: var(--color-bg-primary);
	}

	/* iOS lays its keyboard over the page and scrolls the page up instead of
	   making room (lib/keyboard-viewport.ts). The shell is pinned to the part
	   still visible, so the bars stay put and the terminal gets shorter. */
	:global(html.ios) .app-shell {
		position: fixed;
		left: 0;
		top: var(--vv-top, 0px);
		height: var(--vv-height, 100dvh);
	}

	/* The home indicator is under the keyboard then: no strip kept for it. */
	:global(html.keyboard-open) .app-shell {
		padding-bottom: 0;
	}

	.app-body {
		position: relative;
		display: flex;
		overflow: hidden;
	}

	.main-content {
		flex: 1;
		min-width: 0;
		overflow: auto;
		background-color: var(--color-bg-primary);
	}

	/* The session monitor bar is the window's bottom row and says what the
	   status bar would (monitor-bar.svelte.ts), so the status bar's row goes. */
	.app-shell.monitor-bottom {
		--statusbar-h: 0px;
	}

	/* Phones in landscape, and short desktop windows, have very little vertical
	   room. Trim the fixed chrome so the terminal keeps a usable number of rows. */
	@media (max-height: 480px) {
		.app-shell {
			--titlebar-h: 32px;
			--tabbar-h: 30px;
			--statusbar-h: 20px;
		}
	}

	/* A phone on its side: every line goes to the terminal. The status bar
	   only repeats the tab and the server stats, so it goes. */
	@media (max-height: 480px) and (pointer: coarse) {
		.app-shell {
			--statusbar-h: 0px;
		}

		.app-shell :global(.statusbar) {
			display: none;
		}
	}
</style>
