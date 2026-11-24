<script lang="ts">
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import SyncStatusSection from './SyncStatusSection.svelte';
	import SyncBackendSection from './SyncBackendSection.svelte';
	import { getSettings, saveSettings, exportIdentity, linkDevice, restoreLocalBeforeLink } from '#lib/state/vault.svelte.js';
	import type { AppSettings } from '#lib/ipc/vault.js';
	import { createTursoDatabase, createTursoDatabaseToken, setPersonalSync, acceptInvite, exportDeviceLink, listTursoPersonalDatabases, qrSvg, localLinkRecoveryAvailable } from '#lib/ipc/vault.js';
	import { canScanLinkCode, scanLinkCode } from '#lib/state/link-scan.js';
	import { vaultState } from '#lib/state/vault.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSettings as getLocalSettings, updateSetting } from '#lib/state/settings.svelte.js';

	let loading = $state(true);
	let saving = $state(false);
	let tursoOrg = $state('');
	let tursoApiToken = $state('');
	let tursoGroup = $state('default');
	let personalDbUrl = $state('');
	let personalDbToken = $state('');
	let syncEnabled = $state(false);
	let error = $state('');

	// Identity info
	let publicKey = $derived(vaultState.publicKey);
	let userUuid = $derived(vaultState.userUuid);

	// Export key
	let showExportKey = $state(false);
	let exportedKey = $state('');

	// Link another device / join from one
	let linkCode = $state('');
	let linkQr = $state('');
	let joinCode = $state('');
	let joinArmed = $state(false);
	let joining = $state(false);
	let recoveryAvailable = $state(false);
	let confirmRecovery = $state(false);
	let restoringLocal = $state(false);
	let accountLink = $derived(joinCode.trim().startsWith('reachercm-account-link1:'));
	// Another device's personal database is in the organization: the next
	// Save makes a separate one anyway.
	let createAnyway = $state(false);

	// Accept invite
	let inviteSyncUrl = $state('');
	let inviteToken = $state('');
	let acceptingInvite = $state(false);

	$effect(() => {
		loadSettings();
	});

	async function loadSettings() {
		loading = true;
		try {
			recoveryAvailable = await localLinkRecoveryAvailable();
			const settings = await getSettings();
			tursoOrg = settings.tursoOrg ?? '';
			tursoApiToken = settings.tursoApiToken ?? '';
			tursoGroup = settings.tursoGroup ?? 'default';
			personalDbUrl = settings.personalDbUrl ?? '';
			personalDbToken = settings.personalDbToken ?? '';
			syncEnabled = settings.syncEnabled ?? false;

			// Pick up pending Turso creds from welcome screen setup
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
				// Step 1: Save Turso credentials FIRST (without DB URL)
				addToast(t('sync.saving_credentials'), 'info');
				const initialSettings: AppSettings = {
					tursoOrg: tursoOrg,
					tursoApiToken: tursoApiToken,
					tursoGroup: tursoGroup || 'default',
					syncEnabled: false
				};
				await saveSettings(initialSettings);

				// Step 2: Use this identity's database if it is already
				// there, or create it (backend reads the saved credentials).
				// A database made by another device's identity cannot be
				// opened from here: that device has to link this one.
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

			// Step 3: Save complete settings (with DB URL if created)
			const settings: AppSettings = {
				tursoOrg: tursoOrg || undefined,
				tursoApiToken: tursoApiToken || undefined,
				tursoGroup: tursoGroup || 'default',
				personalDbUrl: newPersonalDbUrl || undefined,
				personalDbToken: newPersonalDbToken || undefined,
				syncEnabled: !!(tursoOrg && tursoApiToken && newPersonalDbUrl)
			};
			await saveSettings(settings);
			syncEnabled = settings.syncEnabled;

			// Step 4: Set personal sync config in identity file so ALL data syncs to cloud
			if (newPersonalDbUrl && newPersonalDbToken) {
				await setPersonalSync(newPersonalDbUrl, newPersonalDbToken);
				addToast(t('sync.cloud_sync_enabled'), 'success');
			} else {
				addToast(t('sync.settings_saved'), 'success');
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
		saving = false;
	}

	async function handleExportKey() {
		try {
			exportedKey = await exportIdentity();
			showExportKey = true;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function handleLinkDevice() {
		error = '';
		try {
			linkCode = await exportDeviceLink();
			linkQr = await qrSvg(linkCode).catch(() => '');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function handleScan() {
		error = '';
		try {
			const code = await scanLinkCode();
			if (code) {
				joinCode = code;
				joinArmed = false;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function handleJoin() {
		if (!joinArmed) {
			joinArmed = true;
			return;
		}
		joining = true;
		error = '';
		try {
			await linkDevice(joinCode.trim());
			joinCode = '';
			joinArmed = false;
			addToast(t('sync.joined_toast'), 'success');
			await loadSettings();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			joining = false;
		}
	}

	async function restoreLocalVault(): Promise<void> {
		restoringLocal = true;
		error = '';
		try {
			await restoreLocalBeforeLink();
			confirmRecovery = false;
			recoveryAvailable = false;
			if (!vaultState.locked) await loadSettings();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			restoringLocal = false;
		}
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
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			acceptingInvite = false;
		}
	}
</script>

<div class="tab-content">
	{#if loading}
		<div class="loading">{t('common.loading')}</div>
	{:else}
		<!-- Identity Section -->
		<div class="section">
			<h3 class="section-title">{t('sync.identity')}</h3>
			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{t('sync.user_uuid')}</span>
					<span class="setting-value mono">{userUuid ?? t('sync.not_initialized')}</span>
				</div>
				{#if userUuid}
					<button class="copy-btn" onclick={() => copyToClipboard(userUuid!)}>{t('sync.copy')}</button>
				{/if}
			</div>
			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{t('sync.public_key')}</span>
					<span class="setting-value mono truncate">{publicKey ?? t('sync.not_initialized')}</span>
				</div>
				{#if publicKey}
					<button class="copy-btn" onclick={() => copyToClipboard(publicKey!)}>{t('sync.copy')}</button>
				{/if}
			</div>
			<div class="action-row">
				<Button variant="secondary" size="sm" onclick={handleExportKey}>
					{t('sync.export_backup_key')}
				</Button>
			</div>
		</div>

		{#if showExportKey}
			<div class="export-box">
				<p class="export-warning">{t('sync.backup_key_warning')}</p>
				<div class="export-key">
					<code>{exportedKey}</code>
					<button class="copy-btn" onclick={() => copyToClipboard(exportedKey)}>{t('sync.copy')}</button>
				</div>
				<Button variant="ghost" size="sm" onclick={() => { showExportKey = false; exportedKey = ''; }}>
					{t('common.close')}
				</Button>
			</div>
		{/if}

		<!-- Link Devices Section -->
		<div class="section">
			<h3 class="section-title">{t('sync.link_device')}</h3>
			<p class="section-desc">{t('sync.link_device_desc')}</p>
			<div class="action-row">
				<Button variant="secondary" size="sm" onclick={handleLinkDevice}>
					{t('sync.show_link_code')}
				</Button>
			</div>
			{#if linkCode}
				<div class="export-box">
					<p class="export-warning">{t('sync.link_code_warning')}</p>
					{#if linkQr}
						<p class="section-desc">{t('sync.link_qr_hint')}</p>
						<!-- SVG drawn by the backend from the code -->
						<div class="link-qr">{@html linkQr}</div>
					{/if}
					<div class="export-key">
						<code>{linkCode}</code>
						<button class="copy-btn" onclick={() => copyToClipboard(linkCode)}>{t('sync.copy')}</button>
					</div>
					<Button variant="ghost" size="sm" onclick={() => { linkCode = ''; linkQr = ''; }}>
						{t('common.close')}
					</Button>
				</div>
			{/if}

			<h3 class="section-title">{t('sync.join_device')}</h3>
			<p class="section-desc">{t('sync.join_device_desc')}</p>
			<div class="form-field">
				<Input
					label={t('sync.link_code')}
					type="password"
					placeholder="reachercm-link1:..."
					bind:value={joinCode}
					disabled={joining}
					oninput={() => (joinArmed = false)}
				/>
			</div>
			{#if joinArmed}
				<p class="export-warning">{t(accountLink ? 'sync_account.vault_link_confirm' : 'sync.join_confirm')}</p>
			{/if}
			<div class="action-row">
				{#if canScanLinkCode()}
					<Button variant="secondary" size="sm" onclick={handleScan} disabled={joining}>
						{t('sync.scan_qr')}
					</Button>
				{/if}
				<Button variant={joinArmed ? 'danger' : 'secondary'} size="sm" onclick={handleJoin} disabled={joining || !joinCode.trim()}>
					{joining ? t('sync.joining') : joinArmed ? t('sync.join_confirm_button') : t('sync.join')}
				</Button>
			</div>
		</div>

		{#if recoveryAvailable}
			<div class="section">
				{#if confirmRecovery}
					<p class="export-warning">{t('sync_account.local_vault_restore_confirm')}</p>
					<div class="action-row">
						<Button variant="danger" size="sm" onclick={restoreLocalVault} disabled={restoringLocal}>
							{t('common.confirm')}
						</Button>
						<Button variant="secondary" size="sm" onclick={() => { confirmRecovery = false; }} disabled={restoringLocal}>
							{t('common.cancel')}
						</Button>
					</div>
				{:else}
					<Button variant="secondary" size="sm" onclick={() => { confirmRecovery = true; }} disabled={joining}>
						{t('sync_account.local_vault_restore')}
					</Button>
				{/if}
			</div>
		{/if}

		<!-- Turso Sync Section -->
		<div class="section">
			<h3 class="section-title">{t('sync.cloud_sync')}</h3>
			<p class="section-desc">{t('sync.cloud_sync_desc')}</p>

			<div class="form-field">
				<Input
					label={t('sync.organization')}
					placeholder="my-org"
					bind:value={tursoOrg}
				/>
			</div>

			<div class="form-field">
				<Input
					label={t('sync.platform_api_token')}
					type="password"
					placeholder="eyJhbG..."
					bind:value={tursoApiToken}
				/>
			</div>

			<div class="form-field">
				<Input
					label={t('sync.group')}
					placeholder="default"
					bind:value={tursoGroup}
				/>
			</div>

			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{t('sync.personal_database')}</span>
					<span class="setting-description">
						{#if personalDbUrl}
							{personalDbUrl}
						{:else}
							{t('sync.will_be_created')}
						{/if}
					</span>
				</div>
				<div class="status-badge" class:enabled={!!personalDbUrl}>
					{personalDbUrl ? t('sync.ready') : t('sync.pending')}
				</div>
			</div>

			<div class="setting-row">
				<div class="setting-info">
					<span class="setting-label">{t('sync.sync_status')}</span>
					<span class="setting-description">
						{#if syncEnabled}
							{t('sync.cloud_sync_active')}
						{:else}
							{t('sync.local_only')}
						{/if}
					</span>
				</div>
				<div class="status-badge" class:enabled={syncEnabled}>
					{syncEnabled ? t('sync.enabled') : t('sync.disabled')}
				</div>
			</div>

			{#if error}
				<div class="form-error">{error}</div>
			{/if}

			<div class="action-row">
				<Button variant="primary" size="sm" onclick={handleSave} disabled={saving}>
					{saving ? t('sync.setting_up') : t('sync.save_setup')}
				</Button>
			</div>
		</div>

		<SyncBackendSection />

		<SyncStatusSection />

		<!-- Accept Invite Section -->
		<div class="section">
			<h3 class="section-title">{t('sync.accept_invite')}</h3>
			<p class="section-desc">{t('sync.accept_invite_desc')}</p>

			<div class="form-field">
				<Input
					label={t('vault.sync_url')}
					placeholder="libsql://vault-xxx.turso.io"
					bind:value={inviteSyncUrl}
					disabled={acceptingInvite}
				/>
			</div>

			<div class="form-field">
				<Input
					label={t('vault.token')}
					type="password"
					placeholder="eyJhbG..."
					bind:value={inviteToken}
					disabled={acceptingInvite}
				/>
			</div>

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
	{/if}
</div>

<style>
	.tab-content {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.loading {
		padding: 20px;
		text-align: center;
		color: var(--color-text-secondary);
	}

	.section-desc {
		margin: 0;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.setting-value {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.setting-value.mono {
		font-family: var(--font-mono);
	}

	.setting-value.truncate {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 200px;
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
		background: rgba(0, 122, 255, 0.1);
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

	.form-field {
		width: 100%;
	}

	.form-error {
		font-size: 0.75rem;
		color: var(--color-danger);
		padding: 4px 0;
	}

	.link-qr {
		align-self: center;
		width: min(320px, 100%);
		padding: 8px;
		background: #ffffff;
		border-radius: 8px;
	}

	.link-qr :global(svg) {
		display: block;
		width: 100%;
		height: auto;
	}

	.export-box {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 12px;
		background: rgba(255, 214, 10, 0.08);
		border: 1px solid rgba(255, 214, 10, 0.3);
		border-radius: 8px;
	}

	.export-warning {
		margin: 0;
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-warning);
	}

	.export-key {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px;
		background: var(--color-bg-primary);
		border-radius: 4px;
		overflow: hidden;
	}

	.export-key code {
		flex: 1;
		font-size: 0.6875rem;
		font-family: var(--font-mono);
		color: var(--color-text-primary);
		word-break: break-all;
	}
</style>
