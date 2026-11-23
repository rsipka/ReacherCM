<script lang="ts">
	/**
	 * An FTP tab: connects its saved session, asking for a password or about
	 * a certificate when the server needs it, then shows the full-pane file
	 * manager on it. The connection lives in the backend; closing the tab
	 * ends it (see closeTab).
	 */
	import { onMount, tick, untrack } from 'svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { setFtpConnection, setTabLink, type Tab } from '#lib/state/tabs.svelte.js';
	import { dropQueue } from '#lib/explorer/transfer-queue.svelte.js';
	import { ftpConnect, ftpDisconnect, ftpTrustCert, FTP_PASSWORD_NEEDED, FTP_UNTRUSTED_CERT } from '#lib/ipc/ftp.js';
	import Button from '#lib/components/shared/Button.svelte';
	import DualPane from './DualPane.svelte';

	interface Props {
		tab: Tab;
		active: boolean;
	}

	let { tab, active }: Props = $props();

	type Stage =
		| { kind: 'connecting' }
		| { kind: 'password'; error?: string }
		| { kind: 'cert'; fingerprint: string }
		| { kind: 'error'; message: string }
		| { kind: 'up' };

	let stage = $state<Stage>({ kind: 'connecting' });
	let password = $state('');
	let passwordEl: HTMLInputElement | undefined = $state();
	/** What a reconnect bump was last seen at. */
	let seenReconnect = untrack(() => tab.reconnect);

	let where = $derived(tab.ftpConnectParams ? `${tab.ftpConnectParams.username ? tab.ftpConnectParams.username + '@' : ''}${tab.ftpConnectParams.host}:${tab.ftpConnectParams.port}` : '');

	async function connect(typed?: string): Promise<void> {
		const params = tab.ftpConnectParams;
		if (!params) return;
		stage = { kind: 'connecting' };
		setTabLink(tab.id, 'reconnecting');
		const old = tab.ftpConnectionId;
		if (old) {
			setFtpConnection(tab.id, undefined);
			dropQueue(old);
			await ftpDisconnect(old).catch(() => {});
		}
		const id = crypto.randomUUID();
		try {
			await ftpConnect(id, params.sessionId, typed);
			setFtpConnection(tab.id, id);
			setTabLink(tab.id, undefined);
			password = '';
			stage = { kind: 'up' };
		} catch (err) {
			setTabLink(tab.id, 'down');
			const msg = String(err);
			if (msg === FTP_PASSWORD_NEEDED) {
				stage = { kind: 'password', error: typed ? t('ftp.password_wrong') : undefined };
				await tick();
				passwordEl?.focus();
			} else if (msg.startsWith(FTP_UNTRUSTED_CERT)) {
				stage = { kind: 'cert', fingerprint: msg.slice(FTP_UNTRUSTED_CERT.length) };
			} else if (typed && /\b530\b/.test(msg)) {
				// Login refused: the password typed was wrong.
				stage = { kind: 'password', error: msg };
			} else {
				stage = { kind: 'error', message: msg };
			}
		}
	}

	async function trust(fingerprint: string): Promise<void> {
		const params = tab.ftpConnectParams;
		if (!params) return;
		try {
			await ftpTrustCert(params.sessionId, fingerprint);
			await connect();
		} catch (err) {
			stage = { kind: 'error', message: String(err) };
		}
	}

	onMount(() => {
		// Moved here from another window: the connection is up already.
		if (tab.ftpConnectionId) stage = { kind: 'up' };
		else void connect();
	});

	// The tab menu's Reconnect.
	$effect(() => {
		const bump = tab.reconnect;
		if (bump === seenReconnect) return;
		seenReconnect = bump;
		void connect();
	});
</script>

{#if stage.kind === 'up' && tab.ftpConnectionId}
	<DualPane connectionId={tab.ftpConnectionId} {active} terminal={false} />
{:else}
	<div class="ftp-panel">
		<div class="card">
			<div class="title">{tab.sessionName ?? tab.title}</div>
			<div class="where">{where}</div>
			{#if stage.kind === 'connecting'}
				<p class="note">{t('ftp.connecting')}</p>
			{:else if stage.kind === 'password'}
				<form
					onsubmit={(e) => {
						e.preventDefault();
						if (password) void connect(password);
					}}
				>
					<label class="field">
						<span>{t('session.password')}</span>
						<input bind:this={passwordEl} type="password" bind:value={password} autocomplete="off" />
					</label>
					{#if stage.error}<p class="note error">{stage.error}</p>{/if}
					<div class="actions">
						<Button type="submit" disabled={!password}>{t('ftp.connect')}</Button>
					</div>
				</form>
			{:else if stage.kind === 'cert'}
				<p class="note">{t('ftp.cert_untrusted')}</p>
				<code class="fp">{stage.fingerprint}</code>
				<p class="note small">{t('ftp.cert_hint')}</p>
				<div class="actions">
					<Button variant="secondary" onclick={() => (stage = { kind: 'error', message: t('ftp.cert_refused') })}>{t('common.cancel')}</Button>
					<Button onclick={() => void trust((stage as { fingerprint: string }).fingerprint)}>{t('ftp.cert_trust')}</Button>
				</div>
			{:else if stage.kind === 'error'}
				<p class="note error">{stage.message}</p>
				<div class="actions">
					<Button onclick={() => void connect()}>{t('ftp.retry')}</Button>
				</div>
			{/if}
		</div>
	</div>
{/if}

<style>
	.ftp-panel {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-bg-primary);
		padding: 16px;
	}
	.card {
		width: min(460px, 100%);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: 10px;
		padding: 20px;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.title {
		font-weight: 600;
		color: var(--color-text-primary);
	}
	.where {
		font-size: 12px;
		color: var(--color-text-tertiary);
		word-break: break-all;
	}
	.note {
		margin: 0;
		font-size: 13px;
		color: var(--color-text-secondary);
	}
	.note.small {
		font-size: 12px;
	}
	.note.error {
		color: var(--color-danger);
		white-space: pre-wrap;
	}
	.fp {
		font-size: 11px;
		word-break: break-all;
		padding: 8px;
		border-radius: 6px;
		background: var(--color-surface-sunken);
		color: var(--color-text-primary);
	}
	form {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.field input {
		padding: 7px 9px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}
</style>
