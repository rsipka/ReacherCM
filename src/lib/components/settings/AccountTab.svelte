<script lang="ts">
	import { onMount } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { t } from '#lib/state/i18n.svelte.js';

	interface Account {
		server: string;
		username: string;
		deviceId: string;
	}

	let account = $state<Account | null>(null);
	let server = $state('');
	let label = $state('');
	let action = $state<'load' | 'sign-in' | 'check' | 'sign-out' | null>('load');
	let error = $state('');
	let message = $state('');

	onMount(() => { void load(); });

	async function load(): Promise<void> {
		try {
			account = await invoke<Account | null>('sync_account_status');
			if (account) server = account.server;
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
			account = await invoke<Account>('sync_account_sign_in', { server, label });
			server = account.server;
			message = '';
		} catch (cause) {
			error = errorKey(cause);
			message = '';
		} finally {
			action = null;
		}
	}

	async function check(): Promise<void> {
		action = 'check';
		error = '';
		message = '';
		try {
			account = await invoke<Account>('sync_account_check');
			message = 'sync_account.connected';
		} catch (cause) {
			error = errorKey(cause);
			if (error === 'sync_account.error_revoked') account = null;
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
			if (!revoked) message = 'sync_account.signed_out_locally';
		} catch (cause) {
			error = errorKey(cause);
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
	{:else if action === 'load'}
		<p role="status">{t('sync_account.loading')}</p>
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
	input {
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
	.error { color: var(--color-danger); }
</style>
