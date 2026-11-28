<script lang="ts">
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import {
		confirmVaultRestore,
		reviewVaultRestore,
		type VaultRestoreReview
	} from '#lib/ipc/vault.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';

	let { vaultId, vaultName }: { vaultId?: string; vaultName?: string } = $props();
	let title = $derived(vaultId ? t('db.restore_title', { name: vaultName ?? '' }) : t('sync.restore_title'));
	let review = $state<VaultRestoreReview | null>(null);
	let busy = $state(false);

	async function checkRequest() {
		busy = true;
		try {
			review = await reviewVaultRestore(vaultId);
			if (!review) addToast(t('sync.restore_none'), 'info');
		} catch (error) {
			addToast(t(String(error)), 'error');
		} finally {
			busy = false;
		}
	}

	async function confirm(reviewed: VaultRestoreReview) {
		busy = true;
		try {
			await confirmVaultRestore(reviewed, vaultId);
			addToast(t('sync.restore_published'), 'success');
		} catch (error) {
			addToast(t(String(error)), 'error');
		} finally {
			review = null;
			busy = false;
		}
	}
</script>

<Button variant="secondary" size="sm" onclick={checkRequest} disabled={busy}>
	{t('sync.restore_check')}
</Button>

<Modal open={review !== null} onclose={() => { if (!busy) review = null; }} title={title} zIndex={200}>
	{#if review}
		<p>{t('sync.restore_summary')}</p>
		<p>{t('sync.restore_revision')} <code>{review.snapshotRevision.slice(1, -1)}</code></p>
	{/if}
	{#snippet actions()}
		<Button variant="ghost" onclick={() => (review = null)} disabled={busy}>{t('common.cancel')}</Button>
		<Button variant="danger" onclick={() => review && confirm(review)} disabled={busy || !review}>
			{title}
		</Button>
	{/snippet}
</Modal>

<style>
	code { overflow-wrap: anywhere; }
</style>
