<script lang="ts">
	/**
	 * A server management tab's Tasks page: the power button, boot once,
	 * the identify light, virtual media, restarting the BMC, and its jobs.
	 * Everything that touches the running server asks first.
	 */
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import {
		bmcOverview,
		bmcPower,
		bmcBootOnce,
		bmcIdentify,
		bmcVirtualMedia,
		bmcMediaInsert,
		bmcMediaEject,
		bmcResetBmc,
		bmcJobs,
		type BmcOverview,
		type BmcVirtualMedia,
		type BmcJob
	} from '#lib/ipc/bmc.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import Toggle from '#lib/components/shared/Toggle.svelte';
	import { word, when } from './bmc-format.js';

	interface Props {
		connectionId: string;
		server: string;
		refresh: number;
		/** Something on the server changed (power, boot once). */
		onchanged: () => void;
		/** The BMC is restarting: this login is gone. */
		onbmcrestart: () => void;
	}

	let { connectionId, server, refresh, onchanged, onbmcrestart }: Props = $props();

	/** The power actions in the order they are shown; others the BMC offers follow. */
	const POWER_ORDER = ['On', 'GracefulShutdown', 'GracefulRestart', 'ForceOff', 'ForceRestart', 'PowerCycle', 'PushPowerButton', 'Nmi', 'ForceOn'];
	/** Actions that cut the server off like pulling its plug. */
	const HARSH = new Set(['ForceOff', 'ForceRestart', 'PowerCycle', 'Nmi']);

	let overview = $state<BmcOverview | null>(null);
	let media = $state<BmcVirtualMedia[] | null>(null);
	let jobs = $state<BmcJob[] | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let bootTarget = $state('');
	let images = $state<Record<string, string>>({});
	/** What waits for a yes: a power action, or restarting the BMC. */
	let confirm = $state<{ kind: 'power'; reset: string } | { kind: 'bmc' } | null>(null);

	let powerActions = $derived(
		[...(overview?.resetTypes ?? [])].sort((a, b) => {
			const i = POWER_ORDER.indexOf(a);
			const j = POWER_ORDER.indexOf(b);
			return (i < 0 ? 99 : i) - (j < 0 ? 99 : j);
		})
	);
	let bootTargets = $derived((overview?.bootTargets ?? []).filter((b) => b !== 'None'));
	let bootSet = $derived(!!overview?.bootEnabled && overview.bootEnabled !== 'Disabled' && !!overview.bootTarget && overview.bootTarget !== 'None');

	async function load(): Promise<void> {
		error = null;
		try {
			const [o, m, j] = await Promise.all([
				bmcOverview(connectionId),
				bmcVirtualMedia(connectionId).catch(() => [] as BmcVirtualMedia[]),
				bmcJobs(connectionId).catch(() => [] as BmcJob[])
			]);
			overview = o;
			media = m;
			jobs = j;
			if (!bootTarget || !o.bootTargets.includes(bootTarget)) bootTarget = o.bootTargets.find((b) => b === 'Pxe') ?? o.bootTargets.find((b) => b !== 'None') ?? '';
		} catch (err) {
			error = String(err);
		}
	}

	async function run(label: string, job: () => Promise<unknown>, after?: () => void): Promise<void> {
		busy = true;
		try {
			const msg = await job();
			addToast(typeof msg === 'string' && msg ? msg : t('bmc.task_sent', { action: label, server }), 'success');
			after?.();
			await load();
		} catch (err) {
			addToast(String(err), 'error');
		} finally {
			busy = false;
		}
	}

	function power(reset: string): void {
		confirm = null;
		void run(word('bmc.reset_', reset), () => bmcPower(connectionId, reset), onchanged);
	}

	function restartBmc(): void {
		confirm = null;
		busy = true;
		bmcResetBmc(connectionId)
			.then(() => onbmcrestart())
			.catch((err) => addToast(String(err), 'error'))
			.finally(() => (busy = false));
	}

	$effect(() => {
		void refresh;
		void connectionId;
		void load();
	});
</script>

<div class="tasks">
	{#if error}<p class="note error">{error}</p>{/if}
	{#if !overview && !error}<p class="note">{t('bmc.loading')}</p>{/if}

	{#if overview}
		<section class="block">
			<h3>{t('bmc.power')} <span class="state">{word('bmc.power_state_', overview.powerState)}</span></h3>
			{#if powerActions.length}
				<div class="buttons">
					{#each powerActions as r (r)}
						<Button variant={HARSH.has(r) ? 'danger' : 'secondary'} size="sm" disabled={busy} onclick={() => (confirm = { kind: 'power', reset: r })}>{word('bmc.reset_', r)}</Button>
					{/each}
				</div>
			{:else}
				<p class="note">{t('bmc.power_none')}</p>
			{/if}
		</section>

		{#if bootTargets.length}
			<section class="block">
				<h3>{t('bmc.boot_title')}</h3>
				{#if bootSet}
					<p class="note">{t('bmc.boot_current', { target: word('bmc.boot_', overview.bootTarget) })}</p>
				{/if}
				<div class="buttons">
					<select class="sel" bind:value={bootTarget} disabled={busy} aria-label={t('bmc.boot_title')}>
						{#each bootTargets as b (b)}
							<option value={b}>{word('bmc.boot_', b)}</option>
						{/each}
					</select>
					<Button size="sm" disabled={busy || !bootTarget} onclick={() => void run(t('bmc.boot_title'), () => bmcBootOnce(connectionId, bootTarget), onchanged)}>{t('bmc.boot_apply')}</Button>
					{#if bootSet}
						<Button variant="ghost" size="sm" disabled={busy} onclick={() => void run(t('bmc.boot_cancel'), () => bmcBootOnce(connectionId, 'None'), onchanged)}>{t('bmc.boot_cancel')}</Button>
					{/if}
				</div>
				<p class="hint">{t('bmc.boot_hint')}</p>
			</section>
		{/if}

		{#if overview.identify !== null}
			<section class="block">
				<h3>{t('bmc.identify_title')}</h3>
				<Toggle
					checked={overview.identify}
					label={t('bmc.identify_label')}
					disabled={busy}
					onchange={(on) => void run(t('bmc.identify_title'), () => bmcIdentify(connectionId, on))}
				/>
				<p class="hint">{t('bmc.identify_hint')}</p>
			</section>
		{/if}

		<section class="block">
			<h3>{t('bmc.media_title')}</h3>
			{#if media && media.length === 0}
				<p class="note">{t('bmc.media_none')}</p>
			{/if}
			{#each media ?? [] as m (m.path)}
				<div class="media">
					<div class="media-head">
						<span class="media-name">{m.name}</span>
						<span class="dim">{m.mediaTypes.join(', ')}</span>
					</div>
					{#if m.inserted && m.image}
						<div class="buttons">
							<span class="mono image">{m.image}</span>
							<Button variant="secondary" size="sm" disabled={busy} onclick={() => void run(t('bmc.media_eject'), () => bmcMediaEject(connectionId, m.path))}>{t('bmc.media_eject')}</Button>
						</div>
					{:else}
						<form
							class="buttons"
							onsubmit={(e) => {
								e.preventDefault();
								const url = (images[m.path] ?? '').trim();
								if (url) void run(t('bmc.media_mount'), () => bmcMediaInsert(connectionId, m.path, url), () => (images[m.path] = ''));
							}}
						>
							<input class="url" type="url" placeholder="http://10.0.0.1/isos/install.iso" bind:value={images[m.path]} disabled={busy} aria-label={t('bmc.media_image')} />
							<Button type="submit" size="sm" disabled={busy || !(images[m.path] ?? '').trim()}>{t('bmc.media_mount')}</Button>
						</form>
					{/if}
				</div>
			{/each}
			{#if media?.length}<p class="hint">{t('bmc.media_hint')}</p>{/if}
		</section>

		{#if overview.canResetBmc}
			<section class="block">
				<h3>{t('bmc.reset_title')}</h3>
				<div class="buttons">
					<Button variant="secondary" size="sm" disabled={busy} onclick={() => (confirm = { kind: 'bmc' })}>{t('bmc.reset_button')}</Button>
				</div>
				<p class="hint">{t('bmc.reset_hint')}</p>
			</section>
		{/if}

		<section class="block">
			<h3>{t('bmc.jobs_title')} {#if jobs?.length}<span class="dim">{jobs.length}</span>{/if}</h3>
			{#if jobs && jobs.length === 0}
				<p class="note">{t('bmc.jobs_none')}</p>
			{:else if jobs}
				<div class="table-wrap">
					<table class="rf-table">
						<thead>
							<tr><th>{t('bmc.col_job')}</th><th>{t('bmc.col_state')}</th><th class="r">%</th><th>{t('bmc.col_time')}</th></tr>
						</thead>
						<tbody>
							{#each jobs as j, i (i)}
								<tr>
									<td>
										{j.name || j.id}
										{#if j.message}<span class="meta">{j.message}</span>{/if}
									</td>
									<td class="nowrap {j.state === 'Failed' || j.state === 'Exception' ? 'bad' : ''}">{j.state ?? '—'}</td>
									<td class="r">{j.percent ?? ''}</td>
									<td class="nowrap dim">{when(j.end ?? j.start)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</section>
	{/if}
</div>

<Modal open={confirm !== null} onclose={() => (confirm = null)} title={confirm?.kind === 'power' ? word('bmc.reset_', confirm.reset) : t('bmc.reset_title')} maxWidth="440px">
	{#if confirm?.kind === 'power'}
		<p class="note">{t('bmc.power_confirm', { action: word('bmc.reset_', confirm.reset), server })}</p>
		{#if HARSH.has(confirm.reset)}<p class="note warn">{t('bmc.power_harsh')}</p>{/if}
	{:else if confirm?.kind === 'bmc'}
		<p class="note">{t('bmc.reset_confirm', { server })}</p>
	{/if}
	{#snippet actions()}
		<Button variant="secondary" onclick={() => (confirm = null)}>{t('common.cancel')}</Button>
		{#if confirm?.kind === 'power'}
			{@const reset = confirm.reset}
			<Button variant={HARSH.has(reset) ? 'danger' : 'primary'} onclick={() => power(reset)}>{word('bmc.reset_', reset)}</Button>
		{:else}
			<Button onclick={restartBmc}>{t('bmc.reset_button')}</Button>
		{/if}
	{/snippet}
</Modal>

<style>
	.tasks {
		display: flex;
		flex-direction: column;
		gap: 18px;
		max-width: 820px;
	}
	.block h3 {
		margin: 0 0 8px;
		font-size: 13px;
		font-weight: 600;
		color: var(--color-text-secondary);
		display: flex;
		gap: 8px;
		align-items: baseline;
	}
	.state {
		font-weight: 400;
		color: var(--color-text-tertiary);
	}
	.buttons {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		align-items: center;
	}
	.sel,
	.url {
		padding: 5px 8px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.url {
		flex: 1;
		min-width: 200px;
	}
	.media {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: 8px 10px;
		border: 1px solid var(--color-border);
		border-radius: 8px;
		margin-bottom: 8px;
	}
	.media-head {
		display: flex;
		gap: 8px;
		font-size: 13px;
	}
	.media-name {
		font-weight: 500;
	}
	.image {
		flex: 1;
		overflow-wrap: anywhere;
	}
	.mono {
		font-family: var(--font-mono, monospace);
		font-size: 12px;
	}
	.dim {
		color: var(--color-text-tertiary);
		font-weight: 400;
	}
	.hint {
		margin: 6px 0 0;
		font-size: 12px;
		color: var(--color-text-tertiary);
	}
	.note {
		margin: 0;
		font-size: 13px;
		color: var(--color-text-secondary);
	}
	.note.warn {
		margin-top: 8px;
		color: var(--color-warning);
	}
	.note.error {
		color: var(--color-danger);
		white-space: pre-wrap;
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
	.r {
		text-align: right !important;
	}
	.nowrap {
		white-space: nowrap;
	}
	.bad {
		color: var(--color-danger);
	}
	.meta {
		display: block;
		font-size: 11px;
		color: var(--color-text-tertiary);
	}
</style>
