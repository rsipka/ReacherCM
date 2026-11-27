<script lang="ts">
	import AccountSignIn from '#lib/components/account/AccountSignIn.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import { isMobile } from '#lib/platform.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSyncAccount } from '#lib/state/preference-sync.svelte.js';

	interface Props {
		onNext: () => void;
		onBack: () => void;
		busy: boolean;
	}

	let { onNext, onBack, busy = $bindable() }: Props = $props();
	const mobile = isMobile();
	let account = $derived(getSyncAccount());
</script>

<div class="step">
	<div class="step-header">
		<h2>{t('setup.account_title')}</h2>
		<p>{t('setup.account_subtitle')}</p>
	</div>

	{#if account}
		<div class="account-status" role="status">
			<span>{t('sync_account.signed_in_as')}</span>
			<strong>{account.username}</strong>
			<span>{account.server}</span>
		</div>
	{:else}
		<div class="sign-in">
			<AccountSignIn {mobile} bind:busy />
		</div>
	{/if}

	<div class="step-actions">
		<Button variant="ghost" size="md" disabled={busy && !account} onclick={onBack}>
			{t('setup.back')}
		</Button>
		<Button variant={account ? 'primary' : 'ghost'} size="md" disabled={busy && !account} onclick={onNext}>
			{t(account ? 'setup.next' : 'setup.skip')}
		</Button>
	</div>
</div>

<style>
	.step { display: grid; justify-items: center; gap: 20px; }
	.step-header { text-align: center; }
	h2 {
		margin: 0;
		font-size: 1.25rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}
	p { margin: 6px 0 0; font-size: 0.8125rem; color: var(--color-text-secondary); }
	.sign-in, .account-status, .step-actions { width: 100%; max-width: 380px; }
	.sign-in, .account-status {
		padding: 0 16px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card);
		background: var(--color-bg-elevated);
	}
	.account-status {
		display: grid;
		gap: 4px;
		padding: 14px 16px;
		overflow-wrap: anywhere;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}
	.account-status strong { color: var(--color-text-primary); }
	.step-actions { display: flex; justify-content: space-between; }
</style>
