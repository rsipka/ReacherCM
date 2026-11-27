<script lang="ts">
	import { onMount } from 'svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import { getPersonalSync, setPersonalSync, testSyncBackend, type SyncProbe } from '#lib/ipc/vault.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import AccountVaultLocation from './AccountVaultLocation.svelte';
	import TursoSync from './TursoSync.svelte';

	/** Sync location: where the personal vault syncs. One choice in the
	 *  dropdown, and only that choice's settings under it. */
	let { onOpenVaultLinks }: { onOpenVaultLinks: () => void } = $props();

	type Kind = 'account' | 'folder' | 'smb' | 'sftp' | 'git' | 's3' | 'turso';

	let kind = $state<Kind>('account');
	let folderPath = $state('');
	let sftpHost = $state('');
	let sftpPort = $state('22');
	let sftpUser = $state('');
	let sftpFolder = $state('reach');
	let sftpAuth = $state<'password' | 'key'>('password');
	let sftpPassword = $state('');
	let sftpKey = $state('');
	let sftpPassphrase = $state('');
	let sftpHostKey = $state('');
	let smbServer = $state('');
	let smbPort = $state('445');
	let smbShare = $state('');
	let smbFolder = $state('');
	let smbUser = $state('');
	let smbPassword = $state('');
	let gitRemote = $state('');
	let gitBranch = $state('reach-vault');
	let gitToken = $state('');
	let s3Endpoint = $state('');
	let s3Bucket = $state('');
	let s3Folder = $state('reach');
	let s3Region = $state('');
	let s3AccessKey = $state('');
	let s3SecretKey = $state('');

	let active = $state<string | null>(null);
	let activeKind = $derived(active ? kindOf(active) : null);
	let probe = $state<SyncProbe | null>(null);
	let busy = $state<'test' | 'use' | null>(null);

	onMount(() => {
		void loadActive(true);
	});

	/** Read the location in use; on the first read, open its choice. */
	async function loadActive(first = false) {
		try {
			const [url, token] = await getPersonalSync();
			active = url;
			if (url && first) prefill(url, token ?? '');
		} catch {
			// Not unlocked yet: nothing to show.
		}
	}

	/** Which choice a sync URL belongs to. */
	function kindOf(url: string): Kind {
		const lower = url.toLowerCase();
		if (url.startsWith('reachercm-sync:') || url.startsWith('reachercm-org:')) return 'account';
		if (url.startsWith('folder:')) return 'folder';
		if (lower.startsWith('sftp://')) return 'sftp';
		if (lower.startsWith('smb://')) return 'smb';
		if (/^s3(\+http)?:\/\//i.test(url)) return 's3';
		if (url.startsWith('git:')) return 'git';
		// libsql://, https://… : a Turso database.
		return 'turso';
	}

	const KIND_LABEL: Record<Kind, string> = {
		account: 'sync.kind_account',
		folder: 'sync.kind_folder',
		smb: 'sync.kind_smb',
		sftp: 'sync.kind_sftp',
		git: 'sync.kind_git',
		s3: 'sync.kind_s3',
		turso: 'sync.kind_turso'
	};
	const KINDS = Object.keys(KIND_LABEL) as Kind[];

	function optionLabel(k: Kind): string {
		const name = t(KIND_LABEL[k]);
		return k === activeKind ? t('sync.kind_in_use', { name }) : name;
	}

	/** Fill the form from the location in use. */
	function prefill(url: string, token: string) {
		kind = kindOf(url);
		if (url.startsWith('folder:')) {
			kind = 'folder';
			folderPath = url.slice('folder:'.length);
		} else if (url.toLowerCase().startsWith('sftp://')) {
			try {
				const u = new URL(url);
				kind = 'sftp';
				sftpHost = u.hostname.replace(/^\[|\]$/g, '');
				sftpPort = u.port || '22';
				sftpUser = decodeURIComponent(u.username);
				const path = decodeURIComponent(u.pathname);
				sftpFolder = path.startsWith('/~/') ? path.slice(3) : path;
				sftpHostKey = u.searchParams.get('hostkey') ?? '';
				const secret = token.trim().startsWith('{') ? JSON.parse(token) : { password: token };
				if (secret.key) {
					sftpAuth = 'key';
					sftpKey = secret.key;
					sftpPassphrase = secret.passphrase ?? '';
				} else {
					sftpAuth = 'password';
					sftpPassword = secret.password ?? '';
				}
			} catch {
				// Leave the form empty.
			}
		} else if (url.toLowerCase().startsWith('smb://')) {
			try {
				const u = new URL(url);
				kind = 'smb';
				smbServer = u.hostname.replace(/^\[|\]$/g, '');
				smbPort = u.port || '445';
				smbUser = decodeURIComponent(u.username).replace(';', '\\');
				const parts = decodeURIComponent(u.pathname).split('/').filter(Boolean);
				smbShare = parts[0] ?? '';
				smbFolder = parts.slice(1).join('/');
				smbPassword = token;
			} catch {
				// Leave the form empty.
			}
		} else if (/^s3(\+http)?:\/\//i.test(url)) {
			try {
				const plain = /^s3\+http:/i.test(url);
				const u = new URL(url.replace(/^s3(\+http)?:/i, plain ? 'http:' : 'https:'));
				kind = 's3';
				s3Endpoint = plain ? `http://${u.host}` : u.host;
				const parts = u.pathname.split('/').filter(Boolean).map(decodeURIComponent);
				s3Bucket = parts[0] ?? '';
				s3Folder = parts.slice(1).join('/');
				s3Region = u.searchParams.get('region') ?? '';
				const secret = JSON.parse(token || '{}');
				s3AccessKey = secret.access_key ?? '';
				s3SecretKey = secret.secret_key ?? '';
			} catch {
				// Leave the form empty.
			}
		} else if (url.startsWith('git:')) {
			kind = 'git';
			const rest = url.slice('git:'.length);
			const hash = rest.lastIndexOf('#');
			gitRemote = hash >= 0 ? rest.slice(0, hash) : rest;
			gitBranch = hash >= 0 ? rest.slice(hash + 1) : 'reach-vault';
			gitToken = token;
		}
	}

	/** The sync URL and token for what the form says. */
	function location(): { url: string; token: string } | null {
		if (kind === 'account' || kind === 'turso') return null;
		if (kind === 'folder') {
			const path = folderPath.trim();
			return path ? { url: `folder:${path}`, token: '' } : null;
		}
		if (kind === 'smb') {
			const server = smbServer.trim();
			const share = smbShare.trim().replace(/^[\\/]+|[\\/]+$/g, '');
			const user = smbUser.trim();
			if (!server || !share || !user) return null;
			const hostPart = server.includes(':') ? `[${server}]` : server;
			// DOMAIN\user is written DOMAIN;user in the address.
			const userPart = user
				.split('\\')
				.map(encodeURIComponent)
				.join(';');
			const folder = smbFolder
				.trim()
				.split(/[\\/]/)
				.filter(Boolean)
				.map(encodeURIComponent)
				.join('/');
			const port = Number(smbPort) || 445;
			return {
				url: `smb://${userPart}@${hostPart}:${port}/${encodeURIComponent(share)}${folder ? '/' + folder : ''}`,
				token: smbPassword
			};
		}
		if (kind === 'sftp') {
			const host = sftpHost.trim();
			const user = sftpUser.trim();
			if (!host || !user) return null;
			const hostPart = host.includes(':') ? `[${host}]` : host;
			const folder = sftpFolder.trim();
			const segments = (f: string) => f.split('/').map(encodeURIComponent).join('/');
			const path = folder.startsWith('/') ? segments(folder) : `/~/${segments(folder)}`;
			const query = sftpHostKey ? `?hostkey=${encodeURIComponent(sftpHostKey)}` : '';
			const port = Number(sftpPort) || 22;
			const secret =
				sftpAuth === 'key'
					? { key: sftpKey, passphrase: sftpPassphrase || undefined }
					: { password: sftpPassword };
			return {
				url: `sftp://${encodeURIComponent(user)}@${hostPart}:${port}${path}${query}`,
				token: JSON.stringify(secret)
			};
		}
		if (kind === 's3') {
			// "https://host", "http://localhost:9000" or just "host[:port]".
			const endpoint = s3Endpoint.trim().replace(/\/+$/, '');
			const bucket = s3Bucket.trim();
			if (!endpoint || !bucket || !s3AccessKey.trim() || !s3SecretKey) return null;
			const plain = /^http:\/\//i.test(endpoint);
			const host = endpoint.replace(/^https?:\/\//i, '');
			const folder = s3Folder
				.trim()
				.split('/')
				.filter(Boolean)
				.map(encodeURIComponent)
				.join('/');
			const region = s3Region.trim();
			return {
				url: `${plain ? 's3+http' : 's3'}://${host}/${encodeURIComponent(bucket)}${folder ? '/' + folder : ''}${region ? `?region=${encodeURIComponent(region)}` : ''}`,
				token: JSON.stringify({ access_key: s3AccessKey.trim(), secret_key: s3SecretKey })
			};
		}
		const remote = gitRemote.trim();
		if (!remote) return null;
		const branch = gitBranch.trim() || 'reach-vault';
		return { url: `git:${remote}#${branch}`, token: gitToken.trim() };
	}

	let ready = $derived(location() !== null);

	function changed() {
		probe = null;
	}

	async function test(): Promise<SyncProbe | null> {
		const loc = location();
		if (!loc) return null;
		busy = 'test';
		try {
			probe = await testSyncBackend(loc.url, loc.token);
			return probe;
		} catch (e) {
			probe = { ok: false, hostKey: null, holds: null, error: e instanceof Error ? e.message : String(e) };
			return probe;
		} finally {
			busy = null;
		}
	}

	async function trustHostKey() {
		if (!probe?.hostKey) return;
		sftpHostKey = probe.hostKey;
		await test();
	}

	async function use() {
		const result = probe ?? (await test());
		const loc = location();
		if (!result?.ok || result.holds === 'other' || !loc) return;
		busy = 'use';
		try {
			await setPersonalSync(loc.url, loc.token);
			active = loc.url;
			addToast(t('sync.backend_switched_toast'), 'success');
		} catch (e) {
			addToast(e instanceof Error ? e.message : String(e), 'error');
		} finally {
			busy = null;
		}
	}

	/** The location in use, without anything secret. */
	function describe(url: string): string {
		if (url.startsWith('folder:')) return url.slice('folder:'.length);
		if (url.startsWith('git:')) return url.slice('git:'.length);
		if (/^s3(\+http)?:\/\//i.test(url)) return url.split('?')[0].replace(/^s3\+http:/i, 's3:');
		if (url.toLowerCase().startsWith('smb://')) {
			try {
				const u = new URL(url);
				return `\\\\${u.hostname}${decodeURIComponent(u.pathname).replace(/\//g, '\\')}`;
			} catch {
				return url;
			}
		}
		return url.split('?')[0];
	}
</script>

<div class="section location">
	<p class="section-desc">{t('sync.backend_desc')}</p>

	{#if active}
		<p class="section-desc active">
			{t('sync.backend_active', { where: activeKind === 'account' ? t('sync.kind_account') : describe(active) })}
		</p>
	{/if}

	<div class="form-field">
		<label class="field-label" for="sync-backend-kind">{t('sync.backend_kind')}</label>
		<select
			id="sync-backend-kind"
			class="field"
			bind:value={kind}
			onchange={changed}
			disabled={busy !== null}
		>
			{#each KINDS as k (k)}
				<option value={k}>{optionLabel(k)}</option>
			{/each}
		</select>
	</div>

	{#if kind === 'account'}
		<AccountVaultLocation {onOpenVaultLinks} onChanged={() => void loadActive()} />
	{:else if kind === 'turso'}
		<TursoSync personalOnTurso={activeKind === 'turso'} onChanged={() => void loadActive()} />
	{:else if kind === 'folder'}
		<div class="form-field">
			<Input
				label={t('sync.folder_path')}
				placeholder={'\\\\nas\\share\\reach  ·  /mnt/nas/reach'}
				bind:value={folderPath}
				oninput={changed}
			/>
		</div>
		<p class="section-desc">{t('sync.folder_hint')}</p>
	{:else if kind === 'smb'}
		<div class="row">
			<div class="grow"><Input label={t('sync.smb_server')} placeholder="nas.local" bind:value={smbServer} oninput={changed} /></div>
			<div class="port"><Input label={t('sync.sftp_port')} placeholder="445" bind:value={smbPort} oninput={changed} /></div>
		</div>
		<div class="form-field">
			<Input label={t('sync.smb_share')} placeholder="backup" bind:value={smbShare} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.smb_folder')} placeholder="reach" bind:value={smbFolder} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.smb_user')} placeholder="DOMAIN\user" bind:value={smbUser} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.sftp_auth_password')} type="password" bind:value={smbPassword} oninput={changed} />
		</div>
		<p class="section-desc">{t('sync.smb_hint')}</p>
	{:else if kind === 'sftp'}
		<div class="row">
			<div class="grow"><Input label={t('sync.sftp_host')} placeholder="files.example.com" bind:value={sftpHost} oninput={changed} /></div>
			<div class="port"><Input label={t('sync.sftp_port')} placeholder="22" bind:value={sftpPort} oninput={changed} /></div>
		</div>
		<div class="form-field">
			<Input label={t('sync.sftp_user')} bind:value={sftpUser} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.sftp_folder')} placeholder="reach" bind:value={sftpFolder} oninput={changed} />
		</div>
		<div class="form-field">
			<label class="field-label" for="sync-sftp-auth">{t('sync.sftp_auth')}</label>
			<select id="sync-sftp-auth" class="field" bind:value={sftpAuth} onchange={changed}>
				<option value="password">{t('sync.sftp_auth_password')}</option>
				<option value="key">{t('sync.sftp_auth_key')}</option>
			</select>
		</div>
		{#if sftpAuth === 'password'}
			<div class="form-field">
				<Input label={t('sync.sftp_auth_password')} type="password" bind:value={sftpPassword} oninput={changed} />
			</div>
		{:else}
			<textarea
				class="field key-text"
				bind:value={sftpKey}
				oninput={changed}
				rows="4"
				spellcheck="false"
				autocomplete="off"
				aria-label={t('sync.sftp_auth_key')}
				placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
			></textarea>
			<div class="form-field">
				<Input label={t('sync.sftp_passphrase')} type="password" bind:value={sftpPassphrase} oninput={changed} />
			</div>
		{/if}
		{#if sftpHostKey}
			<p class="section-desc mono">{t('sync.host_key_trusted', { key: sftpHostKey })}</p>
		{/if}
	{:else if kind === 's3'}
		<div class="form-field">
			<Input label={t('sync.s3_endpoint')} placeholder="s3.eu-central-1.amazonaws.com" bind:value={s3Endpoint} oninput={changed} />
		</div>
		<div class="row">
			<div class="grow"><Input label={t('sync.s3_bucket')} placeholder="my-vault-bucket" bind:value={s3Bucket} oninput={changed} /></div>
			<div class="grow"><Input label={t('sync.s3_region')} placeholder="auto" bind:value={s3Region} oninput={changed} /></div>
		</div>
		<div class="form-field">
			<Input label={t('sync.s3_folder')} placeholder="reach" bind:value={s3Folder} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.s3_access_key')} bind:value={s3AccessKey} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.s3_secret_key')} type="password" bind:value={s3SecretKey} oninput={changed} />
		</div>
		<p class="section-desc">{t('sync.s3_hint')}</p>
	{:else}
		<div class="form-field">
			<Input label={t('sync.git_remote')} placeholder="git@github.com:me/reach-vault.git" bind:value={gitRemote} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.git_branch')} placeholder="reach-vault" bind:value={gitBranch} oninput={changed} />
		</div>
		<div class="form-field">
			<Input label={t('sync.git_token')} type="password" bind:value={gitToken} oninput={changed} />
		</div>
		<p class="section-desc">{t('sync.git_hint')}</p>
	{/if}

	{#if kind !== 'account' && kind !== 'turso'}
	{#if probe}
		{#if probe.hostKey}
			<div class="notice">
				<p>{t('sync.host_key_confirm', { key: probe.hostKey })}</p>
				<div class="buttons">
					<Button variant="secondary" size="sm" onclick={trustHostKey} disabled={busy !== null}>
						{t('sync.host_key_trust')}
					</Button>
				</div>
			</div>
		{:else if probe.error}
			<p class="form-error">{probe.error}</p>
		{:else if probe.holds === 'other'}
			<p class="form-error">{t('sync.probe_other')}</p>
		{:else if probe.holds === 'this'}
			<p class="section-desc ok">{t('sync.probe_this')}</p>
		{:else}
			<p class="section-desc ok">{t('sync.probe_empty')}</p>
		{/if}
	{/if}

	<div class="action-row buttons">
		<Button variant="secondary" size="sm" onclick={test} disabled={!ready || busy !== null}>
			{busy === 'test' ? t('sync.testing') : t('sync.test_connection')}
		</Button>
		<Button
			variant="primary"
			size="sm"
			onclick={use}
			disabled={!ready || busy !== null || (probe !== null && (!probe.ok || probe.holds === 'other'))}
		>
			{busy === 'use' ? t('sync.switching') : t('sync.use_backend')}
		</Button>
	</div>
	<p class="section-desc">{t('sync.backend_note')}</p>
	{/if}
</div>

<style>
	.location {
		padding: 12px 0;
	}

	.section-desc {
		margin: 0;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.section-desc.active {
		color: var(--color-text-primary);
	}

	.section-desc.ok {
		color: var(--color-success);
	}

	.mono {
		font-family: var(--font-mono);
		word-break: break-all;
	}

	.form-field {
		width: 100%;
	}

	.field-label {
		display: block;
		margin-bottom: 4px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.field {
		width: 100%;
		min-width: 0;
		box-sizing: border-box;
		padding: 7px 10px;
		border-radius: var(--radius-btn);
		border: 1px solid var(--color-border-strong);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-family: inherit;
		font-size: 0.8125rem;
	}

	.field:focus {
		outline: none;
		border-color: var(--color-accent);
	}

	.key-text {
		font-family: var(--font-mono);
		font-size: 0.6875rem;
		resize: vertical;
	}

	.row {
		display: flex;
		gap: 8px;
	}

	.grow {
		flex: 1;
		min-width: 0;
	}

	.port {
		width: 80px;
		flex-shrink: 0;
	}

	.buttons {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
	}

	.notice {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 10px 12px;
		font-size: 0.75rem;
		background: rgba(255, 214, 10, 0.08);
		border: 1px solid rgba(255, 214, 10, 0.3);
		border-radius: 8px;
		word-break: break-all;
	}

	.notice p {
		margin: 0;
	}

	.form-error {
		margin: 0;
		font-size: 0.75rem;
		color: var(--color-danger);
	}
</style>
