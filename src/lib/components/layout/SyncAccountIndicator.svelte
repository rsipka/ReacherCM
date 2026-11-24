<script lang="ts">
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSyncAccount, preferenceSync } from '#lib/state/preference-sync.svelte.js';
	import { openSettings } from '#lib/state/shortcuts.svelte.js';

	let account = $derived(getSyncAccount());
	let status = $derived.by(() => {
		if (!preferenceSync.online) return 'offline';
		if (preferenceSync.error) return 'warning';
		if (preferenceSync.syncing) return 'syncing';
		return 'signed-in';
	});
	let statusLabel = $derived.by(() => {
		if (status === 'offline') return t('sync_account.offline');
		if (status === 'warning') return t(preferenceSync.error);
		if (status === 'syncing') return t('sync_account.syncing');
		return '';
	});
	let tooltip = $derived(account
		? [`${t('sync_account.signed_in_as')} ${account.username}`, statusLabel].filter(Boolean).join(' · ')
		: '');
</script>

{#if account}
	<button
		type="button"
		class="account-indicator"
		class:attention={status === 'warning'}
		title={tooltip}
		aria-label={tooltip}
		onclick={() => openSettings('account')}
	>
		<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
			{#if status === 'syncing'}
				<path d="M20 7v5h-5M4 17v-5h5M6 8a7 7 0 0 1 11-2l3 6M4 12l3 6a7 7 0 0 0 11-2" />
			{:else}
				<path d="M7 17a4 4 0 1 1 .5-8 6 6 0 0 1 11.6 1.7A3.2 3.2 0 0 1 19 17h-1" />
				{#if status === 'offline'}
					<path d="m4 4 16 16" />
				{:else if status === 'warning'}
					<path d="M12 11.5v4m0 3h.01" />
				{:else}
					<path d="m9 16 2 2 4-4" />
				{/if}
			{/if}
		</svg>
		<span>{account.username}</span>
	</button>
{/if}

<style>
	.account-indicator {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		min-width: 14px;
		height: 20px;
		padding: 0;
		border: 0;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		font: 11px/1 var(--font-sans);
		cursor: pointer;
	}

	.account-indicator:hover,
	.account-indicator:focus-visible {
		color: var(--color-text-primary);
	}

	.account-indicator:focus-visible {
		outline: 1px solid var(--color-accent);
		outline-offset: 2px;
	}

	.attention svg { color: var(--color-warning); }
	svg { flex: none; }

	span {
		max-inline-size: min(14ch, 28vw);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
