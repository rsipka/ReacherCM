<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import OrganizationVaults from './OrganizationVaults.svelte';
	import SettingsCard from './SettingsCard.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import Toggle from '#lib/components/shared/Toggle.svelte';
	import { Channel, invoke } from '@tauri-apps/api/core';
	import { isMobile } from '#lib/platform.js';
	import { scanLinkCode } from '#lib/state/link-scan.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { refreshVaults } from '#lib/state/vault.svelte.js';

	import {
		bindPreferenceAccount, getSyncAccount, preferenceSync, setPreferenceSync, syncPreferences, type SyncAccount
	} from '#lib/state/preference-sync.svelte.js';

	let { onOpenVaultLinks }: { onOpenVaultLinks: () => void } = $props();

	const mobile = isMobile();
	let pairingCode = $state('');
	let pairingServer = $state('');
	let verificationCode = $state('');
	let account = $derived(getSyncAccount());
	let server = $state('');
	let label = $state('');
	let vaultSync = $state(false);
	let confirmVaultSync = $state(false);
	let vaultError = $state('');
	let action = $state<'load' | 'sign-in' | 'check' | 'sign-out' | 'vault' | 'scan' | 'pair' | null>('load');
	let error = $state('');
	let message = $state('');

	onMount(() => { void load(); });
	onDestroy(() => {
		if (action === 'pair') void invoke('sync_account_cancel');
	});

	$effect(() => {
		if (preferenceSync.error === 'sync_account.error_revoked' && action === null) {
			error = preferenceSync.error;
		}
	});

	async function load(): Promise<void> {
		try {
			const saved = await invoke<SyncAccount | null>('sync_account_status');
			bindPreferenceAccount(saved);
			if (saved) {
				server = saved.server;
				vaultSync = await invoke<boolean>('vault_account_sync_enabled');
			}
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
			const signedIn = await invoke<SyncAccount>('sync_account_sign_in', { server, label });
			server = signedIn.server;
			bindPreferenceAccount(signedIn);
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
			const paired = await invoke<SyncAccount>('sync_account_pair', { code: pairingCode, label, verification });
			server = paired.server;
			bindPreferenceAccount(paired);
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
			bindPreferenceAccount(await invoke<SyncAccount>('sync_account_check'));
			message = 'sync_account.connected';
		} catch (cause) {
			error = errorKey(cause);
			if (error === 'sync_account.error_revoked') {
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
			bindPreferenceAccount(null);
			confirmVaultSync = false;
			vaultError = '';
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
		vaultError = '';
		try {
			await invoke('vault_enable_account_sync');
			vaultSync = true;
			await refreshVaults();
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
		} catch (cause) {
			vaultError = cause instanceof Error ? cause.message : String(cause);
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

{#snippet accountFeedback()}
	{#if message}<p class="feedback" role="status">{t(message)}</p>{/if}
	{#if error}<p class="feedback error" role="alert">{t(error)}</p>{/if}
{/snippet}

{#if account}
	<SettingsCard title={t('sync_account.connection_title')}>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('sync_account.signed_in_as')}</span>
				<span class="setting-description server-address">{account.username}</span>
			</div>
			<div class="setting-control">
				<Button variant="secondary" size="sm" disabled={action !== null} onclick={manage}>
					{t('sync_account.manage')}
				</Button>
			</div>
		</div>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('sync_account.server')}</span>
				<span class="setting-description server-address">{account.server}</span>
			</div>
			<div class="setting-control">
				<Button variant="secondary" size="sm" disabled={action !== null} onclick={check}>
					{t(action === 'check' ? 'sync_account.loading' : 'sync_account.check')}
				</Button>
			</div>
		</div>
		{@render accountFeedback()}
		<div class="footer-actions">
			<Button variant="ghost" size="sm" disabled={action !== null} onclick={signOut}>
				{t('sync_account.sign_out')}
			</Button>
		</div>
	</SettingsCard>

	<SettingsCard title={t('sync_account.preferences_title')}>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('sync_account.sync_preferences')}</span>
				<span class="setting-description">{t('sync_account.sync_preferences_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle hideLabel label={t('sync_account.sync_preferences')} checked={preferenceSync.enabled}
					disabled={action !== null || preferenceSync.error === 'sync_account.error_revoked'}
					onchange={setPreferenceSync} />
			</div>
		</div>
		{#if preferenceSync.enabled}
			<div class="setting-row">
				<div class="setting-info">
					{#if preferenceSync.syncing}<span class="setting-description" role="status">{t('sync_account.syncing')}</span>{/if}
					{#if preferenceSync.error}<span class="setting-description error" role="alert">{t(preferenceSync.error)}</span>{/if}
				</div>
				<div class="setting-control">
					<Button variant="secondary" size="sm" disabled={preferenceSync.syncing || action !== null}
						onclick={() => void syncPreferences()}>{t('sync_account.sync_now')}</Button>
				</div>
			</div>
		{/if}
	</SettingsCard>

	<SettingsCard title={t('sync_account.personal_vault_title')}>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('sync.sync_status')}</span>
				<span class="setting-description" role="status">
					{#if vaultSync}
						{t('sync_account.vault_enabled')}
					{:else if action === 'vault'}
						{t('sync_account.vault_moving')}
					{:else if mobile}
						{t('sync_account.mobile_vault_link_help')}
					{:else}
						{t('sync_account.personal_vault_desc')}
					{/if}
				</span>
			</div>
			<div class="setting-control wrap-control">
				{#if vaultSync}
					<span class="enabled">{t('sync.enabled')}</span>
				{:else if mobile}
					<Button variant="secondary" size="sm" disabled={action !== null} onclick={onOpenVaultLinks}>{t('sync.join_device')}</Button>
				{:else if action !== 'vault'}
					<Button variant="secondary" size="sm" disabled={action !== null}
						ariaExpanded={confirmVaultSync} onclick={() => confirmVaultSync = !confirmVaultSync}>
						{t('sync_account.vault_enable')}
					</Button>
				{/if}
			</div>
		</div>
		{#if confirmVaultSync}
			<div class="confirmation">
				<p class="setting-description">{t('sync_account.vault_confirm')}</p>
				<div class="actions">
					<Button variant="ghost" size="sm" onclick={() => confirmVaultSync = false}>{t('common.cancel')}</Button>
					<Button size="sm" disabled={action !== null} onclick={enableVaultSync}>{t('common.confirm')}</Button>
				</div>
			</div>
		{/if}
		{#if vaultError}<p class="feedback error" role="alert">{t(vaultError)}</p>{/if}
	</SettingsCard>

	{#if !mobile}
		{#key account.server + '/' + account.username}
			<OrganizationVaults />
		{/key}
	{/if}
{:else if action === 'load'}
	<SettingsCard><p class="feedback" role="status">{t('sync_account.loading')}</p></SettingsCard>
{:else if mobile}
	<SettingsCard title={t('sync_account.pair_start')}>
		<form class="auth-form" onsubmit={pair}>
			{#if pairingServer}
				<div class="setting-info">
					<span class="setting-label">{t('sync_account.server')}</span>
					<span class="setting-description server-address">{pairingServer}</span>
				</div>
				<Input label={t('sync_account.device_name')} bind:value={label} maxlength={64} required disabled={action !== null} />
			{:else}
				<p class="setting-description">{t('sync_account.pair_help')}</p>
				<div class="scan-action">
					<Button variant="secondary" disabled={action !== null} onclick={scanPairing}>{t('sync_account.pair_scan')}</Button>
				</div>
				<Input label={t('sync_account.pair_code')} bind:value={pairingCode} maxlength={2048}
					autocomplete="off" spellcheck={false} autocapitalize="off" required disabled={action !== null} />
			{/if}
			{#if verificationCode}
				<div class="confirmation" role="status">
					<p class="setting-description">{t('sync_account.pair_match')}</p>
					<strong class="verification">{verificationCode}</strong>
				</div>
			{/if}
			{@render accountFeedback()}
			<div class="actions">
				{#if action === 'pair' || pairingServer}
					<Button variant="ghost" onclick={() => {
						if (action === 'pair') void invoke('sync_account_cancel');
						else { pairingServer = ''; pairingCode = ''; }
					}}>{t('common.cancel')}</Button>
				{/if}
				<Button type="submit" disabled={action !== null || !(pairingServer ? label.trim() : pairingCode.trim())}>
					{t(pairingServer ? 'sync_account.pair_start' : 'authprompt.continue')}
				</Button>
			</div>
		</form>
	</SettingsCard>
{:else}
	<SettingsCard title={t('sync_account.sign_in')}>
		<form class="auth-form" onsubmit={signIn}>
			<p class="setting-description">{t('sync_account.sign_in_help')}</p>
			<Input label={t('sync_account.server')} type="url" bind:value={server} placeholder="https://sync.example.com"
				required disabled={action !== null} spellcheck={false} autocapitalize="off" />
			<Input label={t('sync_account.device_name')} bind:value={label} maxlength={64} required disabled={action !== null} />
			{@render accountFeedback()}
			<div class="actions">
				{#if action === 'sign-in'}
					<Button variant="ghost" onclick={() => void invoke('sync_account_cancel')}>{t('common.cancel')}</Button>
				{/if}
				<Button type="submit" disabled={action !== null || !server.trim() || !label.trim()}>{t('sync_account.sign_in')}</Button>
			</div>
		</form>
	</SettingsCard>
{/if}

<style>
	.auth-form { display: grid; gap: 12px; padding: 14px 0; max-width: 36rem; }
	.auth-form p, .confirmation p { margin: 0; }
	.actions, .footer-actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; }
	.footer-actions { padding: 8px 0; }
	.server-address { overflow-wrap: anywhere; }
	.scan-action { justify-self: start; }
	.wrap-control :global(button) { white-space: normal; }
	.feedback { margin: 0; padding: 9px 0; color: var(--color-text-secondary); font-size: 0.8125rem; }
	.error, .setting-description.error { color: var(--color-danger); }
	.confirmation {
		display: grid;
		gap: 10px;
		margin-bottom: 12px;
		padding: 12px;
		border-radius: var(--radius-card, 8px);
		background: var(--color-bg-elevated);
	}
	.enabled {
		padding: 3px 8px;
		border-radius: var(--radius-sm, 4px);
		background: color-mix(in srgb, var(--color-success) 12%, transparent);
		color: var(--color-success);
		font-size: 0.75rem;
	}
	.verification { font: 600 1.75rem var(--font-mono, monospace); letter-spacing: 0.15em; }
</style>
