<script lang="ts">
	import Button from '#lib/components/shared/Button.svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { isMobile } from '#lib/platform.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { refreshVaults } from '#lib/state/vault.svelte.js';
	import { getSyncAccount } from '#lib/state/preference-sync.svelte.js';

	/** The "Reacher CM account" choice of Sync location: the personal vault
	 *  through the Reacher CM Sync server signed in to under Account. */
	let { onOpenVaultLinks, onChanged }: { onOpenVaultLinks: () => void; onChanged: () => void } = $props();

	const mobile = isMobile();
	let account = $derived(getSyncAccount());
	let vaultSync = $state(false);
	let confirming = $state(false);
	let busy = $state(false);
	let error = $state('');

	// Signing in or out under Account changes what this can do.
	$effect(() => {
		void account;
		void load();
	});

	async function load(): Promise<void> {
		try {
			vaultSync = account ? await invoke<boolean>('vault_account_sync_enabled') : false;
		} catch {
			vaultSync = false;
		}
	}

	async function enable(): Promise<void> {
		busy = true;
		confirming = false;
		error = '';
		try {
			await invoke('vault_enable_account_sync');
			vaultSync = true;
			await refreshVaults();
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
			onChanged();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : String(cause);
		} finally {
			busy = false;
		}
	}
</script>

<div class="setting-row" class:in-use={vaultSync}>
	<div class="setting-info">
		<span class="setting-label">
			{t('sync_account.personal_vault_title')}
			{#if vaultSync}<span class="active-badge">{t('sync.active_badge')}</span>{/if}
		</span>
		<span class="setting-description" role="status">
			{#if !account}
				{t('sync.account_sign_in_first')}
			{:else if vaultSync}
				{t('sync_account.vault_enabled')}
			{:else if busy}
				{t('sync_account.vault_moving')}
			{:else if mobile}
				{t('sync_account.mobile_vault_link_help')}
			{:else}
				{t('sync_account.personal_vault_desc')}
			{/if}
		</span>
	</div>
	<div class="setting-control wrap-control">
		{#if account && !vaultSync}
			{#if mobile}
				<Button variant="secondary" size="sm" onclick={onOpenVaultLinks}>{t('sync.join_device')}</Button>
			{:else if !busy}
				<Button variant="secondary" size="sm" ariaExpanded={confirming} onclick={() => (confirming = !confirming)}>
					{t('sync_account.vault_enable')}
				</Button>
			{/if}
		{/if}
	</div>
</div>
{#if confirming}
	<div class="confirmation">
		<p class="setting-description">{t('sync_account.vault_confirm')}</p>
		<div class="actions">
			<Button variant="ghost" size="sm" onclick={() => (confirming = false)}>{t('common.cancel')}</Button>
			<Button size="sm" disabled={busy} onclick={enable}>{t('common.confirm')}</Button>
		</div>
	</div>
{/if}
{#if error}<p class="form-error" role="alert">{t(error)}</p>{/if}

<style>
	.wrap-control :global(button) { white-space: normal; }
	.confirmation {
		display: grid;
		gap: 10px;
		padding: 12px;
		border-radius: var(--radius-card, 8px);
		background: var(--color-bg-elevated);
	}
	.confirmation p { margin: 0; }
	.actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; }
	.form-error { margin: 0; font-size: 0.75rem; color: var(--color-danger); }
</style>
