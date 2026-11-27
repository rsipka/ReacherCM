<script lang="ts">
	/**
	 * The DevOps workspaces in the Sessions list, under the sessions: each
	 * switched-on tool, with what it holds beneath it. Kubernetes clusters and
	 * container hosts open a tab each; database connections open in the
	 * Databases tab, which has its own tabs for queries and tables; Ansible and
	 * OpenTofu projects open in their workspace's tab. They used to be pages
	 * picked from the start of the tab bar (see #lib/state/navigation).
	 */
	import { getEnabledDevopsTools, type DevopsTool, type DevopsToolId } from '#lib/state/devops.svelte.js';
	import { openDevopsTab, tabTarget } from '#lib/state/navigation.svelte.js';
	import { getActiveTab, getTabs } from '#lib/state/tabs.svelte.js';
	import { getClustersVersion } from '#lib/state/k8s.svelte.js';
	import { getHostsVersion } from '#lib/state/containers.svelte.js';
	import { k8sClusters } from '#lib/ipc/k8s.js';
	import { ctrHosts } from '#lib/ipc/containers.js';
	import * as dbState from '#lib/state/db.svelte.js';
	import * as ansible from '#lib/state/ansible.svelte.js';
	import * as tofu from '#lib/state/tofu.svelte.js';
	import { vaultState } from '#lib/state/vault.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import EnvBadge from '#lib/components/devops/EnvBadge.svelte';
	import type { Environment } from '#lib/ipc/containers.js';
	import { t } from '#lib/state/i18n.svelte.js';

	interface Props {
		/** The Sessions list's search: rows that do not match it are hidden. */
		search?: string;
	}

	let { search = '' }: Props = $props();

	/** One thing a tool holds: a cluster, a host, a connection, a project. */
	interface Item {
		id: string;
		name: string;
		environment?: Environment;
		/** Open somewhere now, and that tab in front. */
		open: boolean;
		current: boolean;
	}

	const STORAGE_KEY = 'reach.sessions.devops';

	/** Folded: the group ('group') and each tool's list (its id). */
	function readFolded(): Record<string, boolean> {
		try {
			const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
			return raw && typeof raw === 'object' ? raw : {};
		} catch {
			return {};
		}
	}

	let folded = $state<Record<string, boolean>>(readFolded());

	function isFolded(key: string): boolean {
		return query === '' && !!folded[key];
	}

	function fold(key: string): void {
		folded[key] = !isFolded(key);
		try {
			localStorage.setItem(STORAGE_KEY, JSON.stringify(folded));
		} catch {
			// Not remembered then.
		}
	}

	let tools = $derived(getEnabledDevopsTools());
	let on = $derived(new Set(tools.map((d) => d.id)));

	// ---- What each tool holds ----------------------------------------------

	let clusters = $state<{ id: string; name: string; environment: Environment }[]>([]);
	$effect(() => {
		void getClustersVersion();
		if (!on.has('kubernetes')) {
			clusters = [];
			return;
		}
		k8sClusters()
			.then((list) => (clusters = (list ?? []).map((c) => ({ id: c.id, name: c.name, environment: c.environment }))))
			.catch(() => (clusters = []));
	});

	let hosts = $state<{ id: string; name: string; environment: Environment }[]>([]);
	$effect(() => {
		void getHostsVersion();
		if (!on.has('containers')) {
			hosts = [];
			return;
		}
		ctrHosts()
			.then((list) => (hosts = (list ?? []).map((h) => ({ id: h.id, name: h.name, environment: h.environment }))))
			.catch(() => (hosts = []));
	});

	$effect(() => {
		if (on.has('databases') && !dbState.isLoaded()) void dbState.loadConnections();
	});

	// Projects live in the vault: read once it is open, again for another user.
	$effect(() => {
		if (vaultState.locked) return;
		void vaultState.userUuid;
		if (on.has('ansible')) void ansible.loadProjects();
		if (on.has('tofu')) void tofu.loadProjects();
	});

	let tabs = $derived(getTabs());
	let active = $derived(getActiveTab());

	function tabsOf(tool: DevopsToolId) {
		return tabs.filter((x) => x.type === 'devops' && x.devopsTool === tool);
	}

	function itemsOf(tool: DevopsToolId): Item[] {
		const mine = tabsOf(tool);
		const inFront = mine.some((x) => x.id === active?.id);
		const byName = (a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name);
		switch (tool) {
			case 'kubernetes':
			case 'containers':
				return [...(tool === 'kubernetes' ? clusters : hosts)].sort(byName).map((x) => {
					const its = mine.filter((tab) => tabTarget(tab) === x.id);
					return { ...x, open: its.length > 0, current: its.some((tab) => tab.id === active?.id) };
				});
			case 'databases':
				return [...(dbState.getConnections() ?? [])].sort(byName).map((c) => ({
					id: c.id,
					name: c.name,
					open: !!dbState.getOpen(c.id),
					current: inFront && !!dbState.getOpen(c.id)
				}));
			case 'ansible':
			case 'tofu': {
				const st = tool === 'ansible' ? ansible : tofu;
				const shown = st.getActiveProjectId();
				return [...(st.getProjects() ?? [])].sort(byName).map((p) => ({
					id: p.id,
					name: p.name,
					open: mine.length > 0 && p.id === shown,
					current: inFront && p.id === shown
				}));
			}
		}
	}

	// ---- Opening ---------------------------------------------------------------

	async function openItem(tool: DevopsTool, item: Item): Promise<void> {
		switch (tool.id) {
			case 'kubernetes':
			case 'containers':
				openDevopsTab(tool.id, item.id, item.name);
				return;
			case 'databases': {
				openDevopsTab('databases');
				if (dbState.getOpen(item.id)) {
					if (dbState.engineOf(item.id) === 'redis') dbState.openRedis(item.id);
					return;
				}
				if (dbState.isConnecting(item.id)) return;
				const info = await dbState.openConnection(item.id);
				if (info?.engine === 'redis') dbState.openRedis(item.id);
				return;
			}
			case 'ansible':
			case 'tofu': {
				const st = tool.id === 'ansible' ? ansible : tofu;
				openDevopsTab(tool.id);
				if (st.getActiveProjectId() === item.id) return;
				// A run goes on in its project; switching would hide it.
				if (st.isCommandRunning()) {
					addToast(t('devops.project_busy'), 'warning');
					return;
				}
				try {
					await st.openProject(item.id);
				} catch (e) {
					addToast(String(e), 'error');
				}
				return;
			}
		}
	}

	/** A tool's own row: its tab (a Kubernetes or Containers tab at its picker). */
	function openTool(tool: DevopsTool): void {
		if (tool.id === 'kubernetes' || tool.id === 'containers') {
			const picker = tabsOf(tool.id).find((x) => !tabTarget(x));
			if (picker || tabsOf(tool.id).length === 0) {
				openDevopsTab(tool.id);
				return;
			}
		}
		openDevopsTab(tool.page);
	}

	function toolState(tool: DevopsTool): { open: boolean; current: boolean } {
		const mine = tabsOf(tool.id);
		return { open: mine.length > 0, current: mine.some((x) => x.id === active?.id) };
	}

	// ---- Search ------------------------------------------------------------------

	let query = $derived(search.trim().toLowerCase());
	function matches(text: string): boolean {
		return !query || text.toLowerCase().includes(query);
	}

	let rows = $derived(
		tools
			.map((tool) => {
				const all = itemsOf(tool.id);
				const toolMatches = matches(tool.label());
				const items = toolMatches ? all : all.filter((i) => matches(i.name));
				return { tool, all, items, shown: toolMatches || items.length > 0 };
			})
			.filter((r) => r.shown)
	);
</script>

{#if rows.length > 0}
	<div class="devops-group" role="group" aria-label={t('devops.sessions_group')}>
		<button class="group-header" onclick={() => fold('group')} aria-expanded={!isFolded('group')}>
			<svg width="10" height="10" viewBox="0 0 10 10" fill="none" class="chevron" class:collapsed={isFolded('group')} aria-hidden="true"><path d="M3 2l4 3-4 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
			<span class="group-name">{t('devops.sessions_group')}</span>
			<span class="group-count">{rows.length}</span>
		</button>
		{#if !isFolded('group')}
			{#each rows as { tool, all, items } (tool.id)}
				{@const st = toolState(tool)}
				<div class="row-line" data-tool={tool.id}>
					<button class="row" class:open={st.open} class:current={st.current} onclick={() => openTool(tool)} title={tool.description()}>
						<svg class="row-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
							<path d={tool.icon} stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
						</svg>
						<span class="row-name">{tool.label()}</span>
					</button>
					{#if all.length > 0}
						<button class="fold-btn" onclick={() => fold(tool.id)} aria-expanded={!isFolded(tool.id)} aria-label={t('devops.items_toggle', { name: tool.label() })} title={t('devops.items_toggle', { name: tool.label() })}>
							<svg width="10" height="10" viewBox="0 0 10 10" fill="none" class="chevron" class:collapsed={isFolded(tool.id)} aria-hidden="true"><path d="M3 2l4 3-4 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
							<span class="group-count">{all.length}</span>
						</button>
					{/if}
				</div>
				{#if !isFolded(tool.id)}
					{#each items as item (item.id)}
						<button class="row item" data-tool={tool.id} class:open={item.open} class:current={item.current} onclick={() => void openItem(tool, item)} title={t('devops.open_item', { name: item.name })}>
							<span class="dot" class:on={item.open} aria-hidden="true"></span>
							<span class="row-name">{item.name}</span>
							{#if item.environment}<EnvBadge environment={item.environment} />{/if}
						</button>
					{/each}
				{/if}
			{/each}
		{/if}
	</div>
{/if}

<style>
	.devops-group {
		margin-top: 6px;
		padding-top: 4px;
		border-top: 1px solid var(--color-border);
	}

	.group-header,
	.fold-btn {
		display: flex;
		align-items: center;
		gap: 5px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		cursor: pointer;
	}

	.group-header {
		width: 100%;
		padding: 7px 10px;
	}

	.fold-btn {
		padding: 4px 6px;
		flex-shrink: 0;
	}

	.group-header:hover,
	.fold-btn:hover {
		background-color: var(--color-surface-hover);
	}

	.chevron {
		transition: transform 0.15s ease;
		transform: rotate(90deg);
	}

	.chevron.collapsed {
		transform: rotate(0deg);
	}

	.group-name {
		font-weight: 500;
		color: var(--color-text-primary);
	}

	.group-count {
		font-size: var(--text-2xs);
		opacity: 0.6;
	}

	.row-line {
		display: flex;
		align-items: center;
	}

	.row {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 6px 8px 6px 20px;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: var(--color-text-primary);
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		text-align: left;
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.row.item {
		padding-left: 40px;
	}

	.row:hover {
		background-color: var(--color-surface-hover);
	}

	/* Open in a tab: tinted, as an open session is; the one in front more. */
	.row.open {
		background-color: color-mix(in srgb, var(--color-accent) 8%, transparent);
	}

	.row.current {
		background-color: color-mix(in srgb, var(--color-accent) 16%, transparent);
	}

	.row-icon {
		flex-shrink: 0;
		color: var(--color-text-secondary);
	}

	.row-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		flex-shrink: 0;
		background: var(--color-border);
	}

	.dot.on {
		background: var(--color-success, #3fb950);
	}

	@media (pointer: coarse) {
		.row {
			padding-top: 10px;
			padding-bottom: 10px;
		}
	}
</style>
