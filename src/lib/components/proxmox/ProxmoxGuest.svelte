<script lang="ts">
	/**
	 * One guest of a Proxmox tab, in a dialog: its facts and addresses, its
	 * power buttons, its consoles and its snapshots. Stopping, resetting and
	 * rolling back ask first; every action starts a Proxmox task, named in a
	 * toast and followed on the Tasks page.
	 */
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import {
		proxmoxGuest,
		proxmoxGuestAction,
		proxmoxSnapshotCreate,
		proxmoxSnapshotRollback,
		proxmoxSnapshotDelete,
		type ProxmoxGuest,
		type ProxmoxConsole
	} from '#lib/ipc/proxmox.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import type { ConsoleKind } from '#lib/sessions/proxmox-console.js';
	import { bytes, cpu, uptime, when, stateClass, stateLabel, proxmoxError } from './proxmox-format.js';

	interface Props {
		connectionId: string;
		node: string;
		kind: 'qemu' | 'lxc';
		vmid: number;
		onclose: () => void;
		onconsole: (kind: ConsoleKind, target: Omit<ProxmoxConsole, 'sessionId'>, name: string | null) => void;
	}

	let { connectionId, node, kind, vmid, onclose, onconsole }: Props = $props();

	/** Actions that cut the guest off like pulling its plug. */
	const HARSH = new Set(['stop', 'reset']);
	/** The guest is read again this often while the dialog is open. */
	const REFRESH_MS = 5_000;

	let guest = $state<ProxmoxGuest | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let confirm = $state<{ kind: 'action'; action: string } | { kind: 'rollback' | 'delete'; snapshot: string } | null>(null);
	let taking = $state<{ name: string; description: string; vmstate: boolean } | null>(null);

	let label = $derived(guest?.name ? `${guest.name} (${vmid})` : String(vmid));
	let running = $derived(guest?.status === 'running');
	let paused = $derived(running && (guest?.qmpstatus === 'paused' || guest?.qmpstatus === 'suspended' || guest?.qmpstatus === 'prelaunch'));
	let actions = $derived.by(() => {
		const g = guest;
		if (!g || g.template) return [] as string[];
		if (!running) return ['start'];
		if (paused) return ['resume', 'stop'];
		return kind === 'qemu' ? ['shutdown', 'reboot', 'suspend', 'stop', 'reset'] : ['shutdown', 'reboot', 'stop'];
	});

	async function load(): Promise<void> {
		try {
			guest = await proxmoxGuest(connectionId, node, kind, vmid);
			error = null;
		} catch (err) {
			error = proxmoxError(err);
		}
	}

	async function run(what: string, job: () => Promise<string>): Promise<void> {
		busy = true;
		try {
			await job();
			addToast(t('proxmox.task_started', { action: what, guest: label }), 'success');
			await load();
		} catch (err) {
			addToast(proxmoxError(err), 'error');
		} finally {
			busy = false;
		}
	}

	function act(action: string): void {
		if (HARSH.has(action)) {
			confirm = { kind: 'action', action };
			return;
		}
		void run(t(`proxmox.action_${action}`), () => proxmoxGuestAction(connectionId, node, kind, vmid, action));
	}

	function confirmed(): void {
		const c = confirm;
		confirm = null;
		if (!c) return;
		if (c.kind === 'action') void run(t(`proxmox.action_${c.action}`), () => proxmoxGuestAction(connectionId, node, kind, vmid, c.action));
		else if (c.kind === 'rollback') void run(t('proxmox.snap_rollback'), () => proxmoxSnapshotRollback(connectionId, node, kind, vmid, c.snapshot));
		else void run(t('proxmox.snap_delete'), () => proxmoxSnapshotDelete(connectionId, node, kind, vmid, c.snapshot));
	}

	const SNAPSHOT_NAME = /^[A-Za-z][A-Za-z0-9_-]{0,39}$/;

	function take(): void {
		const s = taking;
		if (!s || !SNAPSHOT_NAME.test(s.name)) return;
		taking = null;
		void run(t('proxmox.snap_take'), () => proxmoxSnapshotCreate(connectionId, node, kind, vmid, s.name, s.description, s.vmstate));
	}

	function openConsole(k: ConsoleKind, serial?: string): void {
		onconsole(k, { node, kind, vmid, serial: serial ?? null }, guest?.name ?? null);
	}

	$effect(() => {
		void connectionId;
		void load();
		const timer = setInterval(() => {
			if (!busy && document.visibilityState === 'visible') void load();
		}, REFRESH_MS);
		return () => clearInterval(timer);
	});
</script>

<Modal open={true} {onclose} title={`${kind === 'qemu' ? 'VM' : 'CT'} ${label}`} maxWidth="640px">
	<div class="guest">
		{#if error}<p class="note error">{error}</p>{/if}
		{#if !guest && !error}<p class="note">{t('proxmox.loading')}</p>{/if}

		{#if guest}
			<div class="state">
				<span class="dot {stateClass(guest.template ? null : paused ? 'paused' : guest.status)}"></span>
				<span>{guest.template ? t('proxmox.template') : stateLabel(paused ? guest.qmpstatus : guest.status)}</span>
				<span class="dim">{node}</span>
				{#if guest.lock}<span class="tag">{t('proxmox.locked', { lock: guest.lock })}</span>{/if}
				{#if guest.ha}<span class="tag">HA {guest.ha}</span>{/if}
				{#each guest.tags as tag (tag)}<span class="tag">{tag}</span>{/each}
			</div>

			{#if actions.length}
				<section>
					<h3>{t('proxmox.power')}</h3>
					<div class="buttons">
						{#each actions as a (a)}
							<Button variant={HARSH.has(a) ? 'danger' : a === 'start' || a === 'resume' ? 'primary' : 'secondary'} size="sm" disabled={busy} onclick={() => act(a)}>{t(`proxmox.action_${a}`)}</Button>
						{/each}
					</div>
				</section>
			{/if}

			{#if running && !guest.template}
				<section>
					<h3>{t('proxmox.consoles')}</h3>
					<div class="buttons">
						{#if kind === 'qemu'}
							{#if guest.spice}
								<Button variant="secondary" size="sm" onclick={() => openConsole('spice')}>SPICE</Button>
							{/if}
							<Button variant="secondary" size="sm" onclick={() => openConsole('vnc')}>{t('proxmox.console_display')}</Button>
							{#each guest.serials as s (s)}
								<Button variant="secondary" size="sm" onclick={() => openConsole('term', s)}>{t('proxmox.console_serial', { port: s })}</Button>
							{/each}
						{:else}
							<Button variant="secondary" size="sm" onclick={() => openConsole('term')}>{t('proxmox.console_text')}</Button>
						{/if}
					</div>
					{#if kind === 'qemu' && !guest.spice}
						<p class="note small">{t('proxmox.spice_hint')}</p>
					{/if}
				</section>
			{/if}

			<section>
				<h3>{t('proxmox.details')}</h3>
				<dl class="facts">
					{#if running}
						<dt>{t('proxmox.cpu')}</dt>
						<dd>{cpu(guest.cpu)} · {guest.cpus ?? '—'} vCPU</dd>
						<dt>{t('proxmox.memory')}</dt>
						<dd>{bytes(guest.mem)} / {bytes(guest.maxmem)}</dd>
						<dt>{t('proxmox.uptime')}</dt>
						<dd>{uptime(guest.uptime)}</dd>
					{:else}
						<dt>{t('proxmox.cpu')}</dt>
						<dd>{guest.cpus ?? '—'} vCPU</dd>
						<dt>{t('proxmox.memory')}</dt>
						<dd>{bytes(guest.maxmem)}</dd>
					{/if}
					<dt>{t('proxmox.disk')}</dt>
					<dd>{bytes(guest.maxdisk)}</dd>
					{#if guest.ostype}
						<dt>{t('proxmox.os_type')}</dt>
						<dd>{guest.ostype}</dd>
					{/if}
					<dt>{t('proxmox.onboot')}</dt>
					<dd>{guest.onboot ? t('proxmox.yes') : t('proxmox.no')}</dd>
					{#if kind === 'qemu'}
						<dt>{t('proxmox.agent')}</dt>
						<dd>{guest.agent ? t('proxmox.yes') : t('proxmox.no')}</dd>
					{/if}
					<dt>{t('proxmox.addresses')}</dt>
					<dd class="mono">{guest.addresses.length ? guest.addresses.join(', ') : kind === 'qemu' && !guest.agent ? t('proxmox.addresses_need_agent') : '—'}</dd>
				</dl>
				{#if guest.description}
					<p class="description">{guest.description}</p>
				{/if}
			</section>

			<section>
				<div class="head">
					<h3>{t('proxmox.snapshots')}</h3>
					{#if !guest.template}
						<Button variant="secondary" size="sm" disabled={busy || !!guest.lock} onclick={() => (taking = { name: '', description: '', vmstate: false })}>{t('proxmox.snap_take')}</Button>
					{/if}
				</div>
				{#if guest.snapshots.length === 0}
					<p class="note small">{t('proxmox.snap_none')}</p>
				{:else}
					<ul class="snaps">
						{#each guest.snapshots as s (s.name)}
							<li>
								<div class="snap-text">
									<span class="snap-name">{s.name}{#if s.vmstate} <span class="tag">{t('proxmox.snap_ram')}</span>{/if}</span>
									<span class="dim">{[when(s.time), s.description].filter((x) => x && x !== '—').join(' · ')}</span>
								</div>
								{#if !guest.template}
									<Button variant="ghost" size="sm" disabled={busy} onclick={() => (confirm = { kind: 'rollback', snapshot: s.name })}>{t('proxmox.snap_rollback')}</Button>
									<Button variant="ghost" size="sm" disabled={busy} onclick={() => (confirm = { kind: 'delete', snapshot: s.name })}>{t('proxmox.snap_delete')}</Button>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			</section>
		{/if}
	</div>
</Modal>

<Modal open={confirm !== null} onclose={() => (confirm = null)} title={t('proxmox.confirm_title')} maxWidth="420px" zIndex={110}>
	{#if confirm}
		<div class="dialog">
			<p class="note">
				{#if confirm.kind === 'action'}
					{t(`proxmox.confirm_${confirm.action}`, { guest: label })}
				{:else if confirm.kind === 'rollback'}
					{t('proxmox.confirm_rollback', { guest: label, snapshot: confirm.snapshot })}
				{:else}
					{t('proxmox.confirm_delete', { guest: label, snapshot: confirm.snapshot })}
				{/if}
			</p>
			<div class="actions">
				<Button variant="ghost" size="sm" onclick={() => (confirm = null)}>{t('common.cancel')}</Button>
				<Button variant="danger" size="sm" onclick={confirmed}>
					{confirm.kind === 'action' ? t(`proxmox.action_${confirm.action}`) : confirm.kind === 'rollback' ? t('proxmox.snap_rollback') : t('proxmox.snap_delete')}
				</Button>
			</div>
		</div>
	{/if}
</Modal>

<Modal open={taking !== null} onclose={() => (taking = null)} title={t('proxmox.snap_take')} maxWidth="420px" zIndex={110}>
	{#if taking}
		<form class="dialog" onsubmit={(e) => { e.preventDefault(); take(); }}>
			<label class="field">
				<span>{t('proxmox.snap_name')}</span>
				<input type="text" bind:value={taking.name} autocomplete="off" spellcheck="false" />
			</label>
			{#if taking.name && !SNAPSHOT_NAME.test(taking.name)}
				<p class="note small error">{t('proxmox.snap_name_hint')}</p>
			{/if}
			<label class="field">
				<span>{t('proxmox.snap_description')}</span>
				<input type="text" bind:value={taking.description} autocomplete="off" />
			</label>
			{#if kind === 'qemu' && running}
				<label class="check">
					<input type="checkbox" bind:checked={taking.vmstate} />
					<span>{t('proxmox.snap_with_ram')}</span>
				</label>
			{/if}
			<div class="actions">
				<Button variant="ghost" size="sm" type="button" onclick={() => (taking = null)}>{t('common.cancel')}</Button>
				<Button size="sm" type="submit" disabled={!SNAPSHOT_NAME.test(taking.name)}>{t('proxmox.snap_take')}</Button>
			</div>
		</form>
	{/if}
</Modal>

<style>
	.guest {
		display: flex;
		flex-direction: column;
		gap: 16px;
		color: var(--color-text-primary);
	}
	section {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	h3 {
		margin: 0;
		font-size: 12px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--color-text-tertiary);
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
	}
	.state {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		font-size: 13px;
	}
	.buttons {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 5px 18px;
		margin: 0;
		font-size: 13px;
	}
	.facts dt {
		color: var(--color-text-tertiary);
	}
	.facts dd {
		margin: 0;
		word-break: break-word;
	}
	.description {
		margin: 0;
		font-size: 12px;
		white-space: pre-wrap;
		color: var(--color-text-secondary);
		padding: 8px;
		border-radius: 6px;
		background: var(--color-surface-sunken, rgba(127, 127, 127, 0.08));
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
	}
	.snaps li + li {
		border-top: 1px solid var(--color-border);
	}
	.snap-text {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
	}
	.snap-name {
		font-weight: 500;
	}
	.tag {
		display: inline-block;
		font-size: 11px;
		padding: 0 6px;
		border-radius: 999px;
		background: var(--color-surface-sunken, rgba(127, 127, 127, 0.12));
		color: var(--color-text-secondary);
	}
	.mono {
		font-family: var(--font-mono, monospace);
		font-size: 12px;
	}
	.dim {
		color: var(--color-text-tertiary);
		font-size: 12px;
		word-break: break-word;
	}
	.dot {
		display: inline-block;
		width: 9px;
		height: 9px;
		border-radius: 50%;
		border: 1px solid var(--color-text-tertiary);
	}
	.dot.ok {
		background: var(--color-success, #2e9d5b);
		border-color: transparent;
	}
	.dot.warn {
		background: var(--color-warning, #d99a1e);
		border-color: transparent;
	}
	.dot.crit {
		background: var(--color-danger, #d64545);
		border-color: transparent;
	}
	.note {
		margin: 0;
		font-size: 13px;
		color: var(--color-text-secondary);
	}
	.note.small {
		font-size: 12px;
	}
	.note.error {
		color: var(--color-danger);
		white-space: pre-wrap;
	}
	.dialog {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.field input {
		padding: 7px 9px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.check {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13px;
		color: var(--color-text-primary);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}
	@media (max-width: 640px) {
		.facts {
			grid-template-columns: 1fr;
			gap: 2px;
		}
		.facts dd {
			margin-bottom: 6px;
		}
	}
</style>
