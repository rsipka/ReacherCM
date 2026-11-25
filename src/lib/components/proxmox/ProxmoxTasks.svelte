<script lang="ts">
	/**
	 * A Proxmox tab's Tasks page: the cluster's recent tasks, running ones
	 * first. A task opens its log, which reads on while the task runs; a
	 * running task can be stopped from there.
	 */
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { proxmoxTasks, proxmoxTaskLog, proxmoxTaskStop, type ProxmoxTask, type ProxmoxTaskLog } from '#lib/ipc/proxmox.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import { when, taskLabel, proxmoxError } from './proxmox-format.js';

	interface Props {
		connectionId: string;
		/** Bumped to read the list again. */
		refresh: number;
		/** The tab is in view: the list reads on. */
		active: boolean;
	}

	let { connectionId, refresh, active }: Props = $props();

	const LIST_MS = 10_000;
	const LOG_MS = 2_000;

	let tasks = $state<ProxmoxTask[] | null>(null);
	let error = $state<string | null>(null);
	let open = $state<ProxmoxTask | null>(null);
	let log = $state<ProxmoxTaskLog | null>(null);
	let lines = $state<string[]>([]);
	let logError = $state<string | null>(null);
	let stopping = $state(false);
	let logEl: HTMLPreElement | undefined = $state();

	async function load(): Promise<void> {
		try {
			tasks = await proxmoxTasks(connectionId);
			error = null;
		} catch (err) {
			error = proxmoxError(err);
		}
	}

	/** Read the open task's log on from where it was. */
	async function readLog(): Promise<void> {
		const task = open;
		if (!task) return;
		try {
			const next = await proxmoxTaskLog(connectionId, task.node, task.upid, log?.next ?? 0);
			if (open?.upid !== task.upid) return;
			const atEnd = !logEl || logEl.scrollTop + logEl.clientHeight >= logEl.scrollHeight - 8;
			lines = [...lines, ...next.lines];
			log = next;
			logError = null;
			if (atEnd) requestAnimationFrame(() => logEl && (logEl.scrollTop = logEl.scrollHeight));
		} catch (err) {
			logError = proxmoxError(err);
		}
	}

	function show(task: ProxmoxTask): void {
		open = task;
		log = null;
		lines = [];
		logError = null;
		void readLog();
	}

	function close(): void {
		open = null;
		log = null;
		lines = [];
	}

	async function stop(): Promise<void> {
		const task = open;
		if (!task || stopping) return;
		stopping = true;
		try {
			await proxmoxTaskStop(connectionId, task.node, task.upid);
			addToast(t('proxmox.task_stopping'), 'success');
			await readLog();
			await load();
		} catch (err) {
			addToast(proxmoxError(err), 'error');
		} finally {
			stopping = false;
		}
	}

	function outcome(task: ProxmoxTask): { text: string; cls: string } {
		if (task.running) return { text: t('proxmox.task_running'), cls: 'run' };
		if (!task.status || task.status === 'OK') return { text: 'OK', cls: 'ok' };
		if (task.status.startsWith('WARNINGS')) return { text: task.status, cls: 'warn' };
		return { text: task.status, cls: 'bad' };
	}

	$effect(() => {
		void refresh;
		void connectionId;
		void load();
	});

	$effect(() => {
		if (!active) return;
		const timer = setInterval(() => {
			if (document.visibilityState === 'visible') void load();
		}, LIST_MS);
		return () => clearInterval(timer);
	});

	// The open log reads on while its task runs.
	$effect(() => {
		if (!open || (log && !log.running)) return;
		const timer = setInterval(() => void readLog(), LOG_MS);
		return () => clearInterval(timer);
	});
</script>

<div class="tasks">
	{#if error}<p class="note error">{error}</p>{/if}
	{#if !tasks && !error}<p class="note">{t('proxmox.loading')}</p>{/if}
	{#if tasks && tasks.length === 0}<p class="note">{t('proxmox.no_tasks')}</p>{/if}
	{#if tasks && tasks.length}
		<div class="table-box">
			<table>
				<thead>
					<tr>
						<th>{t('proxmox.col_start')}</th>
						<th class="hide-narrow">{t('proxmox.col_end')}</th>
						<th class="hide-narrow">{t('proxmox.col_node')}</th>
						<th class="hide-narrow">{t('proxmox.col_user')}</th>
						<th>{t('proxmox.col_task')}</th>
						<th>{t('proxmox.col_status')}</th>
					</tr>
				</thead>
				<tbody>
					{#each tasks as task (task.upid)}
						{@const o = outcome(task)}
						<tr class="row" onclick={() => show(task)}>
							<td>{when(task.start)}</td>
							<td class="hide-narrow">{task.running ? '' : when(task.end)}</td>
							<td class="hide-narrow">{task.node}</td>
							<td class="hide-narrow dim">{task.user ?? ''}</td>
							<td class="wide">{taskLabel(task.kind)}{#if task.id} <span class="dim">{task.id}</span>{/if}</td>
							<td><span class="status {o.cls}">{o.text}</span></td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<Modal open={open !== null} onclose={close} title={open ? `${taskLabel(open.kind)}${open.id ? ' ' + open.id : ''}` : ''} maxWidth="760px">
	{#if open}
		<div class="log-box">
			<div class="log-head">
				<span class="dim">{[open.node, open.user, when(open.start)].filter(Boolean).join(' · ')}</span>
				<span class="spacer"></span>
				{#if log?.running ?? open.running}
					<span class="status run">{t('proxmox.task_running')}</span>
					<Button variant="danger" size="sm" disabled={stopping} onclick={() => void stop()}>{t('proxmox.task_stop')}</Button>
				{:else if log}
					<span class="status {!log.exitstatus || log.exitstatus === 'OK' ? 'ok' : log.exitstatus.startsWith('WARNINGS') ? 'warn' : 'bad'}">{log.exitstatus ?? 'OK'}</span>
				{/if}
			</div>
			{#if logError}<p class="note error">{logError}</p>{/if}
			<pre class="log" bind:this={logEl}>{lines.join('\n')}</pre>
		</div>
	{/if}
</Modal>

<style>
	.tasks {
		display: flex;
		flex-direction: column;
		gap: 10px;
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
	.dim {
		color: var(--color-text-tertiary);
		font-size: 12px;
	}
	.status {
		font-size: 12px;
		font-weight: 500;
	}
	.status.ok {
		color: var(--color-success, #2e9d5b);
	}
	.status.run {
		color: var(--color-accent);
	}
	.status.warn {
		color: var(--color-warning, #d99a1e);
	}
	.status.bad {
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
	.log-box {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.log-head {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.spacer {
		flex: 1;
	}
	.log {
		margin: 0;
		height: min(55vh, 480px);
		overflow: auto;
		padding: 10px;
		border-radius: 6px;
		background: var(--color-surface-sunken, rgba(127, 127, 127, 0.08));
		color: var(--color-text-primary);
		font-family: var(--font-mono, monospace);
		font-size: 12px;
		white-space: pre-wrap;
		word-break: break-word;
	}
	@media (max-width: 640px) {
		.hide-narrow {
			display: none;
		}
	}
</style>
