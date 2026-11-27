<script lang="ts">
	import { onMount, tick } from 'svelte';
	import SettingsCard from './SettingsCard.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { t } from '#lib/state/i18n.svelte.js';
	import { refreshVaults, setActiveVault } from '#lib/state/vault.svelte.js';

	type Organization = { id: string; name: string };
	type Role = 'owner' | 'editor' | 'viewer';
	type SharedVault = { id: string; name: string | null; role: Role; rotation_required: boolean };
	type Grant = { username: string; role: 'editor' | 'viewer' };

	let organizations = $state<Organization[]>([]);
	let vaults = $state<SharedVault[]>([]);
	let grants = $state<Grant[]>([]);
	let organization = $state('');
	let name = $state('');
	let creating = $state(false);
	let createForm = $state<HTMLFormElement>();
	let username = $state('');
	let publicKey = $state('');
	let role = $state<Role>('viewer');
	let managing = $state('');
	let verifying = $state('');
	let verifiedOwnerKey = $state('');
	let loaded = $state(false);
	let busy = $state(false);
	let confirmingTransfer = $state(false);
	let error = $state('');
	let message = $state('');

	onMount(() => { void reload(); });

	async function run(work: () => Promise<void>): Promise<void> {
		if (busy) return;
		busy = true;
		error = '';
		message = '';
		try {
			await work();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : String(cause);
		} finally {
			busy = false;
		}
	}

	async function loadVaults(): Promise<void> {
		vaults = [];
		creating = false;
		name = '';
		managing = '';
		verifying = '';
		verifiedOwnerKey = '';
		confirmingTransfer = false;
		if (organization) {
			vaults = await invoke<SharedVault[]>('vault_organization_list', { organization });
		}
	}

	function reload(): Promise<void> {
		return run(async () => {
			organizations = await invoke<Organization[]>('vault_organizations');
			loaded = true;
			if (!organizations.some(({ id }) => id === organization)) {
				organization = organizations[0]?.id ?? '';
			}
			await loadVaults();
		});
	}

	async function loadGrants(vaultId: string): Promise<void> {
		grants = await invoke<Grant[]>('vault_organization_grants', { organization, vaultId });
	}

	async function activate(id: string): Promise<void> {
		await refreshVaults();
		await setActiveVault(id);
		window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
	}

	function promptForOwnerKey(vaultId: string): void {
		if (error === 'organization.verify_owner_required' && verifying !== vaultId) {
			verifying = vaultId;
			verifiedOwnerKey = '';
		}
	}

	async function open(vaultId: string, manage = false, ownerKey?: string): Promise<void> {
		await run(async () => {
			const vault = await invoke<{ id: string; name: string }>('vault_organization_open', {
				organization, vaultId, verifiedOwnerKey: ownerKey
			});
			vaults = vaults.map((row) => row.id === vault.id ? { ...row, name: vault.name } : row);
			verifying = '';
			verifiedOwnerKey = '';
			if (manage) {
				await loadGrants(vault.id);
				managing = vault.id;
				username = '';
				publicKey = '';
				role = 'viewer';
				confirmingTransfer = false;
			} else {
				await activate(vault.id);
			}
		});
		promptForOwnerKey(vaultId);
	}

	function create(event: SubmitEvent): void {
		event.preventDefault();
		void run(async () => {
			const vault = await invoke<{ id: string; name: string }>('vault_organization_create', { organization, name });
			await activate(vault.id);
			await loadVaults();
		});
	}

	async function showCreate(): Promise<void> {
		managing = '';
		verifying = '';
		creating = true;
		await tick();
		createForm?.querySelector('input')?.focus();
	}

	function changeGrant(access: Role | null, recipient = username): Promise<void> {
		return run(async () => {
			await invoke('vault_organization_grant', { vaultId: managing, username: recipient, role: access, publicKey });
			confirmingTransfer = false;
			username = '';
			publicKey = '';
			if (access === 'owner') {
				await loadVaults();
			} else {
				await loadGrants(managing);
				if (access === null) {
					vaults = vaults.map((vault) => vault.id === managing ? { ...vault, rotation_required: false } : vault);
				}
			}
			message = access === null ? 'organization.key_rotated' : 'organization.access_saved';
		});
	}

	async function finishRotation(vaultId: string): Promise<void> {
		await run(async () => {
			await invoke('vault_organization_open', { organization, vaultId });
			await invoke('vault_organization_finish_rotation', { vaultId });
			await loadVaults();
			message = 'organization.key_rotated';
		});
		promptForOwnerKey(vaultId);
	}

	function grant(event: SubmitEvent): void {
		event.preventDefault();
		if (role === 'owner') {
			confirmingTransfer = true;
		} else {
			void changeGrant(role);
		}
	}
</script>

<SettingsCard title={t('organization.vaults')}>
	<div class="toolbar">
		{#if organizations.length}
			<div class="setting-info organization-picker">
				<label class="setting-label" for="share-organization">{t('organization.label')}</label>
				<select id="share-organization" bind:value={organization} onchange={() => run(loadVaults)} disabled={busy}>
					{#each organizations as org (org.id)}<option value={org.id}>{org.name}</option>{/each}
				</select>
			</div>
		{/if}
		<div class="actions">
			<Button variant="ghost" size="sm" disabled={busy}
				onclick={() => run(() => invoke('sync_account_manage', { organizations: true }))}>
				{t('organization.manage')}
			</Button>
			<Button variant="ghost" size="sm" disabled={busy} onclick={reload}>{t('organization.refresh')}</Button>
			{#if organizations.length && !creating}
				<Button variant="secondary" size="sm" disabled={busy} onclick={showCreate}>{t('organization.create')}</Button>
			{/if}
		</div>
	</div>
	{#if loaded && organizations.length === 0}
		<p class="setting-description feedback">{t('organization.empty')}</p>
	{:else if organizations.length}
		{#if creating}
			<form class="subform" bind:this={createForm} onsubmit={create}>
				<Input label={t('vault.vault_name')} bind:value={name} maxlength={64} required disabled={busy} />
				<div class="actions">
					<Button variant="ghost" size="sm" disabled={busy} onclick={() => { creating = false; name = ''; }}>{t('common.cancel')}</Button>
					<Button type="submit" size="sm" disabled={busy || !name.trim()}>{t('organization.create')}</Button>
				</div>
			</form>
		{/if}
		{#if vaults.length}
			<ul class="vault-list">
				{#each vaults as vault (vault.id)}
					<li>
						<div class="vault-row">
							<div class="setting-info">
								{#if vault.name}
									<span class="setting-label">{vault.name}</span>
								{:else}
									<code class="setting-description">{vault.id}</code>
								{/if}
								<span class="setting-description">{t('organization.role_' + vault.role)}</span>
							</div>
							<div class="actions">
								{#if vault.name || vault.role === 'owner'}
									<Button variant="secondary" size="sm" disabled={busy} onclick={() => open(vault.id)}>
										{t('organization.open')}
									</Button>
								{/if}
								<Button variant={vault.name ? 'ghost' : 'secondary'} size="sm" disabled={busy} ariaExpanded={verifying === vault.id}
									onclick={() => {
										managing = '';
										verifying = verifying === vault.id ? '' : vault.id;
										verifiedOwnerKey = '';
									}}>
									{t('organization.verify_owner')}
								</Button>
								{#if vault.role === 'owner'}
									<Button variant="ghost" size="sm" disabled={busy} ariaExpanded={managing === vault.id} onclick={() => {
										if (managing === vault.id) managing = '';
										else void open(vault.id, true);
									}}>
										{t('organization.access')}
									</Button>
								{/if}
							</div>
						</div>
						{#if verifying === vault.id}
							<form class="subform" onsubmit={(event) => {
								event.preventDefault();
								void open(vault.id, false, verifiedOwnerKey.trim());
							}}>
								<p class="setting-description">{t('organization.verify_owner_help')}</p>
								<Input label={t('organization.owner_public_key')} bind:value={verifiedOwnerKey} maxlength={128}
									required disabled={busy} spellcheck={false} autocapitalize="off" />
								<div class="actions">
									<Button variant="ghost" size="sm" disabled={busy} onclick={() => { verifying = ''; verifiedOwnerKey = ''; }}>
										{t('common.cancel')}
									</Button>
									<Button type="submit" size="sm" disabled={busy || !verifiedOwnerKey.trim()}>{t('organization.open')}</Button>
								</div>
							</form>
						{/if}
						{#if vault.rotation_required}
							<div class="subform">
								<p class="setting-description warning">{t('organization.rotation_pending')}</p>
								{#if vault.role === 'owner'}
									<p class="setting-description">{t('organization.rotation_update_clients')}</p>
									<div class="actions">
										<Button variant="secondary" size="sm" disabled={busy} onclick={() => finishRotation(vault.id)}>
											{t('organization.finish_rotation')}
										</Button>
									</div>
								{/if}
							</div>
						{/if}
						{#if managing === vault.id}
							<div class="subform">
								{#if grants.length}
									<ul class="grant-list">
										{#each grants as grant (grant.username)}
											<li class="grant-row">
												<div class="setting-info">
													<span class="setting-label">{grant.username}</span>
													<span class="setting-description">{t('organization.role_' + grant.role)}</span>
												</div>
												<Button variant="ghost" size="sm" disabled={busy || confirmingTransfer}
													onclick={() => changeGrant(null, grant.username)}>
													{t('organization.revoke')}
												</Button>
											</li>
										{/each}
									</ul>
								{/if}
								<form class="grant-form" onsubmit={grant}>
									<Input label={t('organization.username')} bind:value={username} maxlength={32}
										required disabled={busy || confirmingTransfer} spellcheck={false} autocapitalize="off" />
									<Input label={t('vault.invitee_public_key')} bind:value={publicKey} maxlength={128}
										required disabled={busy || confirmingTransfer} spellcheck={false} autocapitalize="off" />
									<label class="setting-label" for="org-grant-role">{t('organization.role')}</label>
									<select id="org-grant-role" bind:value={role} disabled={busy || confirmingTransfer}>
										<option value="viewer">{t('organization.role_viewer')}</option>
										<option value="editor">{t('organization.role_editor')}</option>
										<option value="owner">{t('organization.role_owner')}</option>
									</select>
									{#if confirmingTransfer}
										<p class="setting-description warning">{t('organization.transfer_confirm')}</p>
										<p class="setting-description">{t('organization.rotation_update_clients')}</p>
									{/if}
									<div class="actions">
										<Button variant="ghost" size="sm" disabled={busy} onclick={() => {
											if (confirmingTransfer) confirmingTransfer = false;
											else managing = '';
										}}>{t('common.cancel')}</Button>
										{#if confirmingTransfer}
											<Button size="sm" disabled={busy} onclick={() => changeGrant('owner')}>{t('common.confirm')}</Button>
										{:else}
											<Button type="submit" size="sm" disabled={busy || !username.trim() || !publicKey.trim()}>
												{t('organization.grant')}
											</Button>
										{/if}
									</div>
								</form>
							</div>
						{/if}
					</li>
				{/each}
			</ul>
		{:else if !busy}
			<p class="setting-description feedback">{t('organization.no_vaults')}</p>
		{/if}
	{/if}
	{#if busy}<p class="feedback" role="status">{t('common.loading')}</p>{/if}
	{#if message}<p class="feedback" role="status">{t(message)}</p>{/if}
	{#if error}<p class="feedback error" role="alert">{t(error)}</p>{/if}
</SettingsCard>

<style>
	.actions, .toolbar { display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; }
	.actions { align-items: center; }
	.toolbar { align-items: end; gap: 8px 14px; padding: 12px 0; }
	.organization-picker { flex: 1 1 8rem; max-width: 200px; }
	.toolbar > .actions { margin-left: auto; }
	.vault-list, .grant-list { list-style: none; padding: 0; margin: 0; }
	.vault-list > li { border-top: 1px solid var(--color-border); }
	.vault-row, .grant-row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 8px 16px;
		padding: 12px 0;
	}
	.vault-row > .setting-info { flex: 1 1 10rem; overflow-wrap: anywhere; }
	.vault-row > .actions { margin-left: auto; }
	.grant-row { padding-block: 6px; }
	.subform, .grant-form { display: grid; gap: 10px; }
	.subform {
		margin: 0 0 12px;
		padding: 12px;
		border-radius: var(--radius-card, 8px);
		background: var(--color-bg-elevated);
	}
	.subform p { margin: 0; }
	select {
		min-width: 0;
		max-width: 100%;
		padding: 7px 10px;
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
		font: inherit;
		font-size: 0.875rem;
	}
	.feedback { margin: 0; padding: 12px 0; color: var(--color-text-secondary); font-size: 0.8125rem; }
	.feedback.error { color: var(--color-danger); }
	.setting-description.warning { color: var(--color-warning); }
	@media (max-width: 640px) {
		.vault-row > .actions { margin-left: 0; justify-content: flex-start; }
	}
</style>
