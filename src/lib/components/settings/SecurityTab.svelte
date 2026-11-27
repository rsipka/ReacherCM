<script lang="ts">
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import {
		hasMasterPassword,
		setMasterPassword,
		isLocked as checkIsLocked
	} from '#lib/ipc/credentials.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { hostKeysRemoveAll } from '#lib/ipc/sessions.js';
	import Dropdown from '#lib/components/shared/Dropdown.svelte';
	import Toggle from '#lib/components/shared/Toggle.svelte';
	import { getSettings, updateSetting } from '#lib/state/settings.svelte.js';
	import { biometricName, biometricPrompt, lock as lockVault } from '#lib/state/vault.svelte.js';
	import {
		biometricEnable,
		securityKeyAdd,
		unlockMethodRemove,
		unlockMethods,
		type UnlockMethods
	} from '#lib/ipc/vault.js';
	import { forgetBiometricKeys, DEFAULT_KEY_SETTING } from '#lib/ipc/savedCredentials.js';
	import { sshKeyList, type StoredKeyInfo } from '#lib/ipc/sshkeys.js';
	import { get as getSetting, set as setSetting } from '#lib/ipc/settings.js';
	import {
		PASSWORD_HISTORY_DEFAULT_LIMIT,
		PASSWORD_HISTORY_LIMITS,
		PASSWORD_HISTORY_LIMIT_SETTING,
		passwordHistoryClearAll
	} from '#lib/ipc/passwordHistory.js';

	const settings = getSettings();
	const autoLockOptions = $derived([
		{ label: t('security.auto_lock_never'), value: '0' },
		...[1, 5, 15, 30, 60].map((m) => ({ label: t('security.auto_lock_minutes', { count: m }), value: String(m) }))
	]);

	let hasPassword = $state(false);
	let locked = $state(false);
	let loading = $state(true);

	let showPasswordForm = $state(false);
	let currentPassword = $state('');
	let newPassword = $state('');
	let confirmPassword = $state('');
	let error = $state('');
	let saving = $state(false);

	// Device unlock methods: the platform biometric and security keys.
	let methods = $state<UnlockMethods | null>(null);
	let methodBusy = $state(false);
	let methodError = $state('');
	let addingKey = $state(false);
	let keyLabel = $state('');
	let keyPin = $state('');
	const biometric = $derived(methods?.methods.find((m) => m.kind !== 'fido2'));
	const method = $derived(biometricName(methods?.platform, methods?.platform_label));

	// Off forgets the passphrases remembered behind the biometric; so does
	// turning off unlocking at start, which is what fills them.
	function setKeysBiometric(on: boolean) {
		updateSetting('unlockKeysWithBiometrics', on);
		if (!on) void forgetBiometricKeys().catch((err) => addToast(String(err), 'error'));
	}

	function setUnlockKeysAtStart(on: boolean) {
		updateSetting('unlockKeysAtStart', on);
		if (!on && settings.unlockKeysWithBiometrics) setKeysBiometric(false);
	}
	const keys = $derived(methods?.methods.filter((m) => m.kind === 'fido2') ?? []);
	const biometricNote = $derived.by(() => {
		if (!methods?.platform_available) return t('security.biometric_unavailable', { method });
		if (!hasPassword) return t('security.biometric_needs_password', { method });
		return t('security.biometric_desc', { method });
	});

	// Remove all saved host keys: asked once more in place, then done.
	let hostKeysAsk = $state(false);
	let hostKeysBusy = $state(false);
	let hostKeysDone = $state('');
	let hostKeysError = $state('');

	async function removeAllHostKeys() {
		hostKeysBusy = true;
		hostKeysError = '';
		try {
			const r = await hostKeysRemoveAll();
			hostKeysDone = t('security.host_keys_removed', { sessions: r.sessions, device: r.device });
			if (r.failed > 0) hostKeysError = t('security.host_keys_remove_failed', { count: r.failed });
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
		} catch (e) {
			hostKeysError = String(e);
		} finally {
			hostKeysBusy = false;
			hostKeysAsk = false;
		}
	}

	// Password history: how many old passwords each session and credential
	// keeps (in the vault, so it follows the user), and forgetting them all.
	let historyLimit = $state(String(PASSWORD_HISTORY_DEFAULT_LIMIT));
	let historyAsk = $state(false);
	let historyBusy = $state(false);
	let historyDone = $state('');
	let historyError = $state('');
	const historyOptions = $derived(
		PASSWORD_HISTORY_LIMITS.map((n) => ({
			label: n === 0 ? t('password_history.limit_off') : t('password_history.limit_count', { count: n }),
			value: String(n)
		}))
	);

	// "My default key": the imported key sessions set to it sign in with
	// (a shared vault's, so each member uses their own). Kept in the vault's
	// settings, so it follows the user to their other devices.
	let defaultKey = $state('');
	let importedKeys = $state<StoredKeyInfo[]>([]);
	let defaultKeyError = $state('');
	const defaultKeyOptions = $derived([
		{ label: t('security.default_key_none'), value: '' },
		...importedKeys.map((k) => ({ label: k.algo ? `${k.name} · ${k.algo}` : k.name, value: k.id })),
		...(defaultKey && !importedKeys.some((k) => k.id === defaultKey) ? [{ label: t('session.key_missing_option'), value: defaultKey }] : [])
	]);

	async function loadDefaultKey() {
		try {
			importedKeys = await sshKeyList();
			defaultKey = (await getSetting(DEFAULT_KEY_SETTING)) ?? '';
		} catch {
			// Locked: nothing to pick from yet
		}
	}

	async function changeDefaultKey(value: string) {
		defaultKeyError = '';
		try {
			await setSetting(DEFAULT_KEY_SETTING, value);
			defaultKey = value;
		} catch (e) {
			defaultKeyError = String(e);
		}
	}

	async function loadHistoryLimit() {
		try {
			const v = await getSetting(PASSWORD_HISTORY_LIMIT_SETTING);
			if (v !== null && PASSWORD_HISTORY_LIMITS.includes(Number(v))) historyLimit = v;
		} catch {
			// Locked or no IPC: the default is shown
		}
	}

	async function changeHistoryLimit(value: string) {
		historyError = '';
		try {
			await setSetting(PASSWORD_HISTORY_LIMIT_SETTING, value);
			historyLimit = value;
		} catch (e) {
			historyError = String(e);
		}
	}

	async function clearAllHistory() {
		historyBusy = true;
		historyError = '';
		try {
			const n = await passwordHistoryClearAll();
			historyDone = t('password_history.cleared_all', { count: n });
		} catch (e) {
			historyError = String(e);
		} finally {
			historyBusy = false;
			historyAsk = false;
		}
	}

	async function changeMethods(change: () => Promise<void>) {
		methodBusy = true;
		methodError = '';
		try {
			await change();
		} catch (e) {
			methodError = String(e);
		}
		try {
			methods = await unlockMethods();
		} catch {
			// Keep the last known state
		}
		methodBusy = false;
	}

	function toggleBiometric(on: boolean) {
		void changeMethods(async () => {
			if (on) await biometricEnable(biometricPrompt(t('security.biometric', { method })));
			else if (biometric) await unlockMethodRemove(biometric.id);
		});
	}

	function addKey() {
		const label = keyLabel.trim() || t('security.keys_default_name');
		const pin = keyPin;
		keyPin = '';
		void changeMethods(async () => {
			await securityKeyAdd(label, methods?.keys_ask_pin ? pin : undefined);
			addingKey = false;
			keyLabel = '';
		});
	}

	$effect(() => {
		loadStatus();
		void loadHistoryLimit();
		void loadDefaultKey();
	});

	async function loadStatus() {
		loading = true;
		try {
			hasPassword = await hasMasterPassword();
			locked = await checkIsLocked();
			methods = await unlockMethods().catch(() => null);
		} catch {
			// IPC not available in dev, set safe defaults
			hasPassword = false;
			locked = false;
		}
		loading = false;
	}

	function openPasswordForm() {
		showPasswordForm = true;
		currentPassword = '';
		newPassword = '';
		confirmPassword = '';
		error = '';
	}

	function cancelPasswordForm() {
		showPasswordForm = false;
		currentPassword = '';
		newPassword = '';
		confirmPassword = '';
		error = '';
	}

	async function savePassword() {
		error = '';

		if (newPassword.length < 8) {
			error = t('security.password_too_short');
			return;
		}

		if (newPassword !== confirmPassword) {
			error = t('security.passwords_mismatch');
			return;
		}

		saving = true;
		try {
			await setMasterPassword(newPassword, hasPassword ? currentPassword : undefined);
			hasPassword = true;
			showPasswordForm = false;
			currentPassword = '';
			newPassword = '';
			confirmPassword = '';
		} catch (e) {
			error = e instanceof Error ? e.message : String(e) || 'Failed to set password';
		}
		saving = false;
	}

	async function handleLock() {
		try {
			// Held: the lock screen shows, and only the user opens it again.
			await lockVault();
			locked = true;
		} catch {
			// Lock failed
		}
	}
</script>

<div class="tab-content">
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('security.master_password_status')}</span>
			<span class="setting-description">
				{#if loading}
					Checking...
				{:else if hasPassword}
					{t('security.password_set')}
				{:else}
					{t('security.no_password')}
				{/if}
			</span>
		</div>
		<div class="setting-control">
			<div class="status-badge" class:set={hasPassword && !loading}>
				{#if loading}
					...
				{:else if hasPassword}
					<svg width="14" height="14" viewBox="0 0 14 14" fill="none">
						<path d="M2 7L5.5 10.5L12 3.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
					</svg>
					Set
				{:else}
					<svg width="14" height="14" viewBox="0 0 14 14" fill="none">
						<path d="M1 1L13 13M13 1L1 13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
					</svg>
					Not set
				{/if}
			</div>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('security.lock_status')}</span>
			<span class="setting-description">
				{locked ? t('security.locked_desc') : t('security.unlocked_desc')}
			</span>
		</div>
		<div class="setting-control">
			<div class="lock-badge" class:locked>
				{#if locked}
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
						<path d="M7 11V7a5 5 0 0 1 10 0v4" />
					</svg>
					{t('security.locked')}
				{:else}
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
						<path d="M7 11V7a5 5 0 0 1 9.9-1" />
					</svg>
					{t('security.unlocked')}
				{/if}
			</div>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('security.auto_lock')}</span>
			<span class="setting-description">{t('security.auto_lock_desc')}</span>
		</div>
		<div class="setting-control">
			<Dropdown
				options={autoLockOptions}
				selected={String(settings.autoLockMinutes)}
				onchange={(value) => updateSetting('autoLockMinutes', Number(value))}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('security.lock_on_system_lock')}</span>
			<span class="setting-description">{t('security.lock_on_system_lock_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.lockOnSystemLock}
				label={t('security.lock_on_system_lock')}
				onchange={(checked) => updateSetting('lockOnSystemLock', checked)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('security.unlock_keys_at_start')}</span>
			<span class="setting-description">{t('security.unlock_keys_at_start_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.unlockKeysAtStart}
				label={t('security.unlock_keys_at_start')}
				onchange={setUnlockKeysAtStart}
			/>
		</div>
	</div>

	{#if methods && (methods.keys_biometric || (methods.platform && settings.unlockKeysWithBiometrics))}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('security.keys_biometric', { method })}</span>
				<span class="setting-description">
					{#if !settings.unlockKeysAtStart}
						{t('security.keys_biometric_needs_start')}
					{:else if methods.platform === 'windows_hello'}
						{t('security.keys_windows_hello_desc')}
					{:else}
						{t('security.keys_biometric_desc', { method })}
					{/if}
				</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.unlockKeysAtStart && settings.unlockKeysWithBiometrics}
					label={t('security.keys_biometric', { method })}
					disabled={!settings.unlockKeysAtStart || (!methods.keys_biometric && !settings.unlockKeysWithBiometrics)}
					onchange={setKeysBiometric}
				/>
			</div>
		</div>
	{/if}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('security.host_keys_remove_all')}</span>
			<span class="setting-description">
				{#if hostKeysAsk}
					{t('security.host_keys_remove_all_confirm')}
				{:else if hostKeysDone}
					{hostKeysDone}
				{:else}
					{t('security.host_keys_remove_all_desc')}
				{/if}
			</span>
			{#if hostKeysError}
				<span class="form-error">{hostKeysError}</span>
			{/if}
		</div>
		<div class="setting-control" style="gap: 8px">
			{#if hostKeysAsk}
				<Button variant="ghost" size="sm" disabled={hostKeysBusy} onclick={() => (hostKeysAsk = false)}>
					{t('common.cancel')}
				</Button>
				<Button variant="danger" size="sm" disabled={hostKeysBusy} onclick={removeAllHostKeys}>
					{t('security.host_keys_remove_all_yes')}
				</Button>
			{:else}
				<Button variant="secondary" size="sm" disabled={locked} onclick={() => { hostKeysAsk = true; hostKeysDone = ''; hostKeysError = ''; }}>
					{t('security.host_keys_remove_all_button')}
				</Button>
			{/if}
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('security.default_key')}</span>
			<span class="setting-description">{t('security.default_key_desc')}</span>
			{#if defaultKeyError}
				<span class="form-error">{defaultKeyError}</span>
			{/if}
		</div>
		<div class="setting-control">
			<Dropdown options={defaultKeyOptions} selected={defaultKey} onchange={(v) => void changeDefaultKey(v)} />
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('password_history.limit')}</span>
			<span class="setting-description">{t('password_history.limit_desc')}</span>
		</div>
		<div class="setting-control">
			<Dropdown options={historyOptions} selected={historyLimit} onchange={(v) => void changeHistoryLimit(v)} />
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('password_history.clear_all')}</span>
			<span class="setting-description">
				{#if historyAsk}
					{t('password_history.clear_all_confirm')}
				{:else if historyDone}
					{historyDone}
				{:else}
					{t('password_history.clear_all_desc')}
				{/if}
			</span>
			{#if historyError}
				<span class="form-error">{historyError}</span>
			{/if}
		</div>
		<div class="setting-control" style="gap: 8px">
			{#if historyAsk}
				<Button variant="ghost" size="sm" disabled={historyBusy} onclick={() => (historyAsk = false)}>
					{t('common.cancel')}
				</Button>
				<Button variant="danger" size="sm" disabled={historyBusy} onclick={clearAllHistory}>
					{t('password_history.clear_all_yes')}
				</Button>
			{:else}
				<Button variant="secondary" size="sm" disabled={locked} onclick={() => { historyAsk = true; historyDone = ''; historyError = ''; }}>
					{t('password_history.clear_all_button')}
				</Button>
			{/if}
		</div>
	</div>

	{#if methods?.platform}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('security.biometric', { method })}</span>
				<span class="setting-description">{biometricNote}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={!!biometric}
					label={t('security.biometric', { method })}
					disabled={methodBusy || locked || (!biometric && (!methods.platform_available || !hasPassword))}
					onchange={toggleBiometric}
				/>
			</div>
		</div>
	{/if}

	{#if methods?.keys_supported}
		<div class="setting-row keys-row">
			<div class="setting-info">
				<span class="setting-label">{t('security.keys')}</span>
				<span class="setting-description">
					{hasPassword ? t('security.keys_desc') : t('security.keys_needs_password')}
				</span>
				{#if keys.length}
					<ul class="key-list">
						{#each keys as key (key.id)}
							<li>
								<span class="key-name">{key.label}</span>
								<Button
									variant="ghost"
									size="sm"
									disabled={methodBusy || locked}
									onclick={() => void changeMethods(() => unlockMethodRemove(key.id))}
								>
									{t('security.keys_remove')}
								</Button>
							</li>
						{/each}
					</ul>
				{/if}
				{#if addingKey}
					<form
						class="key-form"
						onsubmit={(e) => {
							e.preventDefault();
							addKey();
						}}
					>
						<Input placeholder={t('security.keys_name_placeholder')} bind:value={keyLabel} />
						{#if methods.keys_ask_pin}
							<Input type="password" placeholder={t('lock.key_pin_placeholder')} bind:value={keyPin} />
						{/if}
						<span class="setting-description">{t('security.keys_touch_twice')}</span>
						<div class="form-actions">
							<Button variant="ghost" size="sm" disabled={methodBusy} onclick={() => (addingKey = false)}>
								{t('common.cancel')}
							</Button>
							<Button
								type="submit"
								size="sm"
								disabled={methodBusy || (methods.keys_ask_pin && !keyPin)}
							>
								{t('security.keys_add')}
							</Button>
						</div>
					</form>
				{/if}
				{#if methodError}
					<span class="form-error">{methodError}</span>
				{/if}
			</div>
			{#if !addingKey}
				<div class="setting-control">
					<Button
						variant="secondary"
						size="sm"
						disabled={methodBusy || locked || !hasPassword}
						onclick={() => (addingKey = true)}
					>
						{t('security.keys_add')}
					</Button>
				</div>
			{/if}
		</div>
	{:else if methodError}
		<span class="form-error">{methodError}</span>
	{/if}

	<div class="action-row">
		{#if !showPasswordForm}
			<Button
				variant="secondary"
				size="sm"
				onclick={openPasswordForm}
			>
				{hasPassword ? t('security.change_password') : t('security.set_password')}
			</Button>

			{#if !locked}
				<Button
					variant="danger"
					size="sm"
					onclick={handleLock}
				>
					{t('security.lock_now')}
				</Button>
			{/if}
		{/if}
	</div>

	{#if showPasswordForm}
		<div class="password-form">
			{#if hasPassword}
				<div class="form-field">
					<Input
						label={t('security.current_password')}
						type="password"
						bind:value={currentPassword}
					/>
				</div>
			{/if}

			<div class="form-field">
				<Input
					label={t('security.new_password')}
					type="password"
					placeholder={t('settings.new_password_placeholder')}
					bind:value={newPassword}
				/>
			</div>

			<div class="form-field">
				<Input
					label={t('security.confirm_password')}
					type="password"
					placeholder={t('common.reenter_password')}
					bind:value={confirmPassword}
				/>
			</div>

			{#if error}
				<div class="form-error">{error}</div>
			{/if}

			<div class="form-actions">
				<Button
					variant="ghost"
					size="sm"
					onclick={cancelPasswordForm}
				>
					{t('common.cancel')}
				</Button>
				<Button
					variant="primary"
					size="sm"
					disabled={saving || newPassword.length === 0}
					onclick={savePassword}
				>
					{saving ? t('security.saving') : t('security.save_password')}
				</Button>
			</div>
		</div>
	{/if}
</div>

<style>

	.status-badge {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 4px 10px;
		border-radius: 6px;
		font-size: 0.75rem;
		font-weight: 500;
		background-color: rgba(255, 69, 58, 0.12);
		color: var(--color-danger);
	}

	.status-badge.set {
		background-color: rgba(48, 209, 88, 0.12);
		color: var(--color-success);
	}

	.lock-badge {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 4px 10px;
		border-radius: 6px;
		font-size: 0.75rem;
		font-weight: 500;
		background-color: rgba(255, 214, 10, 0.12);
		color: var(--color-warning);
	}

	.lock-badge.locked {
		background-color: rgba(48, 209, 88, 0.12);
		color: var(--color-success);
	}

	.keys-row {
		align-items: flex-start;
	}

	.key-list {
		list-style: none;
		margin: 8px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.key-list li {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
	}

	.key-name {
		font-size: 0.9em;
		color: var(--color-text-primary);
	}

	.key-form {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 8px;
	}

	.password-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 16px;
		margin-top: 8px;
		background-color: var(--color-bg-secondary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card);
	}

	.form-field {
		width: 100%;
	}

	.form-error {
		font-size: 0.75rem;
		color: var(--color-danger);
		padding: 4px 0;
	}

	.form-actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		padding-top: 4px;
	}
</style>
