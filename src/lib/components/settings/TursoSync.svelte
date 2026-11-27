<script lang="ts">
	import { onMount } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import { getSettings, saveSettings } from '#lib/state/vault.svelte.js';
	import type { AppSettings, SyncStatus } from '#lib/ipc/vault.js';
	import {
		createTursoDatabase,
		createTursoDatabaseToken,
		setPersonalSync,
		acceptInvite,
		listTursoPersonalDatabases,
		syncStatus
	} from '#lib/ipc/vault.js';
	import { vaultState } from '#lib/state/vault.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSettings as getLocalSettings, updateSetting } from '#lib/state/settings.svelte.js';

	/** Everything Turso, shown under the Turso choice of Sync location.
	 *  `personalOnTurso`: the personal vault syncs to Turso now. */
	let { personalOnTurso, onChanged }: { personalOnTurso: boolean; onChanged: () => void } = $props();

	let loading = $state(true);
	let saving = $state(false);
	let tursoOrg = $state('');
	let tursoApiToken = $state('');
	let tursoGroup = $state('default');
	let personalDbUrl = $state('');
	let personalDbToken = $state('');
	let savedCredentials = $state(false);
	let error = $state('');
	let statuses = $state<SyncStatus[]>([]);

	let publicKey = $derived(vaultState.publicKey);
	let userUuid = $derived(vaultState.userUuid);
	// Shared vaults kept on Turso, whatever the personal vault uses.
	let sharedOnTurso = $derived(statuses.filter((s) => s.backend === 'turso' && !s.name.startsWith('__')));
	let inUse = $derived(personalOnTurso || sharedOnTurso.length > 0);

	// Another device's personal database is in the organization: the next
	// Save makes a separate one anyway.
	let createAnyway = $state(false);

	let inviteSyncUrl = $state('');
	let inviteToken = $state('');
	let acceptingInvite = $state(false);

	onMount(() => {
		void loadSettings();
		void refreshStatuses();
		const unlisten = listen('vault-sync-status', () => void refreshStatuses());
		return () => void unlisten.then((u) => u());
	});

	async function refreshStatuses() {
		try {
			statuses = await syncStatus();
		} catch {
			statuses = [];
		}
	}

	async function loadSettings() {
		loading = true;
		try {
			const settings = await getSettings();
			tursoOrg = settings.tursoOrg ?? '';
			tursoApiToken = settings.tursoApiToken ?? '';
			tursoGroup = settings.tursoGroup ?? 'default';
			personalDbUrl = settings.personalDbUrl ?? '';
			personalDbToken = settings.personalDbToken ?? '';
			savedCredentials = !!(tursoOrg && tursoApiToken);

			// Turso details an older setup wizard kept for later.
			const local = getLocalSettings();
			if (!tursoOrg && local.pendingTursoOrg) {
				tursoOrg = local.pendingTursoOrg;
				updateSetting('pendingTursoOrg', '');
			}
			if (!tursoApiToken && local.pendingTursoApiToken) {
				tursoApiToken = local.pendingTursoApiToken;
				updateSetting('pendingTursoApiToken', '');
			}
		} catch {
			// Default values
		}
		loading = false;
	}

	async function handleSave() {
		saving = true;
		error = '';

		try {
			let newPersonalDbUrl = personalDbUrl;
			let newPersonalDbToken = personalDbToken;

			// If org/token set but no personal DB yet, we need to:
			// 1. FIRST save the Turso credentials so backend can use them
			// 2. THEN create the database using those credentials
			// 3. THEN update settings with the new DB URL/token
			if (tursoOrg && tursoApiToken && !personalDbUrl) {
				addToast(t('sync.saving_credentials'), 'info');
				const initialSettings: AppSettings = {
					tursoOrg: tursoOrg,
					tursoApiToken: tursoApiToken,
					tursoGroup: tursoGroup || 'default',
					syncEnabled: false
				};
				await saveSettings(initialSettings);

				// Use this identity's database if it is already there, or
				// create it (backend reads the saved credentials). A database
				// made by another device's identity cannot be opened from
				// here: that device has to link this one.
				const dbName = `reach-personal-${userUuid?.substring(0, 8) || Date.now()}`;
				const existing = await listTursoPersonalDatabases();
				let dbInfo = existing.find((d) => d.name === dbName);
				if (dbInfo) {
					addToast(t('sync.reusing_database'), 'info');
				} else if (existing.length > 0 && !createAnyway) {
					createAnyway = true;
					error = t('sync.personal_db_exists', { names: existing.map((d) => d.name).join(', ') });
					saving = false;
					return;
				} else {
					addToast(t('sync.creating_database'), 'info');
					dbInfo = await createTursoDatabase(dbName);
				}
				const token = await createTursoDatabaseToken(dbInfo.name);

				newPersonalDbUrl = `libsql://${dbInfo.hostname}`;
				newPersonalDbToken = token;
				personalDbUrl = newPersonalDbUrl;
				personalDbToken = token;

				addToast(t('sync.database_created'), 'success');
			}

			const settings: AppSettings = {
				tursoOrg: tursoOrg || undefined,
				tursoApiToken: tursoApiToken || undefined,
				tursoGroup: tursoGroup || 'default',
				personalDbUrl: newPersonalDbUrl || undefined,
				personalDbToken: newPersonalDbToken || undefined,
				syncEnabled: !!(tursoOrg && tursoApiToken && newPersonalDbUrl)
			};
			await saveSettings(settings);
			savedCredentials = !!(tursoOrg && tursoApiToken);

			// The personal vault (all personal data) syncs to Turso from now on.
			if (newPersonalDbUrl && newPersonalDbToken) {
				await setPersonalSync(newPersonalDbUrl, newPersonalDbToken);
				addToast(t('sync.cloud_sync_enabled'), 'success');
				onChanged();
			} else {
				addToast(t('sync.settings_saved'), 'success');
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
		saving = false;
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
		addToast(t('sync.copied_toast'), 'success');
	}

	async function handleAcceptInvite() {
		if (!inviteSyncUrl.trim() || !inviteToken.trim()) {
			error = t('sync.url_token_required');
			return;
		}

		acceptingInvite = true;
		error = '';

		try {
			const vaultInfo = await acceptInvite(inviteSyncUrl.trim(), inviteToken.trim());
			addToast(t('toast.joined_vault', { name: vaultInfo.name }), 'success');
			inviteSyncUrl = '';
			inviteToken = '';
			await refreshStatuses();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			acceptingInvite = false;
		}
	}
</script>

{#if loading}
	<p class="section-desc">{t('common.loading')}</p>
{:else}
	<p class="section-desc">{t('sync.cloud_sync_desc')}</p>

	<!-- Account: what this device signs in to Turso with -->
	<div class="section turso-part" class:in-use={savedCredentials && inUse}>
		<h4 class="part-title">
			{t('sync.cloud_sync')}
			{#if savedCredentials && inUse}<span class="active-badge">{t('sync.active_badge')}</span>{/if}
		</h4>
		<Input label={t('sync.organization')} placeholder="my-org" bind:value={tursoOrg} />
		<Input label={t('sync.platform_api_token')} type="password" placeholder="eyJhbG..." bind:value={tursoApiToken} />
		<Input label={t('sync.group')} placeholder="default" bind:value={tursoGroup} />
	</div>

	<!-- The personal vault's database -->
	<div class="turso-part" class:in-use={personalOnTurso}>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">
					{t('sync.personal_database')}
					{#if personalOnTurso}<span class="active-badge">{t('sync.active_badge')}</span>{/if}
				</span>
				<span class="setting-description mono">
					{personalDbUrl || t('sync.will_be_created')}
				</span>
				<span class="setting-description">
					{personalOnTurso ? t('sync.cloud_sync_active') : t('sync.turso_personal_elsewhere')}
				</span>
			</div>
			<div class="status-badge" class:enabled={!!personalDbUrl}>
				{personalDbUrl ? t('sync.ready') : t('sync.pending')}
			</div>
		</div>
	</div>

	{#if error}
		<div class="form-error">{error}</div>
	{/if}

	<div class="action-row">
		<Button variant="primary" size="sm" onclick={handleSave} disabled={saving}>
			{saving ? t('sync.setting_up') : personalOnTurso ? t('sync.save_setup') : t('sync.turso_use')}
		</Button>
	</div>

	<!-- Shared vaults on Turso -->
	<div class="section turso-part" class:in-use={sharedOnTurso.length > 0}>
		<h4 class="part-title">
			{t('sync.turso_shared_vaults')}
			{#if sharedOnTurso.length > 0}<span class="active-badge">{t('sync.active_badge')}</span>{/if}
		</h4>
		<p class="section-desc">
			{sharedOnTurso.length > 0 ? t('sync.turso_shared_vaults_desc') : t('sync.turso_shared_none')}
		</p>
		{#each sharedOnTurso as s (s.vaultId)}
			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{s.name}</span>
					<span class="setting-description">
						{s.lastSync
							? t('sync.last_synced', { time: new Date(s.lastSync).toLocaleString() })
							: t('sync.never_synced')}
					</span>
				</div>
			</div>
		{/each}
	</div>

	<!-- Accept an invite to someone's shared vault -->
	<div class="section turso-part">
		<h4 class="part-title">{t('sync.accept_invite')}</h4>
		<p class="section-desc">{t('sync.accept_invite_desc')}</p>
		<Input
			label={t('vault.sync_url')}
			placeholder="libsql://vault-xxx.turso.io"
			bind:value={inviteSyncUrl}
			disabled={acceptingInvite}
		/>
		<Input
			label={t('vault.token')}
			type="password"
			placeholder="eyJhbG..."
			bind:value={inviteToken}
			disabled={acceptingInvite}
		/>
		<div class="action-row">
			<Button
				variant="secondary"
				size="sm"
				onclick={handleAcceptInvite}
				disabled={acceptingInvite || !inviteSyncUrl.trim() || !inviteToken.trim()}
			>
				{acceptingInvite ? t('sync.joining') : t('sync.accept')}
			</Button>
		</div>
	</div>

	<!-- Identity: what the owner of a shared vault needs to invite this user -->
	<div class="section turso-part" class:in-use={sharedOnTurso.length > 0}>
		<h4 class="part-title">{t('sync.identity')}</h4>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('sync.user_uuid')}</span>
				<span class="setting-description mono">{userUuid ?? t('sync.not_initialized')}</span>
			</div>
			{#if userUuid}
				<button class="copy-btn" onclick={() => copyToClipboard(userUuid!)}>{t('sync.copy')}</button>
			{/if}
		</div>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('sync.public_key')}</span>
				<span class="setting-description mono truncate">{publicKey ?? t('sync.not_initialized')}</span>
			</div>
			{#if publicKey}
				<button class="copy-btn" onclick={() => copyToClipboard(publicKey!)}>{t('sync.copy')}</button>
			{/if}
		</div>
	</div>
{/if}

<style>
	.section-desc {
		margin: 0;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.turso-part {
		padding-block: 6px;
	}

	.part-title {
		margin: 0;
		font-size: 0.8125rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.mono {
		font-family: var(--font-mono);
		word-break: break-all;
	}

	.truncate {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 220px;
		word-break: normal;
	}

	.copy-btn {
		padding: 4px 8px;
		font-size: 0.6875rem;
		font-weight: 500;
		color: var(--color-accent);
		background: transparent;
		border: 1px solid var(--color-accent);
		border-radius: 4px;
		cursor: pointer;
	}

	.copy-btn:hover {
		background: color-mix(in srgb, var(--color-accent) 10%, transparent);
	}

	.status-badge {
		padding: 4px 10px;
		font-size: 0.75rem;
		font-weight: 500;
		border-radius: 6px;
		background-color: rgba(255, 69, 58, 0.12);
		color: var(--color-danger);
	}

	.status-badge.enabled {
		background-color: rgba(48, 209, 88, 0.12);
		color: var(--color-success);
	}

	.form-error {
		font-size: 0.75rem;
		color: var(--color-danger);
		padding: 4px 0;
	}
</style>
