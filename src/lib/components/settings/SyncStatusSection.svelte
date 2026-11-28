<script lang="ts">
	import { onMount } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import VaultRestore from './VaultRestore.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import {
		syncStatus,
		syncVault,
		upgradeSyncFormat,
		clearSyncConflicts,
		type SyncStatus
	} from '#lib/ipc/vault.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';

	let statuses = $state<SyncStatus[]>([]);
	let busy = $state<string | null>(null);
	let confirmUpgrade = $state<string | null>(null);

	async function refresh() {
		try {
			statuses = await syncStatus();
		} catch {
			statuses = [];
		}
	}

	onMount(() => {
		void refresh();
		const unlisten = listen('vault-sync-status', () => void refresh());
		return () => void unlisten.then((u) => u());
	});

	function label(s: SyncStatus): string {
		return s.name.startsWith('__') ? t('sync.personal_vault') : s.name;
	}

	async function run(id: string, action: () => Promise<void>, done: string) {
		busy = id;
		try {
			await action();
			addToast(done, 'success');
		} catch (e) {
			addToast(t(e instanceof Error ? e.message : String(e)), 'error');
		} finally {
			busy = null;
			confirmUpgrade = null;
			await refresh();
		}
	}
</script>

<div class="section status">
	<p class="section-desc">{t('sync.status_desc')}</p>

	{#if statuses.length === 0}
		<p class="section-desc">{t('sync.status_none')}</p>
	{/if}

	{#each statuses as s (s.vaultId)}
		<div class="vault-status">
			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{label(s)}</span>
					<span class="setting-description">
						{s.lastSync
							? t('sync.last_synced', { time: new Date(s.lastSync).toLocaleString() })
							: t('sync.never_synced')}
					</span>
					{#if s.pending > 0}
						<span class="setting-description">{t('sync.pending_changes', { count: String(s.pending) })}</span>
					{/if}
					{#if s.lastError}
						<span class="setting-description bad">{t('sync.sync_failed', { error: t(s.lastError) })}</span>
					{/if}
					{#if s.format === 'legacy'}
						<span class="setting-description warn">{t('sync.legacy_format')}</span>
					{/if}
				</div>
				<div class="buttons">
					{#if s.canRestoreShared}<VaultRestore vaultId={s.vaultId} vaultName={label(s)} />{/if}
					{#if s.format === 'legacy'}
						<Button
							variant="secondary"
							size="sm"
							disabled={busy !== null}
							onclick={() => (confirmUpgrade = s.vaultId)}
						>
							{t('sync.upgrade')}
						</Button>
					{/if}
					<Button
						variant="secondary"
						size="sm"
						disabled={busy !== null}
						onclick={() => run(s.vaultId, () => syncVault(s.vaultId), t('sync.synced_toast'))}
					>
						{busy === s.vaultId ? t('sync.syncing') : t('sync.sync_now')}
					</Button>
				</div>
			</div>

			{#if confirmUpgrade === s.vaultId}
				<div class="confirm-box">
					<p>{t('sync.upgrade_confirm')}</p>
					<div class="buttons">
						<Button variant="ghost" size="sm" onclick={() => (confirmUpgrade = null)}>
							{t('common.cancel')}
						</Button>
						<Button
							variant="primary"
							size="sm"
							disabled={busy !== null}
							onclick={() => run(s.vaultId, () => upgradeSyncFormat(s.vaultId), t('sync.upgraded_toast'))}
						>
							{busy === s.vaultId ? t('sync.upgrading') : t('sync.upgrade_go')}
						</Button>
					</div>
				</div>
			{/if}

			{#if s.conflicts > 0}
				<div class="conflict-row">
					<span>{t('sync.conflicts', { count: String(s.conflicts) })}</span>
					<Button
						variant="ghost"
						size="sm"
						onclick={async () => {
							await clearSyncConflicts(s.vaultId);
							await refresh();
						}}
					>
						{t('sync.conflicts_dismiss')}
					</Button>
				</div>
			{/if}
		</div>
	{/each}
</div>

<style>
	.status {
		padding: 12px 0;
	}

	.section-desc {
		margin: 0;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.vault-status {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.buttons {
		display: flex;
		gap: 8px;
		flex-shrink: 0;
	}

	.bad {
		color: var(--color-danger);
	}

	.warn {
		color: var(--color-warning);
	}

	.confirm-box,
	.conflict-row {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 10px 12px;
		font-size: 0.75rem;
		background: rgba(255, 214, 10, 0.08);
		border: 1px solid rgba(255, 214, 10, 0.3);
		border-radius: 8px;
	}

	.confirm-box p {
		margin: 0;
	}

	.conflict-row {
		flex-direction: row;
		align-items: center;
		justify-content: space-between;
	}
</style>
