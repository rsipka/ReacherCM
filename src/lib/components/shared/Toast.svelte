<script lang="ts">
	import { getToasts, dismissToast, type ToastType } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSettings } from '#lib/state/settings.svelte.js';

	let toasts = $derived(getToasts());
	/** Settings > General > Notification position: 'top-center', 'bottom-right'… */
	let position = $derived(getSettings().toastPosition);
	let bottom = $derived(position.startsWith('bottom'));

	const dotColors: Record<ToastType, string> = {
		info: 'var(--color-accent)',
		success: 'var(--color-success)',
		warning: 'var(--color-warning)',
		error: 'var(--color-danger)'
	};
</script>

{#if toasts.length > 0}
	<div
		class="toast-container"
		class:bottom
		class:left={position.endsWith('left')}
		class:right={position.endsWith('right')}
	>
		{#each toasts as toast (toast.id)}
			<div
				class="toast"
				class:dismissing={toast.dismissing}
				class:detailed={toast.type === 'error' || toast.type === 'warning'}
				role="alert"
			>
				<span class="toast-dot" style="background-color: {dotColors[toast.type]}"></span>
				<span class="toast-message" title={toast.message}>{toast.message}</span>
				{#if toast.action}
					<button class="toast-action" type="button" onclick={() => { toast.action?.run(); dismissToast(toast.id); }}>{toast.action.label}</button>
				{/if}
				<button class="toast-close" onclick={() => dismissToast(toast.id)} aria-label={t('common.dismiss')}>
					<svg width="10" height="10" viewBox="0 0 10 10" fill="none">
						<path d="M1 1L9 9M9 1L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
					</svg>
				</button>
			</div>
		{/each}
	</div>
{/if}

<style>
	.toast-action {
		flex-shrink: 0;
		padding: 2px 8px;
		font-family: var(--font-sans);
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--color-accent);
		background: transparent;
		border: 1px solid var(--color-accent);
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	.toast-action:hover {
		background-color: var(--color-surface-hover);
	}

	.toast-container {
		position: fixed;
		top: calc(80px + var(--safe-top));
		/* Full width, the toasts centred in it: at left: 50% the box could only
		   be as wide as the half of the window to its right, and on a phone a
		   warning wrapped into a column half the screen wide. */
		left: 0;
		right: 0;
		z-index: 200;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		pointer-events: none;
	}

	/* Over the status bar, which is 0 tall on a phone. */
	.toast-container.bottom {
		top: auto;
		bottom: calc(var(--statusbar-h, 24px) + 12px + var(--safe-bottom));
	}

	/* A corner: the toasts line up on that side, clear of the edge. */
	.toast-container.left {
		align-items: flex-start;
		left: calc(12px + var(--safe-left));
	}

	.toast-container.right {
		align-items: flex-end;
		right: calc(12px + var(--safe-right));
	}

	.toast {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 16px;
		max-width: min(360px, calc(100vw - var(--safe-left) - var(--safe-right) - 24px));
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: 20px;
		/* The pill is sized for "Copied" and clipped a one-line message with an
		   ellipsis, which made every backend error unreadable — a failure is
		   precisely the toast worth reading in full. `.detailed` widens it and
		   lets the text wrap; the radius drops because a 20px pill looks broken
		   once it is more than one line tall. */
		box-shadow: var(--shadow-elevated);
		animation: slideDown 250ms var(--ease-default) forwards;
	}

	.toast.detailed {
		max-width: min(520px, calc(100vw - var(--safe-left) - var(--safe-right) - 24px));
		align-items: flex-start;
		border-radius: var(--radius-card);
	}

	.toast.detailed .toast-dot {
		/* Line it up with the first line of text rather than the block centre. */
		margin-top: 6px;
	}

	.toast.detailed .toast-message {
		white-space: normal;
		overflow-wrap: anywhere;
		/* Four lines is enough for a server message without letting a runaway
		   payload cover the app; the full text stays on the title attribute. */
		display: -webkit-box;
		-webkit-line-clamp: 4;
		line-clamp: 4;
		-webkit-box-orient: vertical;
	}

	/* A phone has no pointer to hover for the rest, and a narrow toast fits
	   less in four lines: twice that, before it is cut. */
	@media (max-width: 480px) {
		.toast.detailed .toast-message {
			-webkit-line-clamp: 8;
			line-clamp: 8;
		}
	}

	.toast.dismissing {
		animation: slideUp 250ms var(--ease-default) forwards;
	}

	/* At the bottom they come up from the edge and go back down to it. */
	.bottom .toast {
		animation-name: riseIn;
	}

	.bottom .toast.dismissing {
		animation-name: sinkOut;
	}

	.toast-dot {
		flex-shrink: 0;
		width: 7px;
		height: 7px;
		border-radius: 50%;
	}

	.toast-message {
		flex: 1;
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		line-height: 1.3;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.toast-close {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 20px;
		height: 20px;
		border: none;
		border-radius: 50%;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition: background-color 150ms ease;
	}

	.toast-close:hover {
		background-color: var(--color-surface-active);
		color: var(--color-text-primary);
	}

	@keyframes slideDown {
		from {
			opacity: 0;
			transform: translateY(-20px);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	@keyframes slideUp {
		from {
			opacity: 1;
			transform: translateY(0);
		}
		to {
			opacity: 0;
			transform: translateY(-20px);
		}
	}

	@keyframes riseIn {
		from {
			opacity: 0;
			transform: translateY(20px);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	@keyframes sinkOut {
		from {
			opacity: 1;
			transform: translateY(0);
		}
		to {
			opacity: 0;
			transform: translateY(20px);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.toast {
			animation: none;
		}

		.toast.dismissing {
			animation: none;
			opacity: 0;
		}
	}
</style>
