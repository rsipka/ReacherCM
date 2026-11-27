<script lang="ts">
	import Button from '#lib/components/shared/Button.svelte';
	import { onMount } from 'svelte';
	import { exportIdentity, restoreLocalBeforeLink } from '#lib/state/vault.svelte.js';
	import { localLinkRecoveryAvailable } from '#lib/ipc/vault.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';

	/** The Backup page's key and restore rows: the identity key that opens
	 *  the vaults, and the local vault a link or account move left behind. */

	let error = $state('');
	let exportedKey = $state('');
	let recoveryAvailable = $state(false);
	let confirmRecovery = $state(false);
	let restoring = $state(false);

	onMount(async () => {
		try {
			recoveryAvailable = await localLinkRecoveryAvailable();
		} catch {
			recoveryAvailable = false;
		}
	});

	async function handleExportKey() {
		error = '';
		try {
			exportedKey = await exportIdentity();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function restoreLocalVault(): Promise<void> {
		restoring = true;
		error = '';
		try {
			await restoreLocalBeforeLink();
			confirmRecovery = false;
			recoveryAvailable = false;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			restoring = false;
		}
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
		addToast(t('sync.copied_toast'), 'success');
	}
</script>

<div class="section keys">
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('sync.export_backup_key')}</span>
			<span class="setting-description">{t('sync.export_backup_key_desc')}</span>
		</div>
		<div class="setting-control">
			<Button variant="secondary" size="sm" onclick={handleExportKey}>{t('sync.export_backup_key')}</Button>
		</div>
	</div>

	{#if exportedKey}
		<div class="export-box">
			<p class="export-warning">{t('sync.backup_key_warning')}</p>
			<div class="export-key">
				<code>{exportedKey}</code>
				<button class="copy-btn" onclick={() => copyToClipboard(exportedKey)}>{t('sync.copy')}</button>
			</div>
			<Button variant="ghost" size="sm" onclick={() => (exportedKey = '')}>{t('common.close')}</Button>
		</div>
	{/if}

	{#if recoveryAvailable}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('sync_account.local_vault_restore')}</span>
				<span class="setting-description">{t('sync.local_vault_restore_desc')}</span>
			</div>
			<div class="setting-control">
				{#if !confirmRecovery}
					<Button variant="secondary" size="sm" onclick={() => (confirmRecovery = true)}>
						{t('sync.restore')}
					</Button>
				{/if}
			</div>
		</div>
		{#if confirmRecovery}
			<p class="export-warning">{t('sync_account.local_vault_restore_confirm')}</p>
			<div class="action-row">
				<Button variant="secondary" size="sm" onclick={() => (confirmRecovery = false)} disabled={restoring}>
					{t('common.cancel')}
				</Button>
				<Button variant="danger" size="sm" onclick={restoreLocalVault} disabled={restoring}>
					{t('common.confirm')}
				</Button>
			</div>
		{/if}
	{/if}

	{#if error}
		<div class="form-error">{error}</div>
	{/if}
</div>

<style>
	.keys { padding: 6px 0; }
	.form-error { font-size: 0.75rem; color: var(--color-danger); padding: 4px 0; }
	.copy-btn {
		padding: 4px 8px;
		font-size: 0.6875rem;
		font-weight: 500;
		color: var(--color-accent);
		background: transparent;
		border: 1px solid var(--color-accent);
		border-radius: 4px;
		cursor: pointer;
	}
	.copy-btn:hover { background: color-mix(in srgb, var(--color-accent) 10%, transparent); }
	.export-box {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 12px;
		background: rgba(255, 214, 10, 0.08);
		border: 1px solid rgba(255, 214, 10, 0.3);
		border-radius: 8px;
	}
	.export-warning { margin: 0; font-size: 0.75rem; font-weight: 500; color: var(--color-warning); }
	.export-key {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px;
		background: var(--color-bg-primary);
		border-radius: 4px;
		overflow: hidden;
	}
	.export-key code {
		flex: 1;
		font-size: 0.6875rem;
		font-family: var(--font-mono);
		color: var(--color-text-primary);
		word-break: break-all;
	}
</style>
