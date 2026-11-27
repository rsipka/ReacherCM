<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import ContextMenuBackdrop from '#lib/components/shared/ContextMenuBackdrop.svelte';
	import { syncStatus, type SyncStatus } from '#lib/ipc/vault.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSyncAccount, preferenceSync } from '#lib/state/preference-sync.svelte.js';
	import { openSettings } from '#lib/state/shortcuts.svelte.js';
	import { vaultState } from '#lib/state/vault.svelte.js';

	const backendNames: Record<string, string> = {
		account: 'Reacher CM Sync',
		turso: 'Turso',
		sftp: 'SFTP',
		smb: 'SMB',
		git: 'Git',
		s3: 'S3'
	};
	const panelId = $props.id();
	let trigger = $state<HTMLButtonElement>();
	let panel = $state<HTMLDivElement>();
	let bottom = $state<number | null>(null);
	let vaults = $state<SyncStatus[] | null>(null);
	let unavailable = $state(false);
	let revision = 0;
	let account = $derived(getSyncAccount());
	let visible = $derived(!!account || (!vaultState.locked && !!vaults?.length));
	let attention = $derived(vaults?.some((vault) => vault.lastError || vault.conflicts > 0) ?? false);
	let pending = $derived(vaults?.reduce((total, vault) => total + vault.pending, 0) ?? 0);
	let waitsForFirstSync = $derived(vaults?.some((vault) => vault.lastSync === null) ?? false);
	let status = $derived.by(() => {
		if (!preferenceSync.online) return 'offline';
		if ((account && preferenceSync.error) || attention || unavailable) return 'warning';
		if (account && preferenceSync.syncing) return 'syncing';
		if (pending || waitsForFirstSync) return 'waiting';
		return 'idle';
	});
	let statusLabel = $derived.by(() => {
		if (status === 'offline') return t('sync_account.offline');
		if (status === 'warning') {
			if (unavailable) return t('sync.status_unavailable');
			if (attention) return t('sync.status_attention');
			return t(preferenceSync.error);
		}
		if (status === 'syncing') return t('sync_account.syncing');
		if (pending) return t('sync.pending_changes', { count: String(pending) });
		if (waitsForFirstSync) return t('sync.never_synced');
		return '';
	});
	let tooltip = $derived([
		t('sync.status_title'),
		account ? `${t('sync_account.signed_in_as')} ${account.username}` : '',
		statusLabel
	].filter(Boolean).join(' · '));

	async function refresh(): Promise<void> {
		const request = ++revision;
		if (vaultState.locked) {
			vaults = null;
			unavailable = false;
			return;
		}
		try {
			const result = await syncStatus();
			if (request !== revision || vaultState.locked) return;
			vaults = result;
			unavailable = false;
		} catch {
			if (request === revision && !vaultState.locked) unavailable = true;
		}
	}

	onMount(() => {
		const unlisten = listen('vault-sync-status', () => void refresh()).catch(() => undefined);
		return () => {
			revision++;
			void unlisten.then((stop) => stop?.());
		};
	});

	$effect(() => {
		// Re-read after a vault is added, removed, or unlocked, even before a sync finishes.
		void vaultState.vaultList;
		void refresh();
	});

	$effect(() => {
		if (!visible) close(false);
	});

	async function open(): Promise<void> {
		if (!trigger) return;
		bottom = window.innerHeight - trigger.getBoundingClientRect().top + 6;
		void refresh();
		await tick();
		panel?.focus();
	}

	function close(restoreFocus = true): void {
		bottom = null;
		if (restoreFocus) trigger?.focus();
	}

	function goToSettings(page: 'sync' | 'account'): void {
		close(false);
		openSettings(page);
	}

	function handleKey(event: KeyboardEvent): void {
		if (bottom === null || event.key !== 'Escape') return;
		event.preventDefault();
		event.stopPropagation();
		close();
	}
</script>

<svelte:window onkeydowncapture={handleKey} onresize={() => close(false)} />

{#if visible}
	<button
		bind:this={trigger}
		type="button"
		class="account-indicator"
		class:attention={status === 'warning' || status === 'waiting'}
		title={tooltip}
		aria-label={tooltip}
		aria-haspopup="dialog"
		aria-controls={bottom === null ? undefined : panelId}
		aria-expanded={bottom !== null}
		onclick={open}
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
				{:else if status === 'waiting'}
					<circle cx="12.5" cy="16.5" r="4" />
					<path d="M12.5 14.5v2l1.2 1" />
				{:else}
					<path d="m9 16 2 2 4-4" />
				{/if}
			{/if}
		</svg>
		<span class="account-name">{account?.username ?? t('sync.status_title')}</span>
	</button>

	{#if bottom !== null}
		<ContextMenuBackdrop onclose={close} />
		<div
			bind:this={panel}
			id={panelId}
			class="sync-panel"
			role="dialog"
			aria-label={t('sync.status_title')}
			tabindex="-1"
			style:--panel-bottom="{bottom}px"
			onfocusout={(event) => {
				if (event.relatedTarget instanceof Node && !panel?.contains(event.relatedTarget)) close(false);
			}}
		>
			<header>
				<h2>{t('sync.status_title')}</h2>
				<button type="button" class="close-button" aria-label={t('common.close')} onclick={() => close()}>
					<svg viewBox="0 0 16 16" width="16" height="16" fill="none" aria-hidden="true">
						<path d="m4 4 8 8m0-8-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
					</svg>
				</button>
			</header>
			{#if !preferenceSync.online}
				<p>{t('sync_account.offline')}</p>
			{/if}
			{#if account && preferenceSync.error}
				<p class="bad">{t(preferenceSync.error)}</p>
			{:else if account && preferenceSync.syncing}
				<p>{t('sync_account.syncing')}</p>
			{/if}
			{#if vaultState.locked}
				<p>{t('vault.locked_toast')}</p>
			{:else}
				{#if unavailable}
					<p class="bad" role="status">{t('sync.status_unavailable')}</p>
				{:else if vaults === null}
					<p role="status">{t('common.loading')}</p>
				{:else if vaults.length === 0}
					<p>{t('sync.status_none')}</p>
				{/if}
				{#if vaults?.length}
					<ul>
						{#each vaults as vault (vault.vaultId)}
							<li>
								<div class="vault-heading">
									<strong>{vault.name.startsWith('__') ? t('sync.personal_vault') : vault.name}</strong>
									{#if vault.backend === 'folder' || backendNames[vault.backend]}
										<span>{vault.backend === 'folder' ? t('sync.folder_path') : backendNames[vault.backend]}</span>
									{/if}
								</div>
								<span>{vault.lastSync === null
									? t('sync.never_synced')
									: t('sync.last_synced', { time: new Date(vault.lastSync).toLocaleString() })}</span>
								{#if vault.pending > 0}
									<span>{t('sync.pending_changes', { count: String(vault.pending) })}</span>
								{/if}
								{#if vault.lastError}
									<span class="bad">{t('sync.sync_failed', { error: t(vault.lastError) })}</span>
								{/if}
								{#if vault.conflicts > 0}
									<span class="warning">{t('sync.conflicts', { count: String(vault.conflicts) })}</span>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			{/if}
			<footer>
				<button type="button" onclick={() => goToSettings('sync')}>{t('settings.page_sync_backup')}</button>
				{#if account}
					<button type="button" onclick={() => goToSettings('account')}>{t('sync_account.title')}</button>
				{/if}
			</footer>
		</div>
	{/if}
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

	.account-name {
		max-inline-size: min(14ch, 28vw);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.sync-panel {
		position: fixed;
		z-index: 1000;
		right: calc(var(--safe-right, 0px) + 8px);
		bottom: var(--panel-bottom);
		box-sizing: border-box;
		width: min(352px, calc(100vw - var(--safe-left, 0px) - var(--safe-right, 0px) - 16px));
		max-height: min(420px, calc(100dvh - var(--panel-bottom) - var(--safe-top, 0px) - 8px));
		overflow-y: auto;
		overscroll-behavior: contain;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card, 8px);
		background: var(--color-bg-elevated);
		box-shadow: var(--shadow-elevated, 0 8px 24px rgba(0, 0, 0, 0.35));
		padding: 12px;
		color: var(--color-text-secondary);
		font: 12px/1.4 var(--font-sans);
		white-space: normal;
		overflow-wrap: anywhere;
	}

	.sync-panel:focus { outline: none; }

	header,
	footer {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	header { justify-content: space-between; }
	footer { flex-wrap: wrap; margin-top: 12px; }

	h2,
	p,
	ul { margin: 0; }
	h2 { font-size: 13px; }
	h2,
	strong { color: var(--color-text-primary); }
	p { margin-top: 6px; }
	ul { padding: 0; list-style: none; }
	li {
		display: grid;
		gap: 2px;
		margin-top: 8px;
		padding-top: 8px;
		border-top: 1px solid var(--color-border);
	}
	.vault-heading {
		display: flex;
		justify-content: space-between;
		gap: 8px;
	}
	.vault-heading span { flex: none; }
	.bad { color: var(--color-danger); }
	.warning { color: var(--color-warning); }

	.sync-panel button {
		padding: 4px 8px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm, 4px);
		background: transparent;
		color: var(--color-text-primary);
		font: inherit;
		cursor: pointer;
	}

	.sync-panel button:hover { background: var(--color-surface-hover); }
	.sync-panel .close-button { display: flex; padding: 3px; border-color: transparent; }
</style>
