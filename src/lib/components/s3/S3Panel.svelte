<script lang="ts">
	/**
	 * An S3 tab: connects its saved session (asking for the secret key or
	 * about a certificate when needed), then shows the full-pane file
	 * manager on its buckets and objects, with share links (presigned) in
	 * a file's menu. The connection lives in the backend; closing the tab
	 * ends it (see closeTab).
	 */
	import { onMount, tick, untrack } from 'svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { setS3Connection, setTabLink, type Tab } from '#lib/state/tabs.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { dropQueue } from '#lib/explorer/transfer-queue.svelte.js';
	import { s3Connect, s3Disconnect, s3Presign, s3TrustCert, S3_LOGIN_REFUSED, S3_SECRET_NEEDED, S3_UNTRUSTED_CERT } from '#lib/ipc/s3.js';
	import type { FileEntry } from '#lib/state/explorer.svelte.js';
	import type { MenuItem } from '#lib/components/db/DbMenu.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import DualPane from '#lib/components/explorer/DualPane.svelte';

	interface Props {
		tab: Tab;
		active: boolean;
	}

	let { tab, active }: Props = $props();

	type Stage =
		| { kind: 'connecting' }
		| { kind: 'secret'; error?: string }
		| { kind: 'cert'; fingerprint: string }
		| { kind: 'error'; message: string }
		| { kind: 'up' };

	let stage = $state<Stage>({ kind: 'connecting' });
	let secret = $state('');
	let secretEl: HTMLInputElement | undefined = $state();
	let seenReconnect = untrack(() => tab.reconnect);

	let where = $derived(tab.s3ConnectParams ? `${tab.s3ConnectParams.host}:${tab.s3ConnectParams.port}` : '');

	async function connect(typed?: string): Promise<void> {
		const params = tab.s3ConnectParams;
		if (!params) return;
		stage = { kind: 'connecting' };
		setTabLink(tab.id, 'reconnecting');
		const old = tab.s3ConnectionId;
		if (old) {
			setS3Connection(tab.id, undefined);
			dropQueue(old);
			await s3Disconnect(old).catch(() => {});
		}
		const id = crypto.randomUUID();
		try {
			await s3Connect(id, params.sessionId, typed);
			setS3Connection(tab.id, id);
			setTabLink(tab.id, undefined);
			secret = '';
			stage = { kind: 'up' };
		} catch (err) {
			setTabLink(tab.id, 'down');
			const msg = String(err);
			if (msg === S3_SECRET_NEEDED || msg.startsWith(S3_LOGIN_REFUSED)) {
				stage = { kind: 'secret', error: msg.startsWith(S3_LOGIN_REFUSED) ? msg.slice(S3_LOGIN_REFUSED.length) : undefined };
				await tick();
				secretEl?.focus();
			} else if (msg.startsWith(S3_UNTRUSTED_CERT)) {
				stage = { kind: 'cert', fingerprint: msg.slice(S3_UNTRUSTED_CERT.length) };
			} else {
				stage = { kind: 'error', message: msg };
			}
		}
	}

	async function trust(fingerprint: string): Promise<void> {
		const params = tab.s3ConnectParams;
		if (!params) return;
		try {
			await s3TrustCert(params.sessionId, fingerprint);
			await connect();
		} catch (err) {
			stage = { kind: 'error', message: String(err) };
		}
	}

	/** Copy a link anyone can download the file with, for a while. */
	async function shareLink(entry: FileEntry, seconds: number): Promise<void> {
		const id = tab.s3ConnectionId;
		if (!id) return;
		try {
			const link = await s3Presign(id, entry.path, seconds);
			await navigator.clipboard.writeText(link);
			addToast(t('s3.link_copied', { name: entry.name }), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	function fileItems(entry: FileEntry): MenuItem[] {
		return [
			{ label: '', separator: true },
			{ label: t('s3.share_hour'), action: () => void shareLink(entry, 3600) },
			{ label: t('s3.share_day'), action: () => void shareLink(entry, 86400) },
			{ label: t('s3.share_week'), action: () => void shareLink(entry, 7 * 86400) }
		];
	}

	onMount(() => {
		// Moved here from another window: the connection is up already.
		if (tab.s3ConnectionId) stage = { kind: 'up' };
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

{#if stage.kind === 'up' && tab.s3ConnectionId}
	<DualPane connectionId={tab.s3ConnectionId} {active} terminal={false} remoteFileItems={fileItems} />
{:else}
	<div class="connect">
		<div class="card">
			<div class="title">{tab.sessionName ?? tab.title}</div>
			<div class="where">{where}</div>
			{#if stage.kind === 'connecting'}
				<p class="note">{t('ftp.connecting')}</p>
			{:else if stage.kind === 'secret'}
				<form
					onsubmit={(e) => {
						e.preventDefault();
						if (secret) void connect(secret);
					}}
				>
					<label class="field">
						<span>{t('s3.secret_key')}</span>
						<input bind:this={secretEl} type="password" bind:value={secret} autocomplete="off" />
					</label>
					{#if stage.error}<p class="note error">{stage.error}</p>{/if}
					<div class="actions">
						<Button type="submit" disabled={!secret}>{t('ftp.connect')}</Button>
					</div>
				</form>
			{:else if stage.kind === 'cert'}
				<p class="note">{t('s3.cert_untrusted')}</p>
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
	.connect {
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
