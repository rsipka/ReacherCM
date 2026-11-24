<script lang="ts">
	/**
	 * A server management tab's Logs page: the BMC's logs (Dell's SEL and
	 * Lifecycle log, HPE's IML and iLO event log...), newest first, with a
	 * filter, copy as text, and Clear for a log that can be cleared.
	 */
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { bmcLogServices, bmcLogEntries, bmcLogClear, type BmcLogService, type BmcLogEntry } from '#lib/ipc/bmc.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import { healthClass, healthLabel, when } from './bmc-format.js';

	interface Props {
		connectionId: string;
		server: string;
		/** Bumped by the page's Refresh. */
		refresh: number;
	}

	let { connectionId, server, refresh }: Props = $props();

	let services = $state<BmcLogService[] | null>(null);
	let selected = $state('');
	let entries = $state<BmcLogEntry[] | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let filter = $state<'all' | 'warning' | 'critical'>('all');
	let search = $state('');
	let confirmClear = $state(false);
	let clearing = $state(false);

	let service = $derived(services?.find((s) => s.path === selected) ?? null);
	let shown = $derived.by(() => {
		const q = search.trim().toLowerCase();
		return (entries ?? []).filter((e) => {
			if (filter === 'critical' && e.severity !== 'Critical') return false;
			if (filter === 'warning' && e.severity !== 'Critical' && e.severity !== 'Warning') return false;
			return !q || e.message.toLowerCase().includes(q) || (e.messageId ?? '').toLowerCase().includes(q) || (e.sensor ?? '').toLowerCase().includes(q);
		});
	});

	async function loadServices(): Promise<void> {
		loading = true;
		error = null;
		try {
			services = await bmcLogServices(connectionId);
			if (!services.some((s) => s.path === selected)) selected = services[0]?.path ?? '';
			await loadEntries();
		} catch (err) {
			error = String(err);
		} finally {
			loading = false;
		}
	}

	async function loadEntries(): Promise<void> {
		if (!selected) {
			entries = [];
			return;
		}
		loading = true;
		error = null;
		const path = selected;
		try {
			const got = await bmcLogEntries(connectionId, path);
			if (path === selected) entries = got;
		} catch (err) {
			error = String(err);
			entries = null;
		} finally {
			loading = false;
		}
	}

	async function copy(): Promise<void> {
		const text = shown.map((e) => [e.created ?? '', e.severity ?? '', e.messageId ?? '', e.message].join('\t')).join('\n');
		try {
			await navigator.clipboard.writeText(text);
			addToast(t('bmc.logs_copied', { count: String(shown.length) }), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	async function clear(): Promise<void> {
		if (!service) return;
		clearing = true;
		try {
			await bmcLogClear(connectionId, service.path);
			addToast(t('bmc.logs_cleared', { name: service.name }), 'success');
			confirmClear = false;
			await loadEntries();
		} catch (err) {
			addToast(String(err), 'error');
		} finally {
			clearing = false;
		}
	}

	// First shown, and the page's Refresh.
	$effect(() => {
		void refresh;
		void connectionId;
		void loadServices();
	});
</script>

<div class="logs">
	{#if services && services.length === 0}
		<p class="note">{t('bmc.logs_none')}</p>
	{:else}
		<div class="bar">
			<select class="sel" bind:value={selected} onchange={() => void loadEntries()} disabled={loading || !services} aria-label={t('bmc.log')}>
				{#each services ?? [] as s (s.path)}
					<option value={s.path}>{s.name}</option>
				{/each}
			</select>
			<select class="sel" bind:value={filter} aria-label={t('bmc.logs_filter')}>
				<option value="all">{t('bmc.logs_all')}</option>
				<option value="warning">{t('bmc.logs_warning')}</option>
				<option value="critical">{t('bmc.logs_critical')}</option>
			</select>
			<input class="search" type="search" placeholder={t('bmc.logs_search')} bind:value={search} />
			<span class="spacer"></span>
			<Button variant="secondary" size="sm" disabled={!shown.length} onclick={() => void copy()}>{t('bmc.logs_copy')}</Button>
			{#if service?.canClear}
				<Button variant="danger" size="sm" disabled={loading} onclick={() => (confirmClear = true)}>{t('bmc.logs_clear')}</Button>
			{/if}
		</div>
		{#if error}<p class="note error">{error}</p>{/if}
		{#if loading && !entries}
			<p class="note">{t('bmc.loading')}</p>
		{:else if entries}
			<p class="note small">{t('bmc.logs_count', { shown: String(shown.length), count: String(entries.length) })}</p>
			{#if shown.length === 0}
				<p class="note">{t('bmc.logs_empty')}</p>
			{:else}
				<div class="table-wrap">
					<table class="rf-table">
						<thead>
							<tr><th></th><th>{t('bmc.col_time')}</th><th>{t('bmc.col_message')}</th></tr>
						</thead>
						<tbody>
							{#each shown as e, i (i)}
								<tr>
									<td class="dot-cell"><span class="dot {healthClass(e.severity)}" title={healthLabel(e.severity)}></span></td>
									<td class="time">{when(e.created)}</td>
									<td>
										{e.message}
										{#if e.messageId || e.sensor}<span class="meta">{[e.sensor, e.messageId].filter(Boolean).join(' · ')}</span>{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		{/if}
	{/if}
</div>

<Modal open={confirmClear} onclose={() => (confirmClear = false)} title={t('bmc.logs_clear')} maxWidth="440px">
	<p class="note">{t('bmc.logs_clear_confirm', { name: service?.name ?? '', server })}</p>
	{#snippet actions()}
		<Button variant="secondary" onclick={() => (confirmClear = false)}>{t('common.cancel')}</Button>
		<Button variant="danger" disabled={clearing} onclick={() => void clear()}>{t('bmc.logs_clear')}</Button>
	{/snippet}
</Modal>

<style>
	.logs {
		display: flex;
		flex-direction: column;
		gap: 10px;
		min-width: 0;
	}
	.bar {
		display: flex;
		gap: 8px;
		align-items: center;
		flex-wrap: wrap;
	}
	.spacer {
		flex: 1;
	}
	.sel,
	.search {
		padding: 5px 8px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.search {
		min-width: 160px;
	}
	.table-wrap {
		overflow-x: auto;
		border: 1px solid var(--color-border);
		border-radius: 8px;
	}
	.rf-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 13px;
	}
	.rf-table th {
		text-align: left;
		font-weight: 500;
		font-size: 12px;
		color: var(--color-text-tertiary);
		padding: 6px 10px;
		border-bottom: 1px solid var(--color-border);
	}
	.rf-table td {
		padding: 5px 10px;
		border-top: 1px solid var(--color-border);
		vertical-align: top;
	}
	.time {
		white-space: nowrap;
		color: var(--color-text-secondary);
		font-size: 12px;
	}
	.meta {
		display: block;
		font-size: 11px;
		color: var(--color-text-tertiary);
	}
	.dot-cell {
		width: 14px;
		padding-top: 9px !important;
	}
	.dot {
		display: inline-block;
		width: 9px;
		height: 9px;
		border-radius: 50%;
	}
	.dot.ok {
		background: var(--color-success);
	}
	.dot.warn {
		background: var(--color-warning);
	}
	.dot.crit {
		background: var(--color-danger);
	}
	.dot.none {
		border: 1px solid var(--color-text-tertiary);
	}
	.note {
		margin: 0;
		font-size: 13px;
		color: var(--color-text-secondary);
	}
	.note.small {
		font-size: 12px;
		color: var(--color-text-tertiary);
	}
	.note.error {
		color: var(--color-danger);
		white-space: pre-wrap;
	}
</style>
