<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import OrganizationVaults from './OrganizationVaults.svelte';
	import { Channel, invoke } from '@tauri-apps/api/core';
	import { isMobile } from '#lib/platform.js';
	import { scanLinkCode } from '#lib/state/link-scan.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { refreshVaults } from '#lib/state/vault.svelte.js';

	import {
		bindPreferenceAccount, preferenceSync, setPreferenceSync, syncPreferences, type SyncAccount
	} from '#lib/state/preference-sync.svelte.js';

	let { onOpenVaultLinks }: { onOpenVaultLinks: () => void } = $props();

	const mobile = isMobile();
	let pairingCode = $state('');
	let pairingServer = $state('');
	let verificationCode = $state('');
	let account = $state<SyncAccount | null>(null);
	let server = $state('');
	let label = $state('');
	let vaultSync = $state(false);
	let confirmVaultSync = $state(false);
	let action = $state<'load' | 'sign-in' | 'check' | 'sign-out' | 'vault' | 'scan' | 'pair' | null>('load');
	let error = $state('');
	let message = $state('');

	onMount(() => { void load(); });
	onDestroy(() => {
		if (action === 'pair') void invoke('sync_account_cancel');
	});

	$effect(() => {
		if (preferenceSync.error === 'sync_account.error_revoked' && account && action === null) {
			error = preferenceSync.error;
			account = null;
			bindPreferenceAccount(null);
		}
	});

	async function load(): Promise<void> {
		try {
			account = await invoke<SyncAccount | null>('sync_account_status');
			if (account) {
				server = account.server;
				vaultSync = await invoke<boolean>('vault_account_sync_enabled');
			}
			bindPreferenceAccount(account);
		} catch (cause) {
			error = errorKey(cause);
		} finally {
			action = null;
		}
	}

	function errorKey(cause: unknown): string {
		return typeof cause === 'string' && cause.startsWith('sync_account.')
			? cause
			: 'sync_account.error_server';
	}

	async function signIn(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		action = 'sign-in';
		error = '';
		message = 'sync_account.browser_pending';
		try {
			account = await invoke<SyncAccount>('sync_account_sign_in', { server, label });
			server = account.server;
			bindPreferenceAccount(account);
			vaultSync = await invoke<boolean>('vault_account_sync_enabled');
			message = '';
		} catch (cause) {
			error = errorKey(cause);
			message = '';
		} finally {
			action = null;
		}
	}

	async function scanPairing(): Promise<void> {
		action = 'scan';
		error = '';
		try {
			const code = await scanLinkCode();
			if (code) {
				pairingCode = code;
				pairingServer = await invoke<string>('sync_account_pair_origin', { code });
			}
		} catch (cause) {
			error = typeof cause === 'string' ? errorKey(cause) : 'sync_account.pair_camera';
		} finally {
			action = null;
		}
	}

	async function pair(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		error = '';
		message = '';
		if (!pairingServer) {
			action = 'scan';
			try {
				pairingServer = await invoke<string>('sync_account_pair_origin', { code: pairingCode });
			} catch (cause) {
				error = errorKey(cause);
			} finally {
				action = null;
			}
			return;
		}
		action = 'pair';
		try {
			const verification = new Channel<string>((code) => { verificationCode = code; });
			account = await invoke<SyncAccount>('sync_account_pair', { code: pairingCode, label, verification });
			server = pairingServer;
			bindPreferenceAccount(account);
			vaultSync = await invoke<boolean>('vault_account_sync_enabled');
		} catch (cause) {
			error = errorKey(cause);
		} finally {
			verificationCode = '';
			pairingCode = '';
			pairingServer = '';
			action = null;
		}
	}

	async function check(): Promise<void> {
		action = 'check';
		error = '';
		message = '';
		try {
			account = await invoke<SyncAccount>('sync_account_check');
			message = 'sync_account.connected';
		} catch (cause) {
			error = errorKey(cause);
			if (error === 'sync_account.error_revoked') {
				account = null;
				bindPreferenceAccount(null);
			}
		} finally {
			action = null;
		}
	}

	async function signOut(): Promise<void> {
		action = 'sign-out';
		error = '';
		message = '';
		try {
			const revoked = await invoke<boolean>('sync_account_sign_out');
			account = null;
			bindPreferenceAccount(null);
			if (!revoked) message = 'sync_account.signed_out_locally';
		} catch (cause) {
			error = errorKey(cause);
		} finally {
			action = null;
		}
	}

	async function enableVaultSync(): Promise<void> {
		action = 'vault';
		confirmVaultSync = false;
		error = '';
		message = '';
		try {
			await invoke('vault_enable_account_sync');
			vaultSync = true;
			await refreshVaults();
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
		} catch (cause) {
			error = cause instanceof Error ? cause.message : String(cause);
		} finally {
			action = null;
		}
	}

	async function manage(): Promise<void> {
		try {
			await invoke('sync_account_manage');
		} catch (cause) {
			error = errorKey(cause);
		}
	}
</script>

<div class="account">
	{#if account}
		<dl>
			<dt>{t('sync_account.server')}</dt><dd>{account.server}</dd>
			<dt>{t('sync_account.signed_in_as')}</dt><dd>{account.username}</dd>
		</dl>
		<div class="actions">
			<button type="button" disabled={action !== null} onclick={manage}>{t('sync_account.manage')}</button>
			<button type="button" disabled={action !== null} onclick={check}>{t('sync_account.check')}</button>
			<button type="button" disabled={action !== null} onclick={signOut}>{t('sync_account.sign_out')}</button>
		</div>
		<div class="preferences">
			<label>
				<input type="checkbox" checked={preferenceSync.enabled}
					disabled={action !== null || preferenceSync.error === 'sync_account.error_revoked'}
					onchange={(event) => setPreferenceSync(event.currentTarget.checked)} />
				<span>{t('sync_account.sync_preferences')}</span>
			</label>
			<p>{t('sync_account.sync_preferences_desc')}</p>
			{#if preferenceSync.enabled}
				<button type="button" disabled={preferenceSync.syncing || action !== null}
					onclick={() => void syncPreferences()}>{t('sync_account.sync_now')}</button>
			{/if}
			{#if preferenceSync.syncing}<p role="status">{t('sync_account.syncing')}</p>{/if}
			{#if preferenceSync.error}<p class="error" role="alert">{t(preferenceSync.error)}</p>{/if}
		</div>
		{#if mobile}
			<div class="preferences">
				{#if vaultSync}
					<p role="status">{t('sync_account.vault_enabled')}</p>
				{:else}
					<p>{t('sync_account.mobile_vault_link_help')}</p>
					<button type="button" disabled={action !== null} onclick={onOpenVaultLinks}>{t('sync.join_device')}</button>
				{/if}
			</div>
		{:else}
			<div class="preferences">
				{#if vaultSync}
					<p role="status">{t('sync_account.vault_enabled')}</p>
				{:else if action === 'vault'}
					<p role="status">{t('sync_account.vault_moving')}</p>
				{:else if confirmVaultSync}
					<p>{t('sync_account.vault_confirm')}</p>
					<div class="actions">
						<button type="button" onclick={enableVaultSync}>{t('common.confirm')}</button>
						<button type="button" onclick={() => confirmVaultSync = false}>{t('common.cancel')}</button>
					</div>
				{:else}
					<button type="button" disabled={action !== null} onclick={() => confirmVaultSync = true}>{t('sync_account.vault_enable')}</button>
				{/if}
			</div>
			{#key account.server + '/' + account.username}
				<OrganizationVaults />
			{/key}
		{/if}
	{:else if action === 'load'}
		<p role="status">{t('sync_account.loading')}</p>
	{:else if mobile}
		<form onsubmit={pair}>
			<p>{t('sync_account.pair_help')}</p>
			{#if pairingServer}
				<dl><dt>{t('sync_account.server')}</dt><dd>{pairingServer}</dd></dl>
			{:else}
				<button type="button" disabled={action !== null} onclick={scanPairing}>{t('sync_account.pair_scan')}</button>
				<label for="sync-pair-code">{t('sync_account.pair_code')}</label>
				<input id="sync-pair-code" bind:value={pairingCode} maxlength="2048" autocomplete="off"
					spellcheck="false" autocapitalize="off" required disabled={action !== null} />
			{/if}
			<label for="sync-mobile-name">{t('sync_account.device_name')}</label>
			<input id="sync-mobile-name" bind:value={label} maxlength="64" required disabled={action !== null} />
			{#if verificationCode}
				<p role="status">{t('sync_account.pair_match')}</p>
				<strong class="verification">{verificationCode}</strong>
			{/if}
			<div class="actions">
				<button class="primary" type="submit" disabled={action !== null}>
					{t(pairingServer ? 'sync_account.pair_start' : 'common.confirm')}
				</button>
				{#if action === 'pair'}
					<button type="button" onclick={() => void invoke('sync_account_cancel')}>{t('common.cancel')}</button>
				{:else if pairingServer}
					<button type="button" onclick={() => { pairingServer = ''; pairingCode = ''; }}>{t('common.cancel')}</button>
				{/if}
			</div>
		</form>
	{:else}
		<form onsubmit={signIn}>
			<label class="setting-label" for="sync-origin">{t('sync_account.server')}</label>
			<input id="sync-origin" type="url" bind:value={server} placeholder="https://sync.example.com" required disabled={action !== null} />
			<label class="setting-label" for="sync-device-name">{t('sync_account.device_name')}</label>
			<input id="sync-device-name" bind:value={label} maxlength="64" required disabled={action !== null} />
			<div class="actions">
				<button class="primary" type="submit" disabled={action !== null}>{t('sync_account.sign_in')}</button>
				{#if action === 'sign-in'}
					<button type="button" onclick={() => void invoke('sync_account_cancel')}>{t('common.cancel')}</button>
				{/if}
			</div>
		</form>
	{/if}
	{#if message}<p role="status">{t(message)}</p>{/if}
	{#if error}<p class="error" role="alert">{t(error)}</p>{/if}
</div>

<style>
	.account { padding: 14px 0; }
	form { display: grid; gap: 8px; max-width: 30rem; }
	form input {
		width: 100%;
		padding: 7px 9px;
		border-radius: 6px;
		border: 1px solid var(--color-border-strong);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font: inherit;
	}
	dl { margin: 0 0 12px; display: grid; gap: 4px; }
	dt { color: var(--color-text-secondary); }
	dd { margin: 0 0 6px; overflow-wrap: anywhere; }
	.actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 6px; }
	button {
		padding: 7px 12px;
		border: 1px solid var(--color-border-strong);
		border-radius: 6px;
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
		font: inherit;
		cursor: pointer;
	}
	button:hover:enabled { border-color: var(--color-accent); }
	button.primary { background: var(--color-accent-fill); border-color: transparent; color: white; }
	button:disabled { opacity: 0.6; cursor: default; }
	p { margin: 10px 0 0; color: var(--color-text-secondary); }
	.preferences { margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--color-border); }
	.preferences label { display: flex; align-items: center; gap: 7px; }
	.preferences label input { accent-color: var(--color-accent); }
	.preferences button { margin-top: 8px; }
	.verification { font-size: 1.75rem; font-family: monospace; letter-spacing: 0.15em; }
	.error { color: var(--color-danger); }
</style>
