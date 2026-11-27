<script lang="ts">
	import { writeText } from '@tauri-apps/plugin-clipboard-manager';
	import { t, getLocale } from '#lib/state/i18n.svelte.js';
	import {
		getHistory,
		getUnreadCount,
		markHistorySeen,
		removeHistoryEntry,
		clearHistory
	} from '#lib/state/notification-history.svelte.js';
	import type { HistoryType } from '#lib/state/notify-rules.js';

	/**
	 * The bell in the top bar: the last day's notifications, newest first,
	 * with a count of the ones that came in since it was last opened.
	 */

	let open = $state(false);
	let copiedId = $state<string | null>(null);
	let buttonEl = $state<HTMLButtonElement | null>(null);
	let panelEl = $state<HTMLDivElement | null>(null);

	let entries = $derived(getHistory());
	let unread = $derived(getUnreadCount());
	let title = $derived(unread > 0 ? t('notifications.history_unread', { count: unread }) : t('notifications.history'));

	const dotColors: Record<HistoryType, string> = {
		info: 'var(--color-accent)',
		success: 'var(--color-success)',
		warning: 'var(--color-warning)',
		error: 'var(--color-danger)'
	};

	/** Under the bell, kept inside the window; a phone's CSS overrides it. */
	let anchor = $state('');

	function toggle(): void {
		open = !open;
		if (!open) return;
		markHistorySeen();
		const r = buttonEl?.getBoundingClientRect();
		if (r) {
			const left = Math.max(8, Math.min(r.left, window.innerWidth - 388));
			anchor = `top: ${Math.round(r.bottom + 4)}px; left: ${Math.round(left)}px;`;
		}
	}

	function close(): void {
		if (!open) return;
		open = false;
		markHistorySeen();
	}

	/** Today: the time; earlier: the day and the time. */
	function when(at: number): string {
		const d = new Date(at);
		const midnight = new Date();
		midnight.setHours(0, 0, 0, 0);
		const time = { hour: '2-digit', minute: '2-digit' } as const;
		try {
			return at >= midnight.getTime()
				? d.toLocaleTimeString(getLocale(), time)
				: d.toLocaleString(getLocale(), { weekday: 'short', ...time });
		} catch {
			return d.toLocaleTimeString();
		}
	}

	async function copy(id: string, message: string): Promise<void> {
		try {
			await writeText(message);
		} catch {
			try {
				await navigator.clipboard.writeText(message);
			} catch {
				return;
			}
		}
		copiedId = id;
		setTimeout(() => {
			if (copiedId === id) copiedId = null;
		}, 1200);
	}

	function onWindowPointer(e: PointerEvent): void {
		if (!open) return;
		const target = e.target as Node | null;
		if (target && (panelEl?.contains(target) || buttonEl?.contains(target))) return;
		close();
	}

	function onWindowKey(e: KeyboardEvent): void {
		if (open && e.key === 'Escape') {
			e.stopPropagation();
			close();
			buttonEl?.focus();
		}
	}
</script>

<svelte:window onpointerdown={onWindowPointer} onkeydown={onWindowKey} />

<button
	bind:this={buttonEl}
	class="bell-btn"
	class:active={open}
	onclick={toggle}
	aria-label={title}
	aria-expanded={open}
	aria-haspopup="dialog"
	{title}
>
	<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
		<path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9" />
		<path d="M13.73 21a2 2 0 0 1-3.46 0" />
	</svg>
	{#if unread > 0}
		<span class="badge">{unread > 99 ? '99+' : unread}</span>
	{/if}
</button>

{#if open}
	<div bind:this={panelEl} class="history-panel" style={anchor} role="dialog" aria-label={t('notifications.history')}>
		<div class="panel-head">
			<span class="panel-title">{t('notifications.history')}</span>
			{#if entries.length > 0}
				<button class="text-btn" onclick={clearHistory}>{t('notifications.clear_all')}</button>
			{/if}
		</div>
		{#if entries.length === 0}
			<p class="empty">{t('notifications.history_empty')}</p>
		{:else}
			<ul class="entries">
				{#each entries as entry (entry.id)}
					<li class="entry">
						<span class="dot" style="background-color: {dotColors[entry.type]}"></span>
						<div class="entry-body">
							<span class="entry-message">{entry.message}</span>
							<span class="entry-time">{when(entry.at)}</span>
						</div>
						<div class="entry-actions">
							<button
								class="icon-btn"
								onclick={() => void copy(entry.id, entry.message)}
								aria-label={t('notifications.copy')}
								title={copiedId === entry.id ? t('notifications.copied') : t('notifications.copy')}
							>
								{#if copiedId === entry.id}
									<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6L9 17l-5-5" /></svg>
								{:else}
									<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="9" width="13" height="13" rx="2" /><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" /></svg>
								{/if}
							</button>
							<button class="icon-btn" onclick={() => removeHistoryEntry(entry.id)} aria-label={t('common.dismiss')} title={t('common.dismiss')}>
								<svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true"><path d="M1 1L9 9M9 1L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
							</button>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
		<p class="panel-foot">{t('notifications.history_kept')}</p>
	</div>
{/if}

<style>
	.bell-btn {
		position: relative;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: var(--radius-btn);
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		-webkit-app-region: no-drag;
		transition:
			background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default);
	}

	.bell-btn:hover,
	.bell-btn.active {
		background-color: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.badge {
		position: absolute;
		top: 1px;
		right: 0;
		min-width: 14px;
		height: 14px;
		padding: 0 3px;
		border-radius: 7px;
		background-color: var(--color-accent);
		color: #fff;
		font-size: 0.5625rem;
		font-weight: 700;
		line-height: 14px;
		text-align: center;
		pointer-events: none;
	}

	.history-panel {
		position: fixed;
		top: calc(var(--titlebar-h, 38px) + var(--safe-top) + 4px);
		left: calc(8px + var(--safe-left));
		z-index: 210;
		display: flex;
		flex-direction: column;
		width: min(380px, calc(100vw - var(--safe-left) - var(--safe-right) - 16px));
		max-height: min(480px, calc(100vh - var(--titlebar-h, 38px) - var(--safe-top) - var(--safe-bottom) - 24px));
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card);
		box-shadow: var(--shadow-elevated);
		-webkit-app-region: no-drag;
		user-select: text;
	}

	/* A phone: the width of the screen, whatever the bell's place. */
	@media (max-width: 640px) {
		.history-panel {
			left: calc(8px + var(--safe-left)) !important;
			right: calc(8px + var(--safe-right));
			width: auto;
		}
	}

	.panel-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px 12px 8px;
		border-bottom: 1px solid var(--color-border);
	}

	.panel-title {
		font-size: 0.8125rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.text-btn {
		padding: 2px 6px;
		border: none;
		border-radius: var(--radius-btn);
		background: transparent;
		color: var(--color-accent);
		font-family: var(--font-sans);
		font-size: 0.75rem;
		cursor: pointer;
	}

	.text-btn:hover {
		background-color: var(--color-surface-hover);
	}

	.empty {
		margin: 0;
		padding: 24px 12px;
		text-align: center;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.entries {
		flex: 1;
		min-height: 0;
		margin: 0;
		padding: 4px 0;
		list-style: none;
		overflow-y: auto;
	}

	.entry {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		padding: 6px 8px 6px 12px;
	}

	.entry:hover {
		background-color: var(--color-surface-hover);
	}

	.dot {
		flex-shrink: 0;
		width: 7px;
		height: 7px;
		margin-top: 6px;
		border-radius: 50%;
	}

	.entry-body {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.entry-message {
		font-size: 0.8125rem;
		line-height: 1.35;
		color: var(--color-text-primary);
		overflow-wrap: anywhere;
		white-space: pre-wrap;
	}

	.entry-time {
		font-size: 0.6875rem;
		color: var(--color-text-tertiary, var(--color-text-secondary));
	}

	.entry-actions {
		display: flex;
		gap: 2px;
		flex-shrink: 0;
		opacity: 0;
		transition: opacity 120ms ease;
	}

	.entry:hover .entry-actions,
	.entry:focus-within .entry-actions {
		opacity: 1;
	}

	/* No hover on a touch screen: the buttons stay. */
	@media (pointer: coarse) {
		.entry-actions {
			opacity: 1;
		}
	}

	.icon-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 22px;
		height: 22px;
		border: none;
		border-radius: 50%;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.icon-btn:hover {
		background-color: var(--color-surface-active);
		color: var(--color-text-primary);
	}

	.panel-foot {
		margin: 0;
		padding: 6px 12px 8px;
		border-top: 1px solid var(--color-border);
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}
</style>
