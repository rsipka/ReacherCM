<script lang="ts">
	/**
	 * tmux in the terminal bar: the tab's tmux session (and window) at a
	 * glance, and a dropdown of the server's tmux sessions to switch to, a
	 * new one to start, and the session's windows to show.
	 */
	import type { Tab } from '#lib/state/tabs.svelte.js';
	import { sshTmuxList, sshTmuxWindows, sshTmuxWindow, type TmuxListed, type TmuxWindow } from '#lib/ipc/ssh.js';
	import { isTmuxViewTab, openTmuxWindowTab, switchTmuxSession } from '#lib/terminal/tmux-tabs.js';
	import { validTmuxName } from '#lib/terminal/tmux.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { tick } from 'svelte';

	interface Props {
		tab: Tab;
		/** The session the tab runs in. */
		name: string;
	}

	let { tab, name }: Props = $props();

	/** A window with an arrow out of it: open in a new tab. */
	const TAB_ICON = 'M14 4h6v6M20 4l-8 8M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5';

	let usable = $derived(!!tab.connectionId && tab.link !== 'down');
	let windowName = $derived(tab.tmuxWindow?.name ?? '');
	let hint = $derived(windowName ? t('tmux.bar_hint_window', { name, window: windowName }) : t('tmux.bar_hint', { name }));

	let open = $state(false);
	let pos = $state({ left: 0, top: 0 });
	let sessions = $state<TmuxListed[] | null>(null);
	let windows = $state<TmuxWindow[] | null>(null);
	let fresh = $state('');
	let triggerEl = $state<HTMLButtonElement>();
	let popEl = $state<HTMLDivElement>();

	async function toggle(): Promise<void> {
		if (open) {
			open = false;
			return;
		}
		if (!triggerEl || !tab.connectionId) return;
		const r = triggerEl.getBoundingClientRect();
		const width = 300;
		pos = { left: Math.max(8, Math.min(r.left, window.innerWidth - width - 8)), top: r.bottom + 4 };
		sessions = null;
		windows = null;
		fresh = '';
		open = true;
		await tick();
		popEl?.focus();
		void load();
	}

	async function load(): Promise<void> {
		const id = tab.connectionId;
		const start = tab.sshConnectParams?.tmux;
		if (!id || !start) return;
		const [list, wins] = await Promise.all([
			sshTmuxList(id).catch((err) => {
				addToast(String(err), 'error');
				return [] as TmuxListed[];
			}),
			isTmuxViewTab(tab) ? sshTmuxWindows(id, start).catch(() => [] as TmuxWindow[]) : Promise.resolve(null)
		]);
		if (!open) return;
		sessions = list;
		windows = wins;
	}

	function pick(session: string): void {
		open = false;
		switchTmuxSession(tab, session);
	}

	async function showWindow(op: { kind: 'select'; window: string } | { kind: 'new' }): Promise<void> {
		open = false;
		const id = tab.connectionId;
		const start = tab.sshConnectParams?.tmux;
		if (!id || !start) return;
		try {
			await sshTmuxWindow(id, start, op, tab.sshConnectParams?.shell);
		} catch (err) {
			addToast(t('tmux.window_failed', { error: String(err) }), 'error');
		}
	}

	/** A window (or a new one) in a tab of its own, beside this one. */
	function windowTab(window: string): void {
		open = false;
		openTmuxWindowTab(tab, window).catch((err) => addToast(t('tmux.window_failed', { error: String(err) }), 'error'));
	}

	$effect(() => {
		if (!open) return;
		const away = (e: MouseEvent) => {
			const target = e.target as Node;
			if (popEl?.contains(target) || triggerEl?.contains(target)) return;
			open = false;
		};
		const close = () => (open = false);
		document.addEventListener('pointerdown', away, true);
		window.addEventListener('resize', close);
		return () => {
			document.removeEventListener('pointerdown', away, true);
			window.removeEventListener('resize', close);
		};
	});

	// The tab switched away or dropped: nothing here applies any more.
	$effect(() => {
		if (!usable) open = false;
	});
</script>

<button
	bind:this={triggerEl}
	class="chip"
	class:open
	type="button"
	onclick={() => void toggle()}
	disabled={!usable}
	title={hint}
	aria-label={hint}
	aria-haspopup="menu"
	aria-expanded={open}
>
	<span class="label">tmux</span>
	<span class="session">{name}</span>
	{#if windowName}<span class="window">· {windowName}</span>{/if}
	<svg class="chev" width="10" height="10" viewBox="0 0 12 12" fill="none" aria-hidden="true">
		<path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
	</svg>
</button>

{#if open}
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div
		bind:this={popEl}
		class="pop"
		role="menu"
		tabindex="-1"
		style:left="{pos.left}px"
		style:top="{pos.top}px"
		onkeydown={(e) => {
			if (e.key === 'Escape') {
				e.preventDefault();
				open = false;
				triggerEl?.focus();
			}
		}}
	>
		<div class="head">{t('tmux.bar_sessions')}</div>
		{#if sessions === null}
			<p class="note">{t('tmux.listing')}</p>
		{:else}
			{#each sessions as s (s.name)}
				<button type="button" class="row" class:current={s.name === name} role="menuitemradio" aria-checked={s.name === name} disabled={!validTmuxName(s.name)} onclick={() => pick(s.name)}>
					<span class="mark" aria-hidden="true">{s.name === name ? '✓' : ''}</span>
					<span class="row-name">{s.name}</span>
					<span class="row-meta">{t('tmux.windows', { count: String(s.windows) })}{#if s.attached > 0}{' · '}{t('tmux.attached', { count: String(s.attached) })}{/if}</span>
				</button>
			{/each}
		{/if}
		<form
			class="new"
			onsubmit={(e) => {
				e.preventDefault();
				if (fresh.trim()) pick(fresh.trim());
			}}
		>
			<input bind:value={fresh} placeholder={t('tmux.new_session')} aria-label={t('tmux.new_session')} spellcheck="false" autocomplete="off" />
			<button type="submit" class="go" disabled={!fresh.trim()}>{t('tmux.start_new')}</button>
		</form>
		{#if windows}
			<div class="sep"></div>
			<div class="head">{t('tmux.windows_title')}</div>
			{#each windows as w (w.id)}
				<div class="win">
					<!-- Ctrl/Cmd+click or a middle click: in a new tab, as a link would. -->
					<button
						type="button"
						class="row"
						class:current={w.active}
						role="menuitemradio"
						aria-checked={w.active}
						onclick={(e) => (e.ctrlKey || e.metaKey ? windowTab(w.id) : void showWindow({ kind: 'select', window: w.id }))}
						onauxclick={(e) => {
							if (e.button === 1) windowTab(w.id);
						}}
					>
						<span class="mark" aria-hidden="true">{w.active ? '✓' : ''}</span>
						<span class="row-name">{w.index}: {w.name}</span>
						<span class="row-meta">{w.command}{#if w.panes > 1}{' · '}{t('tmux.window_panes', { count: String(w.panes) })}{/if}</span>
					</button>
					<button type="button" class="tab-btn" role="menuitem" title={t('tmux.window_open_tab')} aria-label={`${t('tmux.window_open_tab')}: ${w.name}`} onclick={() => windowTab(w.id)}>
						<svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d={TAB_ICON} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" /></svg>
					</button>
				</div>
			{/each}
			<div class="win">
				<button type="button" class="row" role="menuitem" onclick={() => void showWindow({ kind: 'new' })}>
					<span class="mark" aria-hidden="true">+</span>
					<span class="row-name plain">{t('tmux.window_new')}</span>
				</button>
				<button type="button" class="tab-btn" role="menuitem" title={t('tmux.window_new_tab')} aria-label={t('tmux.window_new_tab')} onclick={() => windowTab('new')}>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d={TAB_ICON} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
			</div>
		{/if}
	</div>
{/if}

<style>
	.chip {
		flex: 0 1 auto;
		display: inline-flex;
		align-items: center;
		gap: 5px;
		min-width: 0;
		max-width: 260px;
		height: 22px;
		padding: 0 6px;
		border: 1px solid var(--color-border-strong);
		border-radius: 4px;
		background: none;
		color: var(--color-text-secondary);
		font: inherit;
		font-size: var(--text-2xs);
		line-height: 16px;
		cursor: pointer;
	}

	.chip:hover:not(:disabled),
	.chip.open {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.chip:disabled {
		opacity: 0.55;
		cursor: default;
	}

	.label {
		font-weight: 600;
		flex-shrink: 0;
	}

	.session,
	.window {
		font-family: var(--font-mono);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}

	.session {
		color: var(--color-text-primary);
	}

	.window {
		flex: 0 2 auto;
	}

	.chev {
		flex-shrink: 0;
	}

	.pop {
		position: fixed;
		z-index: 1000;
		width: 300px;
		max-height: min(70vh, 460px);
		overflow-y: auto;
		padding: 4px 0;
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: 8px;
		box-shadow: var(--shadow-elevated);
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		outline: none;
	}

	.head {
		padding: 6px 12px 4px;
		font-size: var(--text-2xs);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--color-text-tertiary);
	}

	.note {
		margin: 0;
		padding: 6px 12px;
		color: var(--color-text-secondary);
	}

	.row {
		display: grid;
		grid-template-columns: 14px 1fr auto;
		align-items: center;
		gap: 6px;
		width: 100%;
		padding: 6px 12px;
		border: none;
		background: none;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}

	.row:hover:not(:disabled) {
		background: var(--color-surface-hover);
	}

	.row:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.row.current .row-name {
		font-weight: 600;
	}

	.mark {
		color: var(--color-accent);
		font-weight: 700;
		text-align: center;
	}

	.row-name {
		font-family: var(--font-mono);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}

	.win {
		display: flex;
		align-items: center;
	}

	.win .row {
		flex: 1;
		min-width: 0;
		padding-right: 4px;
	}

	.tab-btn {
		flex-shrink: 0;
		display: inline-grid;
		place-items: center;
		width: 28px;
		height: 26px;
		margin-right: 8px;
		padding: 0;
		border: 1px solid transparent;
		border-radius: 6px;
		background: none;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.tab-btn:hover {
		background: var(--color-surface-hover);
		border-color: var(--color-border);
		color: var(--color-text-primary);
	}

	.row-name.plain {
		font-family: inherit;
	}

	.row-meta {
		font-size: var(--text-2xs);
		color: var(--color-text-secondary);
		white-space: nowrap;
	}

	.new {
		display: flex;
		gap: 6px;
		padding: 6px 12px 4px;
	}

	.new input {
		flex: 1;
		min-width: 0;
		height: 26px;
		padding: 0 8px;
		border: 1px solid var(--color-border-strong);
		border-radius: 6px;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font: inherit;
		font-family: var(--font-mono);
		font-size: 0.75rem;
	}

	.go {
		height: 26px;
		padding: 0 10px;
		border: none;
		border-radius: 6px;
		background: var(--color-accent-fill);
		color: #fff;
		font: inherit;
		font-size: 0.75rem;
		font-weight: 600;
		cursor: pointer;
	}

	.go:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.sep {
		height: 1px;
		margin: 6px 0 2px;
		background: var(--color-border);
	}

	/* A phone: only "tmux" and the arrow; the dropdown says the rest. */
	@media (max-width: 640px) {
		.session,
		.window {
			display: none;
		}
	}

	@media (pointer: coarse) {
		.chip {
			height: 30px;
		}

		.row {
			padding: 10px 12px;
		}

		.tab-btn {
			width: 36px;
			height: 36px;
		}
	}
</style>
