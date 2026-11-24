<script lang="ts">
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
	let username = $state('');
	let publicKey = $state('');
	let role = $state<Role>('viewer');
	let managing = $state('');
	let loaded = $state(false);
	let busy = $state(false);
	let confirmingTransfer = $state(false);
	let error = $state('');
	let message = $state('');

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
		managing = '';
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

	function open(vaultId: string, manage = false): Promise<void> {
		return run(async () => {
			const vault = await invoke<{ id: string; name: string }>('vault_organization_open', { organization, vaultId });
			vaults = vaults.map((row) => row.id === vault.id ? { ...row, name: vault.name } : row);
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
	}

	function create(event: SubmitEvent): void {
		event.preventDefault();
		void run(async () => {
			const vault = await invoke<{ id: string; name: string }>('vault_organization_create', { organization, name });
			name = '';
			await activate(vault.id);
			await loadVaults();
		});
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

	function finishRotation(vaultId: string): Promise<void> {
		return run(async () => {
			await invoke('vault_organization_open', { organization, vaultId });
			await invoke('vault_organization_finish_rotation', { vaultId });
			await loadVaults();
			message = 'organization.key_rotated';
		});
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

<details ontoggle={(event) => { if (event.currentTarget.open && !loaded) void reload(); }}>
	<summary>{t('organization.vaults')}</summary>
	<div class="actions">
		<button type="button" disabled={busy} onclick={() => run(() => invoke('sync_account_manage', { organizations: true }))}>
			{t('organization.manage')}
		</button>
		<button type="button" disabled={busy} onclick={reload}>{t('organization.refresh')}</button>
	</div>
	{#if loaded && organizations.length === 0}
		<p>{t('organization.empty')}</p>
	{:else if organizations.length}
		<label>
			{t('organization.label')}
			<select bind:value={organization} onchange={() => run(loadVaults)} disabled={busy}>
				{#each organizations as org (org.id)}<option value={org.id}>{org.name}</option>{/each}
			</select>
		</label>
		<form class="actions" onsubmit={create}>
			<label>
				{t('vault.vault_name')}
				<input bind:value={name} maxlength="64" required disabled={busy} />
			</label>
			<button type="submit" disabled={busy || !name.trim()}>{t('organization.create')}</button>
		</form>
		{#if vaults.length}
			<ul>
				{#each vaults as vault (vault.id)}
					<li>
						<div class="actions">
							<span class="vault-name">{vault.name ?? vault.id}</span>
							<span class="role">{t('organization.role_' + vault.role)}</span>
							<button type="button" disabled={busy} onclick={() => open(vault.id)}>{t('organization.open')}</button>
							{#if vault.role === 'owner'}
								<button type="button" disabled={busy} onclick={() => open(vault.id, true)}>{t('organization.access')}</button>
							{/if}
						</div>
						{#if vault.rotation_required}
							<p>{t('organization.rotation_pending')}</p>
							{#if vault.role === 'owner'}
								<button type="button" disabled={busy} onclick={() => finishRotation(vault.id)}>
									{t('organization.finish_rotation')}
								</button>
							{/if}
						{/if}
						{#if managing === vault.id}
							{#if grants.length}
								<ul>
									{#each grants as grant (grant.username)}
										<li class="actions">
											<span class="vault-name">{grant.username}</span>
											<span class="role">{t('organization.role_' + grant.role)}</span>
											<button type="button" disabled={busy || confirmingTransfer} onclick={() => changeGrant(null, grant.username)}>
												{t('organization.revoke')}
											</button>
										</li>
									{/each}
								</ul>
							{/if}
							<form class="grant" onsubmit={grant}>
								<label>
									{t('organization.username')}
									<input bind:value={username} maxlength="32" required disabled={busy || confirmingTransfer} />
								</label>
								<label>
									{t('vault.invitee_public_key')}
									<input bind:value={publicKey} maxlength="128" disabled={busy || confirmingTransfer} />
								</label>
								<label>
									{t('organization.role')}
									<select bind:value={role} disabled={busy || confirmingTransfer}>
										<option value="viewer">{t('organization.role_viewer')}</option>
										<option value="editor">{t('organization.role_editor')}</option>
										<option value="owner">{t('organization.role_owner')}</option>
									</select>
								</label>
								{#if confirmingTransfer}
									<p>{t('organization.transfer_confirm')}</p>
									<div class="actions">
										<button type="button" disabled={busy} onclick={() => changeGrant('owner')}>{t('common.confirm')}</button>
										<button type="button" disabled={busy} onclick={() => confirmingTransfer = false}>{t('common.cancel')}</button>
									</div>
								{:else}
									<div class="actions">
										<button type="submit" disabled={busy || !username.trim() || !publicKey.trim()}>{t('organization.grant')}</button>
										<button type="button" disabled={busy} onclick={() => managing = ''}>{t('common.cancel')}</button>
									</div>
								{/if}
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{:else if !busy}
			<p>{t('organization.no_vaults')}</p>
		{/if}
	{/if}
	{#if busy}<p role="status">{t('sync_account.loading')}</p>{/if}
	{#if message}<p role="status">{t(message)}</p>{/if}
	{#if error}<p class="error" role="alert">{t(error)}</p>{/if}
</details>

<style>
	details { margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--color-border); }
	summary { cursor: pointer; }
	.actions { display: flex; flex-wrap: wrap; align-items: end; gap: 8px; margin: 8px 0; }
	label { display: grid; gap: 4px; width: fit-content; max-width: 100%; }
	button, input, select {
		padding: 7px 9px;
		border: 1px solid var(--color-border-strong);
		border-radius: 6px;
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
		font: inherit;
	}
	button { cursor: pointer; }
	button:hover:enabled { border-color: var(--color-accent); }
	button:disabled { opacity: 0.6; cursor: default; }
	ul { list-style: none; padding: 0; }
	li { border-top: 1px solid var(--color-border); }
	.vault-name { overflow-wrap: anywhere; align-self: center; }
	.role { color: var(--color-text-secondary); align-self: center; }
	.grant { display: grid; gap: 8px; margin: 10px 0; }
	p { color: var(--color-text-secondary); }
	.error { color: var(--color-danger); }
</style>
