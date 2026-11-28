import { invoke } from '@tauri-apps/api/core';

// Types
export interface VaultInfo {
	id: string;
	name: string;
	vaultType: 'private' | 'shared';
	memberCount?: number;
	secretCount: number;
	lastSync?: number;
	/** The vault is open but its store did not answer — shared vaults are
	 *  remote-only, so a stale token or a dead link degrades them rather than
	 *  failing the whole listing. */
	unreachable?: boolean;
	/** Why it is unreachable, already made readable by the backend. */
	syncError?: string;
}

export interface SecretMetadata {
	id: string;
	name: string;
	category: string;
	createdAt: number;
	updatedAt: number;
}

export interface MemberInfo {
	userUuid: string;
	publicKey: string;
	role: string;
	addedAt: number;
}

export interface InviteInfo {
	vaultId: string;
	syncUrl: string;
	token: string;
}

export interface VaultRestoreReview {
	request: string;
	activeRevision: string;
	snapshotRevision: string;
}

export function reviewVaultRestore(vaultId?: string): Promise<VaultRestoreReview | null> {
	return invoke('vault_restore_review', { vaultId });
}

export function confirmVaultRestore(review: VaultRestoreReview, vaultId?: string): Promise<void> {
	return invoke('vault_restore_confirm', { review, vaultId });
}

// ==================== IDENTITY ====================

export async function initIdentity(password: string): Promise<string> {
	return invoke<string>('vault_init_identity', { password });
}

export async function unlock(password: string): Promise<boolean> {
	return invoke<boolean>('vault_unlock', { password });
}

export async function lock(): Promise<void> {
	return invoke('vault_lock');
}

export async function isLocked(): Promise<boolean> {
	return invoke<boolean>('vault_is_locked');
}

export async function hasIdentity(): Promise<boolean> {
	return invoke<boolean>('vault_has_identity');
}

/** Auto-unlock using OS keychain (TLS-style, no password needed). */
export async function autoUnlock(): Promise<boolean> {
	return invoke<boolean>('vault_auto_unlock');
}

/** Whether the vault is being kept locked until the user opens it again. */
export async function isHeld(): Promise<boolean> {
	return invoke<boolean>('vault_is_held');
}

/** Open a held vault with the keychain, as the user's own act. */
export async function resume(): Promise<boolean> {
	return invoke<boolean>('vault_resume');
}

/** The platform biometric: Windows Hello, Touch ID, Android's fingerprint,
 *  or Face ID / Touch ID on iOS. */
export type PlatformBiometric = 'windows_hello' | 'touch_id' | 'android_biometric' | 'ios_biometric';

/** A device unlock method that is on: the platform biometric, or one security key. */
export interface UnlockMethod {
	id: string;
	kind: PlatformBiometric | 'fido2';
	label: string;
	created: number;
}

/** The device unlock methods this build offers here, and which are on. */
export interface UnlockMethods {
	/** The platform biometric this build offers, if any. */
	platform: PlatformBiometric | null;
	/** What this device calls it, where that varies (iOS: Face ID, Touch ID). */
	platform_label?: string | null;
	/** Key passphrases can be remembered behind the biometric (Android, iOS). */
	keys_biometric?: boolean;
	platform_available: boolean;
	keys_supported: boolean;
	/** Reach asks for the key's PIN itself (macOS, Linux); Windows asks. */
	keys_ask_pin: boolean;
	methods: UnlockMethod[];
}

export async function unlockMethods(): Promise<UnlockMethods> {
	return invoke<UnlockMethods>('vault_unlock_methods');
}

/** What Android's fingerprint prompt says; the desktop prompts are the
 * system's own and ignore it. */
export interface PromptText {
	title: string;
	subtitle?: string;
	cancel: string;
}

/** Turn the platform biometric on; shows the system prompt. */
export async function biometricEnable(prompt?: PromptText): Promise<void> {
	return invoke<void>('vault_biometric_enable', { prompt: prompt ?? null });
}

/** Open the vault with the platform biometric; shows the system prompt. */
export async function biometricUnlock(prompt?: PromptText): Promise<boolean> {
	return invoke<boolean>('vault_biometric_unlock', { prompt: prompt ?? null });
}

/** Add a security key; the user touches it twice. */
export async function securityKeyAdd(label: string, pin?: string): Promise<void> {
	return invoke<void>('vault_security_key_add', { label, pin: pin ?? null });
}

/** Open the vault with an added security key. */
export async function securityKeyUnlock(pin?: string): Promise<boolean> {
	return invoke<boolean>('vault_security_key_unlock', { pin: pin ?? null });
}

/** Remove a device unlock method. */
export async function unlockMethodRemove(id: string): Promise<void> {
	return invoke<void>('vault_unlock_method_remove', { id });
}

/** Export identity for backup/multi-device (returns base64 secret key).
 * WARNING: This is SENSITIVE! Protect this value! */
export async function exportIdentity(): Promise<string> {
	return invoke<string>('vault_export_identity');
}

/** Import identity from backup (for new device). */
export async function importIdentity(secretKey: string): Promise<string> {
	return invoke<string>('vault_import_identity', { secret_key: secretKey });
}

/** A code that links another device to this identity and the vaults it
 * syncs. As SENSITIVE as the backup key. */
export async function exportDeviceLink(): Promise<string> {
	return invoke<string>('vault_export_device_link');
}

/** `text` as a QR code, in SVG. */
export async function qrSvg(text: string): Promise<string> {
	return invoke<string>('qr_svg', { text });
}

/** Replace this device's identity with the one in a link code and open the
 * vaults it syncs. */
export async function linkDevice(code: string): Promise<string> {
	return invoke<string>('vault_link_device', { code });
}

export async function localLinkRecoveryAvailable(): Promise<boolean> {
	return invoke<boolean>('vault_local_link_recovery_available');
}

export async function restoreLocalBeforeLink(): Promise<void> {
	return invoke('vault_restore_local_before_link');
}

export async function getPublicKey(): Promise<string | null> {
	return invoke<string | null>('vault_get_public_key');
}

export async function getUserUuid(): Promise<string | null> {
	return invoke<string | null>('vault_get_user_uuid');
}

// ==================== VAULT MANAGEMENT ====================

export async function createVault(
	name: string,
	vaultType: 'private' | 'shared',
	syncUrl?: string,
	syncToken?: string
): Promise<VaultInfo> {
	return invoke<VaultInfo>('vault_create', {
		name,
		vault_type: vaultType,
		sync_url: syncUrl,
		sync_token: syncToken
	});
}

export async function openVault(
	vaultId: string,
	syncUrl?: string,
	token?: string
): Promise<VaultInfo> {
	return invoke<VaultInfo>('vault_open', { vault_id: vaultId, sync_url: syncUrl, token });
}

export async function closeVault(vaultId: string): Promise<void> {
	return invoke('vault_close', { vault_id: vaultId });
}

export async function deleteVault(vaultId: string): Promise<void> {
	return invoke('vault_delete', { vault_id: vaultId });
}

export async function listVaults(): Promise<VaultInfo[]> {
	return invoke<VaultInfo[]>('vault_list');
}

/** Shared vaults joined more than once, grouped; see DuplicateVaultsDialog. */
export async function vaultDuplicates(): Promise<VaultInfo[][]> {
	return invoke<VaultInfo[][]>('vault_duplicates');
}

export async function unlockVault(vaultId: string): Promise<void> {
	return invoke('vault_unlock_vault', { vault_id: vaultId });
}

export async function lockVault(vaultId: string): Promise<void> {
	return invoke('vault_lock_vault', { vault_id: vaultId });
}

export async function syncVault(vaultId: string): Promise<void> {
	return invoke('vault_sync', { vault_id: vaultId });
}

/** How a synced vault's sync stands. */
export interface SyncStatus {
	vaultId: string;
	name: string;
	/** "turso", or a file backend's kind. */
	backend: string;
	canRestoreShared: boolean;
	/** "legacy" (older Reach versions read it too), "2" (sealed records), or null before the first sync. */
	format: string | null;
	/** Milliseconds since the epoch. */
	lastSync: number | null;
	lastError: string | null;
	pending: number;
	conflicts: number;
}

export async function syncStatus(): Promise<SyncStatus[]> {
	return invoke<SyncStatus[]>('vault_sync_status');
}

/** What trying a sync backend found. */
export interface SyncProbe {
	ok: boolean;
	/** An SFTP server's host key, to confirm before anything is sent. */
	hostKey: string | null;
	/** 'empty', 'this' (this device's own data) or 'other' (another vault). */
	holds: 'empty' | 'this' | 'other' | null;
	error: string | null;
}

/** Try a sync backend before using it. */
export async function testSyncBackend(url: string, token: string): Promise<SyncProbe> {
	return invoke<SyncProbe>('vault_sync_test', { url, token });
}

export async function upgradeSyncFormat(vaultId: string): Promise<void> {
	return invoke('vault_sync_upgrade', { vault_id: vaultId });
}

export async function clearSyncConflicts(vaultId: string): Promise<void> {
	return invoke('vault_sync_clear_conflicts', { vault_id: vaultId });
}

// ==================== SECRETS ====================

export async function createSecret(
	vaultId: string,
	name: string,
	category: string,
	value: string
): Promise<string> {
	return invoke<string>('vault_secret_create', { vault_id: vaultId, name, category, value });
}

export async function readSecret(vaultId: string, secretId: string): Promise<string> {
	return invoke<string>('vault_secret_read', { vault_id: vaultId, secret_id: secretId });
}

export async function updateSecret(vaultId: string, secretId: string, value: string): Promise<void> {
	return invoke('vault_secret_update', { vault_id: vaultId, secret_id: secretId, value });
}

export async function deleteSecret(vaultId: string, secretId: string): Promise<void> {
	return invoke('vault_secret_delete', { vault_id: vaultId, secret_id: secretId });
}

export async function listSecrets(vaultId: string): Promise<SecretMetadata[]> {
	return invoke<SecretMetadata[]>('vault_secret_list', { vault_id: vaultId });
}

// ==================== SHARING ====================

export async function inviteMember(
	vaultId: string,
	inviteePublicKey: string,
	inviteeUuid: string,
	role: string
): Promise<InviteInfo> {
	return invoke<InviteInfo>('vault_invite_member', {
		vault_id: vaultId,
		invitee_public_key: inviteePublicKey,
		invitee_uuid: inviteeUuid,
		role
	});
}

export async function acceptInvite(syncUrl: string, token: string): Promise<VaultInfo> {
	return invoke<VaultInfo>('vault_accept_invite', { sync_url: syncUrl, token });
}

export async function removeMember(vaultId: string, userUuid: string): Promise<void> {
	return invoke('vault_remove_member', { vault_id: vaultId, user_uuid: userUuid });
}

export async function listMembers(vaultId: string): Promise<MemberInfo[]> {
	return invoke<MemberInfo[]>('vault_list_members', { vault_id: vaultId });
}

// ==================== SHARE INDIVIDUAL ITEMS ====================

export interface SharedItemInfo {
	id: string;
	secretId: string;
	recipientUuid: string;
	expiresAt?: number;
	createdAt: number;
}

export interface ShareItemResult {
	shareId: string;
	secretId: string;
	recipientUuid: string;
	syncUrl?: string;
	expiresAt?: number;
}

export interface ReceivedShare {
	shareId: string;
	secretId: string;
	secretName: string;
	category: string;
	sharerUuid: string;
	receivedAt: number;
}

/** Share a specific secret (session/credential) with another user. */
export async function shareItem(
	vaultId: string,
	secretId: string,
	recipientUuid: string,
	recipientPublicKey: string,
	expiresInHours?: number
): Promise<ShareItemResult> {
	return invoke<ShareItemResult>('vault_share_item', {
		vault_id: vaultId,
		secret_id: secretId,
		recipient_uuid: recipientUuid,
		recipient_public_key: recipientPublicKey,
		expires_in_hours: expiresInHours
	});
}

/** List items shared from a vault. */
export async function listSharedItems(vaultId: string): Promise<SharedItemInfo[]> {
	return invoke<SharedItemInfo[]>('vault_list_shared_items', { vault_id: vaultId });
}

/** Revoke a shared item. */
export async function revokeSharedItem(vaultId: string, shareId: string): Promise<void> {
	return invoke('vault_revoke_shared_item', { vault_id: vaultId, share_id: shareId });
}

/** Accept a shared item (copy to local vault). */
export async function acceptSharedItem(
	sourceVaultId: string,
	shareId: string,
	targetVaultId: string
): Promise<string> {
	return invoke<string>('vault_accept_shared_item', {
		source_vault_id: sourceVaultId,
		share_id: shareId,
		target_vault_id: targetVaultId
	});
}

/** List items shared with me. */
export async function listReceivedShares(): Promise<ReceivedShare[]> {
	return invoke<ReceivedShare[]>('vault_list_received_shares');
}

// ==================== APP SETTINGS (ENCRYPTED) ====================

export interface AppSettings {
	tursoOrg?: string;
	tursoApiToken?: string;
	tursoGroup?: string;
	personalDbUrl?: string;
	personalDbToken?: string;
	syncEnabled: boolean;
	theme?: string;
	[key: string]: unknown;
}

// ==================== TURSO PLATFORM API ====================

export interface TursoDbInfo {
	dbId: string;
	hostname: string;
	name: string;
}

/** Create a new database in Turso (for shared vaults). */
export async function createTursoDatabase(dbName: string): Promise<TursoDbInfo> {
	return invoke<TursoDbInfo>('turso_create_database', { db_name: dbName });
}

/** Personal vault databases already in the Turso organization. */
export async function listTursoPersonalDatabases(): Promise<TursoDbInfo[]> {
	return invoke<TursoDbInfo[]>('turso_list_personal_databases');
}

/** Create an auth token for a Turso database. */
export async function createTursoDatabaseToken(dbName: string): Promise<string> {
	return invoke<string>('turso_create_database_token', { db_name: dbName });
}

/** Get app settings. */
export async function getSettings(): Promise<AppSettings> {
	return invoke<AppSettings>('vault_get_settings');
}

/** Save app settings. */
export async function saveSettings(settings: AppSettings): Promise<void> {
	return invoke('vault_save_settings', { settings });
}

/** Get Turso config. */
export async function getTursoConfig(): Promise<[string | null, string | null]> {
	return invoke<[string | null, string | null]>('vault_get_turso_config');
}

/** Set Turso config. */
export async function setTursoConfig(org?: string, token?: string): Promise<void> {
	return invoke('vault_set_turso_config', { org, token });
}

// ==================== FULL BACKUP ====================

export interface BackupPreview {
	version: number;
	exportedAt: number;
	userUuid: string;
	vaultCount: number;
	secretCount: number;
	hasSyncConfig: boolean;
}

/** Export a full encrypted backup to a file. */
export async function exportBackup(exportPassword: string, filePath: string): Promise<void> {
	return invoke('vault_export_backup', { export_password: exportPassword, file_path: filePath });
}

/** Preview a backup file (validate and return metadata). */
export async function previewBackup(filePath: string, exportPassword: string): Promise<BackupPreview> {
	return invoke<BackupPreview>('vault_preview_backup', { file_path: filePath, export_password: exportPassword });
}

/** Import a full encrypted backup from a file. */
export async function importBackup(filePath: string, exportPassword: string, masterPassword: string): Promise<string> {
	return invoke<string>('vault_import_backup', { file_path: filePath, export_password: exportPassword, master_password: masterPassword });
}

// ==================== PERSONAL SYNC CONFIG ====================

/** Set personal sync config (for cloud backup of ALL user data). */
export async function setPersonalSync(syncUrl?: string, syncToken?: string): Promise<void> {
	return invoke('vault_set_personal_sync', { sync_url: syncUrl, sync_token: syncToken });
}

/** Get personal sync config. */
export async function getPersonalSync(): Promise<[string | null, string | null]> {
	return invoke<[string | null, string | null]>('vault_get_personal_sync');
}
