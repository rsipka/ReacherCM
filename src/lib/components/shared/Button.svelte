<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
		size?: 'sm' | 'md' | 'lg';
		type?: 'button' | 'submit' | 'reset';
		disabled?: boolean;
		ariaExpanded?: boolean;
		children: Snippet;
		onclick?: (e: MouseEvent) => void;
	}

	let {
		variant = 'primary',
		size = 'md',
		type = 'button',
		disabled = false,
		ariaExpanded,
		children,
		onclick
	}: Props = $props();

	const sizeClasses: Record<string, string> = {
		sm: 'px-3 py-1.5 text-xs',
		md: 'px-4 py-2 text-sm',
		lg: 'px-6 py-3 text-base'
	};
</script>

<button
	{type}
	class="btn btn-{variant} {sizeClasses[size]}"
	{disabled}
	aria-expanded={ariaExpanded}
	{onclick}
>
	{@render children()}
</button>

<style>
	.btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		border-radius: var(--radius-btn);
		font-family: var(--font-sans);
		font-weight: 500;
		border: none;
		cursor: pointer;
		transition:
			background-color var(--duration-default) var(--ease-default),
			opacity var(--duration-default) var(--ease-default),
			transform var(--duration-default) var(--ease-default),
			box-shadow var(--duration-default) var(--ease-default);
		user-select: none;
		white-space: nowrap;
	}

	.btn:active:not(:disabled) {
		transform: scale(0.97);
	}

	.btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	/* Primary */
	.btn-primary {
		background-color: var(--color-accent-fill);
		color: #fff;
	}
	.btn-primary:hover:not(:disabled) {
		background-color: color-mix(in srgb, var(--color-accent-fill) 88%, black);
	}

	/* Secondary */
	.btn-secondary {
		background-color: var(--color-bg-elevated);
		color: var(--color-text-primary);
		/* An outline people can see: the divider colour left a secondary
		   button looking like plain text on a white dialog. */
		border: 1px solid var(--color-border-strong);
	}
	.btn-secondary:hover:not(:disabled) {
		background-color: var(--color-surface-active);
	}

	/* Ghost */
	.btn-ghost {
		background-color: transparent;
		color: var(--color-text-primary);
	}
	.btn-ghost:hover:not(:disabled) {
		background-color: var(--color-surface-hover);
	}

	/* Danger */
	.btn-danger {
		background-color: var(--color-danger);
		color: #fff;
	}
	.btn-danger:hover:not(:disabled) {
		background-color: color-mix(in srgb, var(--color-danger) 88%, black);
	}
</style>
