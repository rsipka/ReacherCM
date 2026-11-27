<script lang="ts">
	import FileExplorer from '#lib/components/explorer/FileExplorer.svelte';
	import SessionList from '#lib/components/sessions/SessionList.svelte';
	import TunnelManager from '#lib/components/tunnel/TunnelManager.svelte';
	import NetworkTools from '#lib/components/net/NetworkTools.svelte';
	import PluginPanel from '#lib/components/plugin/PluginPanel.svelte';
	import SnippetPanel from '#lib/components/snippets/SnippetPanel.svelte';
	import RecipePanel from '#lib/components/recipes/RecipePanel.svelte';
	import VaultPanel from '#lib/components/vault/VaultPanel.svelte';
	import ServerPanel from '#lib/components/servers/ServerPanel.svelte';
	import { isMobile } from '#lib/platform.js';
	import NewIndicator from '#lib/components/shared/NewIndicator.svelte';
	import { sidebarHasNew, markSidebarSeen, newSince } from '#lib/state/whats-new.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { pendingSessionAction } from '#lib/state/palette.svelte.js';
	import { getPlugins, setPlugins } from '#lib/state/plugin.svelte.js';
	import { pluginList } from '#lib/ipc/plugin.js';
	import { onMount } from 'svelte';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { addToast } from '#lib/state/toasts.svelte.js';

	type Section = 'sessions' | 'vault' | 'explorer' | 'tunnels' | 'servers' | 'network' | 'snippets' | 'recipes' | 'plugins';

	const STORAGE_KEY = 'reach-sidebar-width';
	/* `sidebarWidth` is the width of the *panel*, not of the whole sidebar. The
	   rail is added on top when rendering. Keeping the stored value meaning the
	   same thing as before the rail existed means saved widths stay correct
	   instead of every existing user losing 48px of panel. */
	const MIN_WIDTH = 160;
	const MAX_WIDTH = 600;
	const DEFAULT_WIDTH = 240;
	const RAIL_WIDTH = 48;
	const COLLAPSED_WIDTH = RAIL_WIDTH;

	interface Props {
		collapsed: boolean;
		connectionId?: string;
	}

	let { collapsed = $bindable(false), connectionId }: Props = $props();

	let activeSection = $state<Section>('sessions');

	// A request for the session list (the command palette's, a container
	// shell's): show the list so it can carry it out, and its prompts, which
	// live inside it, can be seen. Before the list's own effect takes the
	// request: otherwise, with the sidebar collapsed, New session opened
	// inside a hidden panel and nothing appeared.
	$effect.pre(() => {
		if (!pendingSessionAction()) return;
		activeSection = 'sessions';
		collapsed = false;
	});
	// A section asked for from elsewhere (the terminal bar's Files and
	// Snippets): shown, and the sidebar opened if it was folded.
	$effect(() => {
		function onSection(e: Event): void {
			const id = (e as CustomEvent<Section>).detail;
			if (!sections.some((s) => s.id === id)) return;
			markSidebarSeen(id);
			activeSection = id;
			collapsed = false;
		}
		// A panel's own way back to what is under the drawer on a phone
		// (the Files panel's terminal button).
		function onClose(): void {
			collapsed = true;
		}
		window.addEventListener('reach:sidebar-section', onSection);
		window.addEventListener('reach:sidebar-close', onClose);
		return () => {
			window.removeEventListener('reach:sidebar-section', onSection);
			window.removeEventListener('reach:sidebar-close', onClose);
		};
	});
	let sidebarWidth = $state(loadWidth());
	let dragging = $state(false);

	function loadWidth(): number {
		try {
			const saved = localStorage.getItem(STORAGE_KEY);
			if (saved) {
				const n = parseInt(saved, 10);
				if (n >= MIN_WIDTH && n <= MAX_WIDTH) return n;
			}
		} catch {}
		return DEFAULT_WIDTH;
	}

	function saveWidth(w: number): void {
		try {
			localStorage.setItem(STORAGE_KEY, String(w));
		} catch {}
	}

	let hasPlugins = $derived(getPlugins().length > 0);

	/* Each panel the sidebar can show. The rail has fewer buttons than this:
	   Tunnels, Servers and Network tools share one (Network), and Snippets,
	   Recipes and Plugins another (Library), with a switch at the top of the
	   panel between them. Nine icons were more than the rail could hold on a
	   laptop screen, and more than anyone could tell apart. */
	let sections = $derived<Array<{ id: Section; label: string; icon: string }>>([
		{
			id: 'sessions',
			label: t('sidebar.sessions'),
			icon: 'M4 6h16M4 10h16M4 14h16M4 18h16'
		},
		{
			id: 'vault',
			label: t('sidebar.vault'),
			// A safe with a dial: the secrets side of Reach, which until now had
			// no door of its own.
			icon: 'M4 5h16a1 1 0 011 1v12a1 1 0 01-1 1H4a1 1 0 01-1-1V6a1 1 0 011-1zM12 9a3 3 0 100 6 3 3 0 000-6zM12 12h.01M7 19v2M17 19v2'
		},
		{
			id: 'explorer',
			label: t('sidebar.explorer'),
			icon: 'M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z'
		},
		{
			id: 'tunnels',
			label: t('sidebar.tunnels'),
			icon: 'M10 13a5 5 0 007.54.54l3-3a5 5 0 00-7.07-7.07l-1.72 1.71M14 11a5 5 0 00-7.54-.54l-3 3a5 5 0 007.07 7.07l1.71-1.71'
		},
		// Local TFTP, FTP, SFTP and HTTP servers: desktop only, as a phone
		// is not where a switch pulls its firmware from.
		...(isMobile()
			? []
			: [
					{
						id: 'servers' as const,
						label: t('sidebar.servers'),
						// A server rack: two units with their lights.
						icon: 'M4 4h16a1 1 0 011 1v5a1 1 0 01-1 1H4a1 1 0 01-1-1V5a1 1 0 011-1zM4 13h16a1 1 0 011 1v5a1 1 0 01-1 1H4a1 1 0 01-1-1v-5a1 1 0 011-1zM7 7.5h.01M7 16.5h.01M11 7.5h6M11 16.5h6'
					}
				]),
		{
			id: 'network',
			label: t('sidebar.network'),
			// A pulse: ping, port scan and Wake-on-LAN check what answers.
			icon: 'M22 12h-4l-3 9L9 3l-3 9H2'
		},
		{
			id: 'snippets',
			label: t('sidebar.snippets'),
			icon: 'M16 4h2a2 2 0 012 2v14a2 2 0 01-2 2H6a2 2 0 01-2-2V6a2 2 0 012-2h2M9 2h6a1 1 0 011 1v1a1 1 0 01-1 1H9a1 1 0 01-1-1V3a1 1 0 011-1z'
		},
		{
			id: 'recipes',
			label: t('sidebar.recipes'),
			// A book: a recipe is something written down to be followed again.
			icon: 'M4 19.5A2.5 2.5 0 016.5 17H20M6.5 2H20v20H6.5A2.5 2.5 0 014 19.5v-15A2.5 2.5 0 016.5 2zM9 7h7M9 11h5'
		},
		// Only with a plugin installed: managing them (adding, the marketplace)
		// is in Settings → Plugins; this is where their own panels show.
		...(hasPlugins
			? [
					{
						id: 'plugins' as const,
						label: t('sidebar.plugins'),
						icon: 'M13 2L3 14h9l-1 8 10-12h-9l1-8z'
					}
				]
			: [])
	]);

	onMount(() => {
		pluginList()
			.then((list) => setPlugins(Array.isArray(list) ? list : []))
			.catch(() => {});
		// A plugin's notify(): shown whatever panel is open, as plugins now sit
		// behind the Library button rather than a rail button of their own.
		let unlisten: UnlistenFn | undefined;
		let gone = false;
		listen<{ pluginId: string; message: string; level?: string }>('plugin-notify', (event) => {
			const { message, level } = event.payload;
			addToast(message, level === 'error' ? 'error' : level === 'warning' ? 'warning' : 'info');
		})
			.then((fn) => (gone ? fn() : (unlisten = fn)))
			.catch(() => {});
		return () => {
			gone = true;
			unlisten?.();
		};
	});

	type RailId = 'sessions' | 'vault' | 'explorer' | 'network' | 'library';
	interface Rail {
		id: RailId;
		label: string;
		icon: string;
		/** The panels behind this button, in the order of its switch. */
		members: Section[];
	}

	const RAIL_MEMBERS: Record<RailId, Section[]> = {
		sessions: ['sessions'],
		vault: ['vault'],
		explorer: ['explorer'],
		network: ['tunnels', 'servers', 'network'],
		library: ['snippets', 'recipes', 'plugins']
	};

	let rails = $derived<Rail[]>(
		(Object.keys(RAIL_MEMBERS) as RailId[]).map((id) => {
			const members = RAIL_MEMBERS[id].filter((m) => sections.some((s) => s.id === m));
			const own = sections.find((s) => s.id === id && members.length === 1);
			return {
				id,
				members,
				label: own?.label ?? (id === 'network' ? t('sidebar.network_group') : t('sidebar.library')),
				icon:
					own?.icon ??
					(id === 'network'
						? // A globe: tunnels, servers here and tools that reach out.
							'M12 3a9 9 0 100 18 9 9 0 000-18zM3 12h18M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9c-2.5-2.6-3.8-5.6-3.8-9S9.5 5.6 12 3z'
						: // A book: snippets, recipes and plugins are kept to use again.
							'M4 19.5A2.5 2.5 0 016.5 17H20M6.5 2H20v20H6.5A2.5 2.5 0 014 19.5v-15A2.5 2.5 0 016.5 2zM9 7h7M9 11h5')
			};
		})
	);

	function railOf(section: Section): RailId {
		return (Object.keys(RAIL_MEMBERS) as RailId[]).find((r) => RAIL_MEMBERS[r].includes(section)) ?? 'sessions';
	}

	/** The panel each grouped button showed last, to go back to it. */
	let lastIn = $state<Partial<Record<RailId, Section>>>({});
	$effect(() => {
		lastIn[railOf(activeSection)] = activeSection;
	});

	let activeRail = $derived(rails.find((r) => r.id === railOf(activeSection)));

	// A panel that went (the last plugin removed): back to the first of its group.
	$effect(() => {
		if (!sections.some((s) => s.id === activeSection)) activeSection = activeRail?.members[0] ?? 'sessions';
	});

	function railHasNew(rail: Rail): boolean {
		return rail.members.some((m) => sidebarHasNew(m));
	}

	function handleRailClick(rail: Rail): void {
		const remembered = lastIn[rail.id];
		const target = remembered && rail.members.includes(remembered) ? remembered : rail.members[0];
		if (collapsed) {
			collapsed = false;
			activeSection = target;
		} else if (railOf(activeSection) === rail.id) {
			collapsed = true;
			return;
		} else {
			activeSection = target;
		}
		markSidebarSeen(target);
	}

	function toggleCollapsed(): void {
		collapsed = !collapsed;
	}

	const clampWidth = (w: number) => Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(w)));

	// Pointer events, so a pen or a finger on a touch screen drags it too.
	function startResize(e: PointerEvent): void {
		if (e.button !== 0) return;
		e.preventDefault();
		dragging = true;
		const startX = e.clientX;
		const startW = sidebarWidth;

		function onMove(ev: PointerEvent): void {
			// startW is the panel width, and the pointer moves the sidebar's outer
			// edge, so the delta applies to the panel directly.
			sidebarWidth = clampWidth(startW + ev.clientX - startX);
		}

		function onUp(): void {
			dragging = false;
			document.removeEventListener('pointermove', onMove);
			document.removeEventListener('pointerup', onUp);
			document.removeEventListener('pointercancel', onUp);
			saveWidth(sidebarWidth);
		}

		document.addEventListener('pointermove', onMove);
		document.addEventListener('pointerup', onUp);
		document.addEventListener('pointercancel', onUp);
	}

	// The keyboard's way to the same width, as the split divider has.
	function onResizeKey(e: KeyboardEvent): void {
		const next =
			e.key === 'ArrowLeft' ? sidebarWidth - 16 :
			e.key === 'ArrowRight' ? sidebarWidth + 16 :
			e.key === 'Home' ? MIN_WIDTH :
			e.key === 'End' ? MAX_WIDTH :
			null;
		if (next === null) return;
		e.preventDefault();
		sidebarWidth = clampWidth(next);
		saveWidth(sidebarWidth);
	}

	function resetWidth(): void {
		sidebarWidth = DEFAULT_WIDTH;
		saveWidth(sidebarWidth);
	}
</script>

<aside class="sidebar" class:collapsed class:no-transition={dragging} style:width="{collapsed ? COLLAPSED_WIDTH : sidebarWidth + RAIL_WIDTH}px">
	<nav class="sidebar-nav">
		{#each rails as rail (rail.id)}
			<button
				class="nav-btn"
				class:active={activeRail?.id === rail.id}
				class:has-new={railHasNew(rail)}
				onclick={() => handleRailClick(rail)}
				title={rail.label}
				aria-label={rail.label}
			>
				<svg width="19" height="19" viewBox="0 0 24 24" fill="none">
					<path
						d={rail.icon}
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>
				</svg>
				{#if railHasNew(rail)}
					<NewIndicator since={newSince({ kind: 'sidebar', section: rail.members.find((m) => sidebarHasNew(m)) ?? rail.id })} />
				{/if}
			</button>
		{/each}

		<button
			class="nav-btn rail-toggle"
			onclick={toggleCollapsed}
			title={collapsed ? t('sidebar.expand') : t('sidebar.collapse')}
			aria-label={collapsed ? t('sidebar.expand') : t('sidebar.collapse')}
		>
			<svg width="16" height="16" viewBox="0 0 16 16" fill="none">
				{#if collapsed}
					<path d="M6 3l5 5-5 5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
				{:else}
					<path d="M10 3L5 8l5 5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
				{/if}
			</svg>
		</button>
	</nav>

	<!-- Kept mounted (hidden when collapsed) so collapsing/expanding the sidebar
	     does NOT unmount the active panel and re-read the DB. Panels still mount/
	     unmount on section switch, just not on collapse. -->
	<div class="sidebar-content" class:hidden={collapsed}>
		<div class="section-header">
			{activeRail?.label ?? ''}
		</div>
		{#if activeRail && activeRail.members.length > 1}
			<!-- The panels behind one rail button. -->
			<div class="section-switch" role="tablist" aria-label={activeRail.label}>
				{#each activeRail.members as id (id)}
					{@const section = sections.find((x) => x.id === id)}
					{#if section}
						<button
							class="switch-btn"
							class:active={activeSection === id}
							role="tab"
							aria-selected={activeSection === id}
							onclick={() => {
								markSidebarSeen(id);
								activeSection = id;
							}}
						>
							{id === 'network' ? t('sidebar.net_tools_short') : section.label}
							{#if sidebarHasNew(id)}<span class="switch-new" aria-hidden="true"></span>{/if}
						</button>
					{/if}
				{/each}
			</div>
		{/if}
		<div class="section-body">
			{#key activeSection}
				<div class="section-panel">
					{#if activeSection === 'sessions'}
						<SessionList />
					{:else if activeSection === 'explorer'}
						<FileExplorer {connectionId} />
					{:else if activeSection === 'tunnels'}
						<TunnelManager />
					{:else if activeSection === 'servers'}
						<ServerPanel />
					{:else if activeSection === 'network'}
						<NetworkTools />
					{:else if activeSection === 'snippets'}
						<SnippetPanel {connectionId} />
					{:else if activeSection === 'vault'}
						<VaultPanel />
					{:else if activeSection === 'recipes'}
						<RecipePanel />
					{:else if activeSection === 'plugins'}
						<PluginPanel {connectionId} />
					{/if}
				</div>
			{/key}
		</div>
	</div>

	{#if !collapsed}
		<!-- A focusable separator is ARIA's window splitter, as the split divider is:
		     drag it, arrow keys or Home/End; a double click goes back to the default. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
		<div
			class="resize-handle"
			role="separator"
			aria-orientation="vertical"
			aria-label={t('sidebar.resize')}
			aria-valuemin={MIN_WIDTH}
			aria-valuemax={MAX_WIDTH}
			aria-valuenow={sidebarWidth}
			tabindex="0"
			onpointerdown={startResize}
			onkeydown={onResizeKey}
			ondblclick={resetWidth}
		></div>
	{/if}
</aside>

{#if dragging}
	<!-- Overlay to prevent iframe/webview stealing mouse events during drag -->
	<div class="resize-overlay"></div>
{/if}

<style>
	/* Navigation and panel content are orthogonal concerns. Stacking them meant
	   the five nav buttons (149px) and the collapse row took their cut off the
	   top before the active panel got anything, so the session tree was left
	   ~186px of a 601px column — four rows — and every new tool would have taken
	   another slice. The nav is now a fixed rail beside the panel, so the panel
	   gets the full height and stays that size however many tools exist.
	   COLLAPSED_WIDTH is 48px, which is exactly the rail, so collapsing simply
	   hides the panel and leaves the rail. */
	.sidebar {
		position: relative;
		display: flex;
		flex-direction: row;
		height: 100%;
		background-color: var(--color-bg-secondary);
		border-right: 1px solid var(--color-border);
		overflow: hidden;
		transition: width var(--duration-default) var(--ease-default);
		user-select: none;
		flex-shrink: 0;
	}

	/* The width is remembered in localStorage and applied inline, so a sidebar
	   dragged to 600px on a desktop would still be 600px on a phone, leaving no
	   room for the terminal. Cap it against the viewport instead of the stored
	   value; the inline width still wins whenever it is the smaller of the two. */
	@media (max-width: 900px) {
		.sidebar:not(.collapsed) {
			max-width: 55vw;
		}
	}

	@media (max-width: 600px) {
		.sidebar:not(.collapsed) {
			max-width: 75vw;
		}
	}

	/* On a phone a 240px sidebar beside the terminal left the terminal 149px of a
	   390px screen — 62% of the display spent on navigation. Below 700px the
	   sidebar stops taking a column and overlays the content as a drawer, so the
	   terminal gets the full width and the existing collapse control opens and
	   closes the drawer. */
	@media (max-width: 700px) {
		/* The whole screen, whatever width was dragged to on a desktop: a
		   drawer that stops short leaves a strip of the terminal beside it
		   that is too narrow to read and still takes taps. */
		.sidebar:not(.collapsed) {
			position: absolute;
			inset: 0 auto 0 0;
			z-index: 20;
			width: 100% !important;
			max-width: none;
			box-shadow: none;
		}

		.resize-handle {
			display: none;
		}
	}

	/* Finger-sized hit targets where there is no mouse. */
	@media (pointer: coarse) {
		.nav-btn {
			width: 40px;
			height: 40px;
		}

		.resize-handle {
			width: 12px;
		}
	}

	.sidebar-nav {
		flex: 0 0 48px;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-1);
		padding: var(--space-2) 0;
		border-right: 1px solid var(--color-border);
		/* Scrolls when the window is shorter than its buttons, as a phone on
		   its side is: the last ones were cut off. */
		overflow-y: auto;
		scrollbar-width: none;
	}

	.rail-toggle {
		margin-top: auto;
	}

	/* Something new behind a button: the dot alone. A tinted tile here read as
	   a second selected item, so the rail looked to have two or three active
	   sections at once; the tile is for the active one only. */
	.nav-btn.has-new:not(.active) {
		--new-dot-ring: var(--color-bg-secondary);
	}

	.nav-btn {
		position: relative;
		display: flex;
		align-items: center;
		justify-content: center;
		flex: 0 0 auto;
		width: 36px;
		height: 36px;
		padding: 0;
		border: none;
		border-radius: var(--radius-btn);
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default);
	}

	.nav-btn:hover {
		background-color: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.nav-btn.active {
		background-color: var(--color-accent-subtle);
		color: var(--color-accent);
	}

	.nav-btn.active::before {
		content: '';
		position: absolute;
		left: -6px;
		top: 50%;
		transform: translateY(-50%);
		width: 3px;
		height: 20px;
		border-radius: 0 2px 2px 0;
		background-color: var(--color-accent-fill);
	}






	.sidebar-content {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}

	/* Hidden (not unmounted) when the sidebar is collapsed, so the active panel
	   keeps its loaded state instead of re-fetching on expand. */
	.sidebar-content.hidden {
		display: none;
	}

	.section-header {
		padding: 5px 12px;
		font-size: 0.625rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--color-text-secondary);
		border-top: 1px solid var(--color-border);
	}

	/* The panels behind a grouped rail button, as a segmented switch. */
	.section-switch {
		display: flex;
		gap: 2px;
		margin: 2px 8px 6px;
		padding: 2px;
		border-radius: 7px;
		background: var(--color-bg-secondary);
	}

	.switch-btn {
		position: relative;
		flex: 1;
		min-width: 0;
		padding: 4px 6px;
		border: none;
		border-radius: 5px;
		background: transparent;
		color: var(--color-text-secondary);
		font-family: var(--font-sans);
		font-size: var(--text-xs, 0.75rem);
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		cursor: pointer;
	}

	.switch-btn:hover {
		color: var(--color-text-primary);
	}

	.switch-btn.active {
		background: var(--color-bg-elevated, var(--color-bg-primary));
		color: var(--color-text-primary);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.15);
	}

	.switch-new {
		position: absolute;
		top: 3px;
		right: 4px;
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--color-accent);
	}

	@media (pointer: coarse) {
		.switch-btn {
			padding: 8px 6px;
		}
	}

	.section-body {
		flex: 1;
		overflow-y: auto;
		padding: 2px 6px;
	}

	/* A section fades in when it is switched to. A CSS animation, not the
	   View Transitions API: starting a view transition froze or crashed
	   WebKitGTK's web process on Linux (#143). Full height, as the panels
	   size themselves to it. */
	.section-panel {
		height: 100%;
		animation: section-in 150ms ease-out;
	}

	@keyframes section-in {
		from {
			opacity: 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.section-panel {
			animation: none;
		}
	}




	.sidebar.no-transition {
		transition: none;
	}

	.resize-handle {
		position: absolute;
		top: 0;
		right: 0;
		width: 4px;
		height: 100%;
		cursor: col-resize;
		z-index: 10;
		/* A finger's drag moves the edge instead of scrolling the page. */
		touch-action: none;
		transition: background-color 0.15s ease;
	}

	.resize-handle:hover,
	.resize-handle:active,
	.resize-handle:focus-visible {
		background-color: var(--color-accent, var(--color-accent));
		outline: none;
	}

	.resize-overlay {
		position: fixed;
		inset: 0;
		z-index: 9999;
		cursor: col-resize;
	}
</style>
