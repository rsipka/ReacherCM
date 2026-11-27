<script lang="ts">
	/**
	 * The DevOps workspaces in the Sessions list, under the sessions: each
	 * switched-on tool opens in a tab, and every saved Kubernetes cluster is a
	 * row of its own that opens that cluster's tab. They used to be pages picked
	 * from the start of the tab bar (see #lib/state/navigation).
	 */
	import { getEnabledDevopsTools, type DevopsTool } from '#lib/state/devops.svelte.js';
	import { openDevopsTab } from '#lib/state/navigation.svelte.js';
	import { getActiveTab, getTabs } from '#lib/state/tabs.svelte.js';
	import { getClustersVersion } from '#lib/state/k8s.svelte.js';
	import { k8sClusters, type ClusterView } from '#lib/ipc/k8s.js';
	import EnvBadge from '#lib/components/devops/EnvBadge.svelte';
	import { t } from '#lib/state/i18n.svelte.js';

	interface Props {
		/** The Sessions list's search: rows that do not match it are hidden. */
		search?: string;
	}

	let { search = '' }: Props = $props();

	const STORAGE_KEY = 'reach.sessions.devops';

	function readFolded(): { group: boolean; clusters: boolean } {
		try {
			const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
			return { group: !!raw.group, clusters: !!raw.clusters };
		} catch {
			return { group: false, clusters: false };
		}
	}

	let folded = $state(readFolded());

	function fold(key: 'group' | 'clusters'): void {
		folded[key] = !folded[key];
		try {
			localStorage.setItem(STORAGE_KEY, JSON.stringify(folded));
		} catch {
			// Not remembered then.
		}
	}

	let tools = $derived(getEnabledDevopsTools());
	let k8sOn = $derived(tools.some((d) => d.id === 'kubernetes'));

	let clusters = $state<ClusterView[]>([]);
	$effect(() => {
		void getClustersVersion();
		if (!k8sOn) {
			clusters = [];
			return;
		}
		k8sClusters()
			.then((list) => (clusters = [...list].sort((a, b) => a.name.localeCompare(b.name))))
			.catch(() => (clusters = []));
	});

	let query = $derived(search.trim().toLowerCase());
	function matches(text: string): boolean {
		return !query || text.toLowerCase().includes(query);
	}

	let shownClusters = $derived(clusters.filter((c) => matches(c.name) || matches(t('nav.kubernetes'))));
	let shownTools = $derived(tools.filter((d) => matches(d.label()) || (d.id === 'kubernetes' && shownClusters.length > 0)));

	let tabs = $derived(getTabs());
	let active = $derived(getActiveTab());

	/** Open in a tab, and which one is in front, for the rows' tint. */
	function toolState(tool: DevopsTool): { open: boolean; current: boolean } {
		const mine = tabs.filter((x) => x.type === 'devops' && x.devopsTool === tool.id && (tool.id !== 'kubernetes' || !x.k8sClusterId));
		return { open: mine.length > 0, current: mine.some((x) => x.id === active?.id) };
	}

	function clusterState(id: string): { open: boolean; current: boolean } {
		const mine = tabs.filter((x) => x.type === 'devops' && x.devopsTool === 'kubernetes' && x.k8sClusterId === id);
		return { open: mine.length > 0, current: mine.some((x) => x.id === active?.id) };
	}
</script>

{#if shownTools.length > 0}
	<div class="devops-group" role="group" aria-label={t('devops.sessions_group')}>
		<button class="group-header" onclick={() => fold('group')} aria-expanded={query !== '' || !folded.group}>
			<svg width="10" height="10" viewBox="0 0 10 10" fill="none" class="chevron" class:collapsed={query === '' && folded.group} aria-hidden="true"><path d="M3 2l4 3-4 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
			<span class="group-name">{t('devops.sessions_group')}</span>
			<span class="group-count">{shownTools.length}</span>
		</button>
		{#if query !== '' || !folded.group}
			{#each shownTools as tool (tool.id)}
				{@const st = toolState(tool)}
				<div class="row-line">
					<button class="row" class:open={st.open} class:current={st.current} onclick={() => openDevopsTab(tool.page)} title={tool.description()}>
						<svg class="row-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
							<path d={tool.icon} stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
						</svg>
						<span class="row-name">{tool.label()}</span>
					</button>
					{#if tool.id === 'kubernetes' && clusters.length > 0}
						<button class="fold-btn" onclick={() => fold('clusters')} aria-expanded={query !== '' || !folded.clusters} aria-label={t('devops.k8s_clusters_toggle')} title={t('devops.k8s_clusters_toggle')}>
							<svg width="10" height="10" viewBox="0 0 10 10" fill="none" class="chevron" class:collapsed={query === '' && folded.clusters} aria-hidden="true"><path d="M3 2l4 3-4 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
							<span class="group-count">{clusters.length}</span>
						</button>
					{/if}
				</div>
				{#if tool.id === 'kubernetes' && (query !== '' || !folded.clusters)}
					{#each shownClusters as cluster (cluster.id)}
						{@const cs = clusterState(cluster.id)}
						<button class="row cluster" class:open={cs.open} class:current={cs.current} onclick={() => openDevopsTab('kubernetes', cluster.id, cluster.name)} title={t('devops.k8s_open_cluster', { name: cluster.name })}>
							<span class="dot" class:on={cs.open} aria-hidden="true"></span>
							<span class="row-name">{cluster.name}</span>
							<EnvBadge environment={cluster.environment} />
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

	.row.cluster {
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
