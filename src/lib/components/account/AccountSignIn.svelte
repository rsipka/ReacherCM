<script lang="ts">
	import { onDestroy } from 'svelte';
	import { Channel, invoke } from '@tauri-apps/api/core';
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { scanLinkCode } from '#lib/state/link-scan.js';
	import { bindPreferenceAccount, type SyncAccount } from '#lib/state/preference-sync.svelte.js';
	import { syncAccountErrorKey } from '#lib/sync/account.js';

	interface Props {
		mobile: boolean;
		busy?: boolean;
		server?: string;
		label?: string;
		initialError?: string;
		initialMessage?: string;
		onSignedIn?: () => void;
	}

	let {
		mobile, busy = $bindable(false), server = $bindable(''), label = $bindable(''),
		initialError = '', initialMessage = '', onSignedIn
	}: Props = $props();
	let pairingCode = $state('');
	let pairingServer = $state('');
	let verificationCode = $state('');
	let action = $state<'sign-in' | 'scan' | 'pair' | null>(null);
	let error = $state<string | null>(null);
	let displayedError = $derived(error ?? initialError);

	$effect(() => { busy = action !== null; });

	function cancel(): void {
		void invoke('sync_account_cancel').catch(() => {});
	}

	onDestroy(() => {
		if (action === 'sign-in' || action === 'pair') cancel();
		busy = false;
	});

	async function signIn(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		action = 'sign-in';
		error = '';
		try {
			const signedIn = await invoke<SyncAccount>('sync_account_sign_in', { server, label });
			server = signedIn.server;
			bindPreferenceAccount(signedIn);
			onSignedIn?.();
		} catch (cause) {
			error = syncAccountErrorKey(cause);
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
			error = typeof cause === 'string' ? syncAccountErrorKey(cause) : 'sync_account.pair_camera';
		} finally {
			action = null;
		}
	}

	async function pair(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		error = '';
		if (!pairingServer) {
			action = 'scan';
			try {
				pairingServer = await invoke<string>('sync_account_pair_origin', { code: pairingCode });
			} catch (cause) {
				error = syncAccountErrorKey(cause);
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
			onSignedIn?.();
		} catch (cause) {
			error = syncAccountErrorKey(cause);
		} finally {
			verificationCode = '';
			pairingCode = '';
			pairingServer = '';
			action = null;
		}
	}
</script>

{#snippet accountFeedback()}
	{#if action === 'sign-in'}
		<p class="description" role="status">{t('sync_account.browser_pending')}</p>
	{:else if error === null && initialMessage}
		<p class="description" role="status">{t(initialMessage)}</p>
	{/if}
	{#if displayedError}<p class="description error" role="alert">{t(displayedError)}</p>{/if}
{/snippet}

{#if mobile}
	<form class="auth-form" onsubmit={pair}>
		{#if pairingServer}
			<div class="server-info">
				<span class="server-label">{t('sync_account.server')}</span>
				<span class="description server-address">{pairingServer}</span>
			</div>
			<Input label={t('sync_account.device_name')} bind:value={label} maxlength={64} required disabled={action !== null} />
		{:else}
			<p class="description">{t('sync_account.pair_help')}</p>
			<div class="scan-action">
				<Button variant="secondary" disabled={action !== null} onclick={scanPairing}>{t('sync_account.pair_scan')}</Button>
			</div>
			<Input label={t('sync_account.pair_code')} bind:value={pairingCode} maxlength={2048}
				autocomplete="off" spellcheck={false} autocapitalize="off" required disabled={action !== null} />
		{/if}
		{#if verificationCode}
			<div class="confirmation" role="status">
				<p class="description">{t('sync_account.pair_match')}</p>
				<strong class="verification">{verificationCode}</strong>
			</div>
		{/if}
		{@render accountFeedback()}
		<div class="actions">
			{#if action === 'pair' || pairingServer}
				<Button variant="ghost" onclick={() => {
					if (action === 'pair') cancel();
					else { pairingServer = ''; pairingCode = ''; }
				}}>{t('common.cancel')}</Button>
			{/if}
			<Button type="submit" disabled={action !== null || !(pairingServer ? label.trim() : pairingCode.trim())}>
				{t(pairingServer ? 'sync_account.pair_start' : 'authprompt.continue')}
			</Button>
		</div>
	</form>
{:else}
	<form class="auth-form" onsubmit={signIn}>
		<p class="description">{t('sync_account.sign_in_help')}</p>
		<Input label={t('sync_account.server')} type="url" bind:value={server} placeholder="https://sync.example.com"
			required disabled={action !== null} spellcheck={false} autocapitalize="off" />
		<Input label={t('sync_account.device_name')} bind:value={label} maxlength={64} required disabled={action !== null} />
		{@render accountFeedback()}
		<div class="actions">
			{#if action === 'sign-in'}
			<Button variant="ghost" onclick={cancel}>{t('common.cancel')}</Button>
			{/if}
			<Button type="submit" disabled={action !== null || !server.trim() || !label.trim()}>{t('sync_account.sign_in')}</Button>
		</div>
	</form>
{/if}

<style>
	.auth-form { display: grid; gap: 12px; padding: 14px 0; max-width: 36rem; }
	.auth-form p, .confirmation p { margin: 0; }
	.actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; }
	.server-info { display: grid; gap: 4px; min-width: 0; }
	.server-label { font-size: var(--text-sm); color: var(--color-text-primary); }
	.description { color: var(--color-text-secondary); font-size: 0.8125rem; }
	.server-address { overflow-wrap: anywhere; }
	.scan-action { justify-self: start; }
	.error { color: var(--color-danger); }
	.confirmation {
		display: grid;
		gap: 10px;
		margin-bottom: 12px;
		padding: 12px;
		border-radius: var(--radius-card, 8px);
		background: var(--color-bg-elevated);
	}
	.verification { font: 600 1.75rem var(--font-mono, monospace); letter-spacing: 0.15em; }
</style>
