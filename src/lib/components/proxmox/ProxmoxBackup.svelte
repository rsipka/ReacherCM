<script lang="ts">
	/**
	 * A Proxmox Backup Server tab's own pages. *Overview*: the server (with
	 * its shell, reboot and shutdown) and each datastore's usage, with garbage
	 * collection and verifying. *Datastores*: the backups in a datastore, by
	 * namespace and group; a group opens its snapshots, which can be
	 * verified, protected and forgotten.
	 */
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import {
		pbsOverview,
		pbsNamespaces,
		pbsGroups,
		pbsSnapshots,
		pbsGc,
		pbsVerify,
		pbsProtect,
		pbsForget,
		type PbsOverview,
		type PbsDatastore,
		type PbsGroup,
		type PbsSnapshot
	} from '#lib/ipc/proxmox.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import { bytes, share, cpu, uptime, when, proxmoxError } from './proxmox-format.js';

	interface Props {
		connectionId: string;
		page: 'overview' | 'datastores';
		/** Bumped to read the page again. */
		refresh: number;
		/** The tab is in view: the page reads on. */
		active: boolean;
		/** Show the Datastores page (on the datastore just picked). */
		onbrowse: () => void;
		onshell: (name: string) => void;
		onnodeaction: (command: 'reboot' | 'shutdown', name: string) => void;
	}

	let { connectionId, page, refresh, active, onbrowse, onshell, onnodeaction }: Props = $props();

	const REFRESH_MS = 15_000;

	let overview = $state<PbsOverview | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);

	/** The datastore and namespace the Datastores page shows. */
	let store = $state('');
	let ns = $state('');
	let namespaces = $state<string[]>(['']);
	let groups = $state<PbsGroup[] | null>(null);
	let groupError = $state<string | null>(null);
	let filter = $state('');

	/** The group whose snapshots are open. */
	let group = $state<PbsGroup | null>(null);
	let snaps = $state<PbsSnapshot[] | null>(null);
	let snapError = $state<string | null>(null);
	let forgetting = $state<PbsSnapshot | null>(null);

	let hostname = $derived(overview?.node.name || 'localhost');
	let shownGroups = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		const all = groups ?? [];
		return q ? all.filter((g) => `${g.kind}/${g.id} ${g.owner ?? ''} ${g.comment ?? ''}`.toLowerCase().includes(q)) : all;
	});

	async function loadOverview(): Promise<void> {
		try {
			overview = await pbsOverview(connectionId);
			error = null;
			if (!store || !overview.datastores.some((d) => d.store === store)) store = overview.datastores[0]?.store ?? '';
		} catch (err) {
			error = proxmoxError(err);
		}
	}

	async function loadGroups(): Promise<void> {
		const s = store;
		if (!s) return;
		try {
			const [list, spaces] = await Promise.all([pbsGroups(connectionId, s, ns), pbsNamespaces(connectionId, s).catch(() => [''])]);
			if (s !== store) return;
			groups = list;
			namespaces = spaces;
			groupError = null;
		} catch (err) {
			groups = null;
			groupError = proxmoxError(err);
		}
	}

	async function loadSnaps(): Promise<void> {
		const g = group;
		if (!g) return;
		try {
			const list = await pbsSnapshots(connectionId, store, ns, g.kind, g.id);
			if (group !== g) return;
			snaps = list;
			snapError = null;
		} catch (err) {
			snapError = proxmoxError(err);
		}
	}

	async function run(what: string, target: string, job: () => Promise<unknown>, after?: () => Promise<void>): Promise<void> {
		busy = true;
		try {
			await job();
			addToast(t('proxmox.task_started', { action: what, guest: target }), 'success');
			await after?.();
		} catch (err) {
			addToast(proxmoxError(err), 'error');
		} finally {
			busy = false;
		}
	}

	function browse(d: PbsDatastore): void {
		if (store !== d.store) {
			store = d.store;
			ns = '';
			groups = null;
		}
		onbrowse();
	}

	function openGroup(g: PbsGroup): void {
		group = g;
		snaps = null;
		snapError = null;
		void loadSnaps();
	}

	function snapLabel(s: PbsSnapshot): string {
		return `${s.kind}/${s.id} ${when(s.time)}`;
	}

	async function protect(s: PbsSnapshot): Promise<void> {
		busy = true;
		try {
			await pbsProtect(connectionId, store, ns, s.kind, s.id, s.time, !s.protected);
			await loadSnaps();
		} catch (err) {
			addToast(proxmoxError(err), 'error');
		} finally {
			busy = false;
		}
	}

	async function forget(): Promise<void> {
		const s = forgetting;
		forgetting = null;
		if (!s) return;
		busy = true;
		try {
			await pbsForget(connectionId, store, ns, s.kind, s.id, s.time);
			addToast(t('proxmox.pbs_forgotten', { snapshot: snapLabel(s) }), 'success');
			await Promise.all([loadSnaps(), loadGroups()]);
		} catch (err) {
			addToast(proxmoxError(err), 'error');
		} finally {
			busy = false;
		}
	}

	// The overview (which also lists the datastores): at first, on a bump, and on.
	$effect(() => {
		void refresh;
		void connectionId;
		void loadOverview();
	});

	// The groups of the datastore and namespace picked.
	$effect(() => {
		if (page !== 'datastores' || !store) return;
		void ns;
		void refresh;
		void loadGroups();
	});

	$effect(() => {
		if (!active) return;
		const which = page;
		const timer = setInterval(() => {
			if (busy || document.visibilityState !== 'visible') return;
			if (which === 'overview') void loadOverview();
			else if (!group) void loadGroups();
		}, REFRESH_MS);
		return () => clearInterval(timer);
	});
</script>

{#snippet bar(used: number | null | undefined, total: number | null | undefined)}
	{@const pct = share(used, total)}
	<span class="bar" title={pct === null ? '' : `${pct.toFixed(0)} %`}>
		<span class="fill" class:high={pct !== null && pct >= 90} style:width={`${pct ?? 0}%`}></span>
	</span>
{/snippet}

{#if error}<p class="note error">{error}</p>{/if}
{#if !overview && !error}<p class="note">{t('proxmox.loading')}</p>{/if}

{#if overview && page === 'overview'}
	{@const n = overview.node}
	<section class="card">
		<div class="head">
			<span class="dot ok"></span>
			<span class="strong">{hostname}</span>
			<span class="dim">{[n.kernel].filter(Boolean).join(' · ')}</span>
			<span class="spacer"></span>
			<Button variant="secondary" size="sm" onclick={() => onshell(hostname)}>{t('proxmox.shell')}</Button>
			<Button variant="ghost" size="sm" onclick={() => onnodeaction('reboot', hostname)}>{t('proxmox.reboot')}</Button>
			<Button variant="ghost" size="sm" onclick={() => onnodeaction('shutdown', hostname)}>{t('proxmox.shutdown')}</Button>
		</div>
		<dl class="facts">
			<dt>{t('proxmox.cpu')}</dt>
			<dd>{@render bar((n.cpu ?? 0) * 100, 100)} {cpu(n.cpu)} · {n.cpus ?? '—'} × {n.cpuModel ?? ''}</dd>
			<dt>{t('proxmox.load')}</dt>
			<dd class="mono">{n.load.length ? n.load.map((l) => l.toFixed(2)).join(' ') : '—'}</dd>
			<dt>{t('proxmox.memory')}</dt>
			<dd>{@render bar(n.memUsed, n.memTotal)} {bytes(n.memUsed)} / {bytes(n.memTotal)}</dd>
			{#if n.swapTotal}
				<dt>{t('proxmox.swap')}</dt>
				<dd>{@render bar(n.swapUsed, n.swapTotal)} {bytes(n.swapUsed)} / {bytes(n.swapTotal)}</dd>
			{/if}
			<dt>{t('proxmox.root_disk')}</dt>
			<dd>{@render bar(n.rootUsed, n.rootTotal)} {bytes(n.rootUsed)} / {bytes(n.rootTotal)}</dd>
			<dt>{t('proxmox.uptime')}</dt>
			<dd>{uptime(n.uptime)}</dd>
		</dl>
	</section>

	{#if overview.datastores.length === 0}
		<p class="note">{t('proxmox.pbs_no_datastores')}</p>
	{/if}
	{#each overview.datastores as d (d.store)}
		<section class="card">
			<div class="head">
				<span class="dot {d.error ? 'crit' : d.maintenance ? 'warn' : 'ok'}"></span>
				<span class="strong">{d.store}</span>
				{#if d.comment}<span class="dim">{d.comment}</span>{/if}
				{#if d.maintenance}<span class="tag">{t('proxmox.pbs_maintenance', { mode: d.maintenance })}</span>{/if}
				<span class="spacer"></span>
				<Button variant="secondary" size="sm" onclick={() => browse(d)}>{t('proxmox.pbs_browse')}</Button>
				<Button variant="ghost" size="sm" disabled={busy || !!d.error} onclick={() => void run(t('proxmox.pbs_gc'), d.store, () => pbsGc(connectionId, d.store), loadOverview)}>{t('proxmox.pbs_gc')}</Button>
				<Button variant="ghost" size="sm" disabled={busy || !!d.error} onclick={() => void run(t('proxmox.pbs_verify_all'), d.store, () => pbsVerify(connectionId, d.store, ''))}>{t('proxmox.pbs_verify_all')}</Button>
			</div>
			{#if d.error}
				<p class="note error">{d.error}</p>
			{:else}
				<dl class="facts">
					<dt>{t('proxmox.col_usage')}</dt>
					<dd>{@render bar(d.used, d.total)} {bytes(d.used)} / {bytes(d.total)} · {t('proxmox.pbs_free', { size: bytes(d.avail) })}</dd>
					{#if d.fullAt}
						<dt>{t('proxmox.pbs_full_at')}</dt>
						<dd>{new Date(d.fullAt * 1000).toLocaleDateString()}</dd>
					{/if}
					{#if d.gcPending !== null || d.gcRemoved !== null}
						<dt>{t('proxmox.pbs_gc_last')}</dt>
						<dd>{t('proxmox.pbs_gc_bytes', { removed: bytes(d.gcRemoved), pending: bytes(d.gcPending) })}</dd>
					{/if}
				</dl>
			{/if}
		</section>
	{/each}
{:else if overview && page === 'datastores'}
	{#if overview.datastores.length === 0}
		<p class="note">{t('proxmox.pbs_no_datastores')}</p>
	{:else}
		<div class="toolbar">
			<label class="pick">
				<span>{t('proxmox.pbs_datastore')}</span>
				<select bind:value={store} onchange={() => { ns = ''; groups = null; }}>
					{#each overview.datastores as d (d.store)}
						<option value={d.store}>{d.store}</option>
					{/each}
				</select>
			</label>
			{#if namespaces.length > 1}
				<label class="pick">
					<span>{t('proxmox.pbs_namespace')}</span>
					<select bind:value={ns} onchange={() => (groups = null)}>
						{#each namespaces as n (n)}
							<option value={n}>{n || t('proxmox.pbs_root_ns')}</option>
						{/each}
					</select>
				</label>
			{/if}
			<input class="filter" type="search" bind:value={filter} placeholder={t('proxmox.pbs_filter')} autocomplete="off" spellcheck="false" />
		</div>
		{#if groupError}<p class="note error">{groupError}</p>{/if}
		{#if !groups && !groupError}<p class="note">{t('proxmox.loading')}</p>{/if}
		{#if groups && shownGroups.length === 0}<p class="note">{t('proxmox.pbs_no_groups')}</p>{/if}
		{#if groups && shownGroups.length}
			<div class="table-box">
				<table>
					<thead>
						<tr>
							<th>{t('proxmox.pbs_group')}</th>
							<th>{t('proxmox.pbs_last_backup')}</th>
							<th class="r">{t('proxmox.pbs_count')}</th>
							<th class="hide-narrow">{t('proxmox.pbs_owner')}</th>
						</tr>
					</thead>
					<tbody>
						{#each shownGroups as g (`${g.kind}/${g.id}`)}
							<tr class="row" onclick={() => openGroup(g)}>
								<td class="wide"><span class="kind-tag">{g.kind.toUpperCase()}</span>{g.id}{#if g.comment} <span class="dim">{g.comment}</span>{/if}</td>
								<td>{when(g.last)}</td>
								<td class="r mono">{g.count}</td>
								<td class="hide-narrow dim">{g.owner ?? ''}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	{/if}
{/if}

<Modal open={group !== null} onclose={() => (group = null)} title={group ? `${group.kind}/${group.id}` : ''} maxWidth="720px">
	{#if group}
		<div class="snaps-box">
			<p class="dim">{[store, ns || null].filter(Boolean).join(' · ')}</p>
			{#if snapError}<p class="note error">{snapError}</p>{/if}
			{#if !snaps && !snapError}<p class="note">{t('proxmox.loading')}</p>{/if}
			{#if snaps && snaps.length === 0}<p class="note">{t('proxmox.snap_none')}</p>{/if}
			{#if snaps && snaps.length}
				<ul class="snaps">
					{#each snaps as s (s.time)}
						<li>
							<div class="snap-text">
								<span class="strong">{when(s.time)}</span>
								<span class="dim">
									{bytes(s.size)}
									{#if s.verified === 'ok'} · <span class="ok-text">{t('proxmox.pbs_verified')}</span>{:else if s.verified} · <span class="bad-text">{t('proxmox.pbs_verify_failed')}</span>{:else} · {t('proxmox.pbs_not_verified')}{/if}
									{#if s.encrypted} · {t('proxmox.pbs_encrypted')}{/if}
									{#if s.comment} · {s.comment}{/if}
								</span>
							</div>
							{#if s.protected}<span class="tag" title={t('proxmox.pbs_protected_hint')}>🛡 {t('proxmox.pbs_protected')}</span>{/if}
							<Button variant="ghost" size="sm" disabled={busy} onclick={() => void run(t('proxmox.pbs_verify'), snapLabel(s), () => pbsVerify(connectionId, store, ns, { kind: s.kind, id: s.id, time: s.time }))}>{t('proxmox.pbs_verify')}</Button>
							<Button variant="ghost" size="sm" disabled={busy} onclick={() => void protect(s)}>{s.protected ? t('proxmox.pbs_unprotect') : t('proxmox.pbs_protect')}</Button>
							<Button variant="ghost" size="sm" disabled={busy || s.protected} onclick={() => (forgetting = s)}>{t('proxmox.pbs_forget')}</Button>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}
</Modal>

<Modal open={forgetting !== null} onclose={() => (forgetting = null)} title={t('proxmox.confirm_title')} maxWidth="420px" zIndex={110}>
	{#if forgetting}
		<div class="dialog">
			<p class="note">{t('proxmox.pbs_confirm_forget', { snapshot: snapLabel(forgetting), store })}</p>
			<div class="actions">
				<Button variant="ghost" size="sm" onclick={() => (forgetting = null)}>{t('common.cancel')}</Button>
				<Button variant="danger" size="sm" onclick={() => void forget()}>{t('proxmox.pbs_forget')}</Button>
			</div>
		</div>
	{/if}
</Modal>

<style>
	.card {
		border: 1px solid var(--color-border);
		border-radius: 8px;
		padding: 10px 12px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		max-width: 760px;
	}
	.head {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		font-size: 13px;
	}
	.strong {
		font-weight: 600;
	}
	.spacer {
		flex: 1;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 6px 18px;
		margin: 0;
		font-size: 13px;
	}
	.facts dt {
		color: var(--color-text-tertiary);
	}
	.facts dd {
		margin: 0;
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		word-break: break-word;
	}
	.bar {
		display: inline-block;
		width: 90px;
		height: 6px;
		border-radius: 3px;
		background: var(--color-border);
		overflow: hidden;
		flex-shrink: 0;
	}
	.fill {
		display: block;
		height: 100%;
		background: var(--color-accent);
	}
	.fill.high {
		background: var(--color-danger, #d64545);
	}
	.toolbar {
		display: flex;
		align-items: flex-end;
		gap: 10px;
		flex-wrap: wrap;
	}
	.pick {
		display: flex;
		flex-direction: column;
		gap: 3px;
		font-size: 12px;
		color: var(--color-text-tertiary);
	}
	.pick select,
	.filter {
		padding: 6px 9px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.filter {
		flex: 1;
		max-width: 320px;
		min-width: 160px;
	}
	.table-box {
		border: 1px solid var(--color-border);
		border-radius: 8px;
		overflow-x: auto;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 13px;
	}
	th {
		text-align: left;
		font-weight: 500;
		font-size: 12px;
		color: var(--color-text-tertiary);
		padding: 6px 10px;
		border-bottom: 1px solid var(--color-border);
		white-space: nowrap;
	}
	td {
		padding: 6px 10px;
		border-top: 1px solid var(--color-border);
		white-space: nowrap;
	}
	td.wide {
		width: 100%;
		white-space: normal;
	}
	tr.row {
		cursor: pointer;
	}
	tr.row:hover td {
		background: var(--color-bg-hover, rgba(127, 127, 127, 0.08));
	}
	.r {
		text-align: right;
	}
	.mono {
		font-family: var(--font-mono, monospace);
		font-size: 12px;
	}
	.dim {
		color: var(--color-text-tertiary);
		font-size: 12px;
		margin: 0;
	}
	.kind-tag {
		display: inline-block;
		font-size: 10px;
		padding: 0 5px;
		margin-right: 6px;
		border-radius: 4px;
		border: 1px solid var(--color-border);
		color: var(--color-text-tertiary);
	}
	.tag {
		display: inline-block;
		font-size: 11px;
		padding: 0 6px;
		border-radius: 999px;
		background: var(--color-surface-sunken, rgba(127, 127, 127, 0.12));
		color: var(--color-text-secondary);
		white-space: nowrap;
	}
	.dot {
		display: inline-block;
		width: 9px;
		height: 9px;
		border-radius: 50%;
		flex-shrink: 0;
	}
	.dot.ok {
		background: var(--color-success, #2e9d5b);
	}
	.dot.warn {
		background: var(--color-warning, #d99a1e);
	}
	.dot.crit {
		background: var(--color-danger, #d64545);
	}
	.ok-text {
		color: var(--color-success, #2e9d5b);
	}
	.bad-text {
		color: var(--color-danger, #d64545);
	}
	.note {
		margin: 0;
		font-size: 13px;
		color: var(--color-text-secondary);
	}
	.note.error {
		color: var(--color-danger);
		white-space: pre-wrap;
	}
	.snaps-box,
	.dialog {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.snaps {
		list-style: none;
		margin: 0;
		padding: 0;
		border: 1px solid var(--color-border);
		border-radius: 8px;
	}
	.snaps li {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 6px 10px;
		font-size: 13px;
		flex-wrap: wrap;
	}
	.snaps li + li {
		border-top: 1px solid var(--color-border);
	}
	.snap-text {
		flex: 1;
		min-width: 160px;
		display: flex;
		flex-direction: column;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}
	@media (max-width: 640px) {
		.hide-narrow {
			display: none;
		}
		.facts {
			grid-template-columns: 1fr;
			gap: 2px;
		}
	}
</style>
