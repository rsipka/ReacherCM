<script lang="ts">
	/**
	 * ZMODEM in a terminal (SSH, Telnet, serial). When the remote runs `rz`
	 * this asks for files to send; when it runs `sz`, for a folder to save
	 * into. While the transfer runs it shows over the bottom of the terminal
	 * with its progress and a Cancel button. The transfer itself runs in the
	 * backend, on the session's own connection (src-tauri/src/zmodem).
	 */
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { open } from '@tauri-apps/plugin-dialog';
	import { downloadDir } from '@tauri-apps/api/path';
	import { sshZmodemExec } from '#lib/ipc/zmodem.js';
	import type { TmuxStart } from '#lib/ipc/ssh.js';
	import { zmodemCancel, zmodemDownload, zmodemUpload, type ZmodemDirection, type ZmodemEvent } from '#lib/ipc/zmodem.js';
	import { formatBytes, percent, Speed } from '#lib/terminal/zmodem.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { isMobile } from '#lib/platform.js';
	import { ZMODEM_SEND_EVENT, type ZmodemRequest } from '#lib/state/zmodem.js';

	interface Props {
		/** The session's connection; events come as `zmodem-{id}`. */
		connectionId?: string;
		/** The tab, for a Send files request from its bar or menu. */
		tabId: string;
		/** Types into the session (`rz` for Send files); false when it is down. */
		send: (data: number[]) => boolean;
		/** An SSH tab running in tmux: its terminal cannot carry ZMODEM, so
		 *  the buttons run rz or sz on a channel of their own, in the
		 *  folder the tmux pane is in. */
		tmux?: TmuxStart | null;
	}

	let { connectionId, tabId, send, tmux }: Props = $props();

	/** Where the transfer the bar shows came from: the terminal's
	 *  connection, or the side channel (`{id}-zx`). */
	let activeId: string | null = null;

	/** Run rz or the folder's sz on a side channel (tmux tabs). */
	function viaChannel(kind: 'receive' | 'send-folder'): void {
		if (!connectionId || !tmux) return;
		sshZmodemExec(connectionId, kind, tmux.name, tmux.view ?? null).catch((e) => {
			clearPending();
			addToast(t('zmodem.failed', { error: String(e) }), 'error', 8000);
		});
	}

	/** Asked for from the bar or the tab menu: what goes when the rz or sz
	 *  typed for it answers. A folder download may run sz several times
	 *  (find hands it the files in batches); each is accepted while this
	 *  lasts. */
	type Pending = { kind: 'upload'; paths: string[] } | { kind: 'download'; folder: string };
	let pending: (Pending & { timer: ReturnType<typeof setTimeout> }) | null = null;
	/** How long rz or sz has to answer. */
	const ANSWER_WAIT_MS = 20_000;
	/** Between two sz runs of one folder download. */
	const NEXT_BATCH_MS = 10_000;

	function clearPending(): void {
		if (pending) clearTimeout(pending.timer);
		pending = null;
	}

	function arm(p: Pending, ms: number, warn: boolean): void {
		clearPending();
		pending = {
			...p,
			timer: setTimeout(() => {
				pending = null;
				if (warn) addToast(t(p.kind === 'upload' ? 'zmodem.no_rz' : 'zmodem.no_sz'), 'warning', 8000);
			}, ms)
		};
	}

	function type(text: string): boolean {
		if (send(Array.from(new TextEncoder().encode(text)))) return true;
		clearPending();
		addToast(t('zmodem.not_connected'), 'warning', 4000);
		return false;
	}

	/** Send files or a folder: pick, then type rz; they go when rz answers. */
	async function sendFiles(folder: boolean): Promise<void> {
		if (!connectionId || running || choosing) return;
		if (isMobile()) {
			addToast(t('zmodem.desktop_only'), 'info', 5000);
			return;
		}
		const picked = await open({ multiple: true, directory: folder, title: t(folder ? 'zmodem.send_folder' : 'zmodem.send_files') }).catch(() => null);
		const paths = picked === null ? [] : Array.isArray(picked) ? picked : [picked];
		if (paths.length === 0) return;
		arm({ kind: 'upload', paths }, ANSWER_WAIT_MS, true);
		if (tmux) viaChannel('receive');
		else type('rz\r');
	}

	/** Download the remote's current folder: pick where, then have find run
	 *  sz on every file under it, with paths, from its parent. In a subshell,
	 *  so the shell stays where it was. */
	async function downloadFolder(): Promise<void> {
		if (!connectionId || running || choosing) return;
		if (isMobile()) {
			addToast(t('zmodem.desktop_only'), 'info', 5000);
			return;
		}
		const defaultPath = lastFolder() ?? (await downloadDir().catch(() => undefined));
		const picked = await open({ directory: true, multiple: false, defaultPath, title: t('zmodem.pick_folder') }).catch(() => null);
		const folder = Array.isArray(picked) ? picked[0] : picked;
		if (!folder) return;
		rememberFolder(folder);
		arm({ kind: 'download', folder }, ANSWER_WAIT_MS, true);
		if (tmux) viaChannel('send-folder');
		else type(`(d="\${PWD##*/}"; cd .. && find "$d" -type f -exec sz -f {} +)\r`);
	}

	$effect(() => {
		const onRequest = (e: Event) => {
			const { tabId: id, kind } = (e as CustomEvent<ZmodemRequest>).detail;
			if (id !== tabId) return;
			if (kind === 'download-folder') void downloadFolder();
			else void sendFiles(kind === 'folder');
		};
		window.addEventListener(ZMODEM_SEND_EVENT, onRequest);
		return () => {
			window.removeEventListener(ZMODEM_SEND_EVENT, onRequest);
			clearPending();
		};
	});

	interface Running {
		direction: ZmodemDirection;
		file: string;
		index: number;
		count: number | null;
		done: number;
		total: number | null;
		rate: number | null;
	}

	/** Choosing files or a folder, or the transfer going. */
	let choosing = $state<ZmodemDirection | null>(null);
	let running = $state<Running | null>(null);
	let cancelling = $state(false);
	const speed = new Speed();

	const LAST_FOLDER = 'reach.zmodem.folder';

	function lastFolder(): string | null {
		try {
			return localStorage.getItem(LAST_FOLDER);
		} catch {
			return null;
		}
	}

	function rememberFolder(folder: string): void {
		try {
			localStorage.setItem(LAST_FOLDER, folder);
		} catch {
			// Only a convenience.
		}
	}

	async function answer(id: string, direction: ZmodemDirection): Promise<void> {
		if (isMobile()) {
			addToast(t('zmodem.desktop_only'), 'info', 5000);
			await zmodemCancel(id).catch(() => {});
			return;
		}
		const ready = pending?.kind === 'upload' && direction === 'upload' ? pending.paths : undefined;
		const into = pending?.kind === 'download' && direction === 'download' ? pending.folder : undefined;
		// A folder download keeps its folder for sz's next batch.
		if (!into) clearPending();
		choosing = direction;
		try {
			if (ready) {
				running = { direction, file: '', index: 0, count: null, done: 0, total: null, rate: null };
				await zmodemUpload(id, ready);
			} else if (into) {
				running = { direction, file: '', index: 0, count: null, done: 0, total: null, rate: null };
				await zmodemDownload(id, into);
			} else if (direction === 'upload') {
				const picked = await open({ multiple: true, directory: false, title: t('zmodem.pick_files') });
				const paths = picked === null ? [] : Array.isArray(picked) ? picked : [picked];
				if (paths.length === 0) {
					await zmodemCancel(id);
					return;
				}
				running = { direction, file: '', index: 0, count: paths.length, done: 0, total: null, rate: null };
				await zmodemUpload(id, paths);
			} else {
				const defaultPath = lastFolder() ?? (await downloadDir().catch(() => undefined));
				const picked = await open({ directory: true, multiple: false, defaultPath, title: t('zmodem.pick_folder') });
				const folder = Array.isArray(picked) ? picked[0] : picked;
				if (!folder) {
					await zmodemCancel(id);
					return;
				}
				rememberFolder(folder);
				running = { direction, file: '', index: 0, count: null, done: 0, total: null, rate: null };
				await zmodemDownload(id, folder);
			}
		} catch (e) {
			running = null;
			addToast(t('zmodem.failed', { error: String(e) }), 'error', 6000);
			await zmodemCancel(id).catch(() => {});
		} finally {
			choosing = null;
		}
	}

	function onEvent(id: string, e: ZmodemEvent): void {
		switch (e.kind) {
			case 'offer':
				cancelling = false;
				activeId = id;
				void answer(id, e.direction);
				break;
			case 'progress':
				running = {
					direction: e.direction,
					file: e.file,
					index: e.index,
					count: e.count,
					done: e.done,
					total: e.total,
					rate: speed.add(`${e.index}:${e.file}`, e.done, performance.now())
				};
				break;
			case 'finished': {
				running = null;
				if (pending?.kind === 'download') arm(pending, NEXT_BATCH_MS, false);
				const files = e.files.length > 4 ? `${e.files.slice(0, 3).join(', ')}, … (${e.files.length})` : e.files.join(', ');
				if (e.direction === 'download') {
					addToast(t('zmodem.received', { files, folder: e.folder ?? '' }), 'success', 6000);
				} else {
					addToast(t('zmodem.sent', { files }), 'success', 4000);
				}
				break;
			}
			case 'cancelled':
				running = null;
				addToast(t('zmodem.cancelled'), 'info', 3000);
				break;
			case 'unsupported':
				clearPending();
				addToast(t('zmodem.multiplexer'), 'warning', 12000);
				break;
			case 'failed':
				clearPending();
				running = null;
				addToast(t('zmodem.failed', { error: e.error }), 'error', 8000);
				break;
		}
	}

	$effect(() => {
		const id = connectionId;
		if (!id) return;
		let unlisten: UnlistenFn | undefined;
		let gone = false;
		const unlisteners: UnlistenFn[] = [];
		// The terminal's own connection, and the side channel tmux tabs use.
		for (const source of [id, `${id}-zx`]) {
			void listen<ZmodemEvent>(`zmodem-${source}`, (event) => onEvent(source, event.payload)).then((u) => {
				if (gone) u();
				else unlisteners.push(u);
			});
		}
		return () => {
			gone = true;
			for (const u of unlisteners) u();
			running = null;
			choosing = null;
		};
	});

	function cancel(): void {
		const id = activeId ?? connectionId;
		if (!id) return;
		cancelling = true;
		zmodemCancel(id).catch(() => {});
	}

	let shown = $derived(running ?? (choosing ? { direction: choosing } : null));
	let pct = $derived(running ? percent(running.done, running.total) : null);
</script>

{#if shown}
	<div class="zmodem-bar" role="status" aria-live="polite">
		<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
			{#if shown.direction === 'upload'}
				<path d="M12 19V5" /><path d="m5 12 7-7 7 7" />
			{:else}
				<path d="M12 5v14" /><path d="m19 12-7 7-7-7" />
			{/if}
		</svg>
		<div class="body">
			{#if running && running.file}
				<div class="line">
					<span class="name" title={running.file}>
						{t(running.direction === 'upload' ? 'zmodem.sending' : 'zmodem.receiving', { file: running.file })}
					</span>
					{#if running.count && running.count > 1}
						<span class="muted">{t('zmodem.file_of', { index: String(running.index), count: String(running.count) })}</span>
					{/if}
				</div>
				<div class="meter" class:unknown={pct === null}>
					<div class="fill" style:width={pct === null ? '100%' : `${pct}%`}></div>
				</div>
				<div class="line muted">
					<span>
						{formatBytes(running.done)}{running.total !== null ? ` / ${formatBytes(running.total)}` : ''}{pct !== null ? ` · ${pct} %` : ''}
					</span>
					{#if running.rate !== null}
						<span>{formatBytes(running.rate)}/s</span>
					{/if}
				</div>
			{:else if running}
				<span class="name">{t('zmodem.starting')}</span>
			{:else}
				<span class="name">{t(shown.direction === 'upload' ? 'zmodem.pick_files' : 'zmodem.pick_folder')}</span>
			{/if}
		</div>
		{#if running}
			<button class="bar-btn" onclick={cancel} disabled={cancelling} title={t('zmodem.cancel_hint')}>{t('common.cancel')}</button>
		{/if}
	</div>
{/if}

<style>
	.zmodem-bar {
		position: absolute;
		left: 12px;
		right: 12px;
		bottom: 12px;
		z-index: 5;
		display: flex;
		align-items: center;
		gap: 10px;
		max-width: 560px;
		margin: 0 auto;
		padding: 8px 12px;
		border-radius: 10px;
		font-size: var(--text-sm, 0.8125rem);
		color: var(--color-text-primary);
		background: color-mix(in srgb, var(--color-bg-secondary, var(--color-bg-primary)) 94%, transparent);
		border: 1px solid var(--color-border, rgba(127, 127, 127, 0.3));
		box-shadow: 0 6px 24px rgba(0, 0, 0, 0.25);
		backdrop-filter: blur(8px);
	}

	.zmodem-bar svg {
		flex-shrink: 0;
		color: var(--color-accent);
	}

	.body {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.line {
		display: flex;
		justify-content: space-between;
		gap: 8px;
		min-width: 0;
	}

	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-weight: 600;
	}

	.muted {
		color: var(--color-text-secondary);
		font-size: var(--text-xs, 0.75rem);
		white-space: nowrap;
		font-variant-numeric: tabular-nums;
	}

	.meter {
		height: 4px;
		border-radius: 2px;
		background: color-mix(in srgb, var(--color-text-secondary) 20%, transparent);
		overflow: hidden;
	}

	.fill {
		height: 100%;
		background: var(--color-accent);
		transition: width 0.15s linear;
	}

	.meter.unknown .fill {
		opacity: 0.5;
	}

	.bar-btn {
		flex-shrink: 0;
		padding: 3px 12px;
		border-radius: 6px;
		border: 1px solid var(--color-border, currentColor);
		background: transparent;
		color: inherit;
		font: inherit;
		cursor: pointer;
	}

	.bar-btn:hover:not(:disabled) {
		background: color-mix(in srgb, var(--color-error, #e5484d) 14%, transparent);
	}

	.bar-btn:disabled {
		opacity: 0.6;
		cursor: default;
	}

	.bar-btn:focus-visible {
		outline: 2px solid var(--color-accent);
		outline-offset: 1px;
	}
</style>
