import { invoke } from '@tauri-apps/api/core';
import type { AuthMethod } from './sessions';
import type { PromptText } from './vault';

/** A login kept once and used by many sessions in the same vault. */
export interface SavedCredential {
  id: string;
  name: string;
  username: string;
  /** Windows logon domain, used by RDP sessions. */
  domain?: string | null;
  auth_method: AuthMethod;
  /** The vault it is in (absent: this device's own sessions). */
  vault_id?: string | null;
  /** The credential folder it is filed in (absent: the top of the list). */
  folder_id?: string | null;
}

/** A folder in the credentials list, kept in the vault of its credentials. */
export interface CredentialFolder {
  id: string;
  name: string;
  parent_id?: string | null;
  vault_id?: string | null;
  /** Icon and colour, as for session folders. */
  icon?: string | null;
  color?: string | null;
}

export async function savedCredentialList(): Promise<SavedCredential[]> {
  return invoke<SavedCredential[]>('saved_credential_list');
}

export async function savedCredentialCreate(params: {
  name: string;
  username: string;
  domain?: string | null;
  authMethod: AuthMethod;
  vaultId?: string | null;
  folderId?: string | null;
}): Promise<SavedCredential> {
  return invoke<SavedCredential>('saved_credential_create', {
    name: params.name,
    username: params.username,
    domain: params.domain?.trim() ? params.domain.trim() : null,
    authMethod: params.authMethod,
    vaultId: params.vaultId ?? null,
    folderId: params.folderId ?? null,
  });
}

export async function savedCredentialUpdate(credential: SavedCredential): Promise<SavedCredential> {
  return invoke<SavedCredential>('saved_credential_update', { credential });
}

/** Delete a credential; the sessions and folders using it go back to their
 *  own login. Returns how many were changed. */
export async function savedCredentialDelete(credentialId: string): Promise<number> {
  return invoke<number>('saved_credential_delete', { credentialId });
}

/** File credentials in a folder of their vault, or at the top (null). */
export async function savedCredentialMove(credentialIds: string[], folderId: string | null): Promise<number> {
  return invoke<number>('saved_credential_move', { credentialIds, folderId });
}

export async function credentialFolderList(): Promise<CredentialFolder[]> {
  return invoke<CredentialFolder[]>('credential_folder_list');
}

export async function credentialFolderCreate(name: string, parentId: string | null, vaultId: string | null): Promise<CredentialFolder> {
  return invoke<CredentialFolder>('credential_folder_create', { name, parentId, vaultId });
}

export async function credentialFolderRename(folderId: string, name: string): Promise<CredentialFolder> {
  return invoke<CredentialFolder>('credential_folder_rename', { folderId, name });
}

export async function credentialFolderMove(folderId: string, parentId: string | null): Promise<CredentialFolder> {
  return invoke<CredentialFolder>('credential_folder_move', { folderId, parentId });
}

export async function credentialFolderSetIcon(folderId: string, icon: string | null, color: string | null): Promise<CredentialFolder> {
  return invoke<CredentialFolder>('credential_folder_set_icon', { folderId, icon, color });
}

/** Delete a folder; its credentials and subfolders move up a level. */
export async function credentialFolderDelete(folderId: string): Promise<void> {
  return invoke('credential_folder_delete', { folderId });
}

/** "Parent / Child" for a credential folder. */
export function credentialFolderPath(id: string, folders: CredentialFolder[], separator = ' / '): string {
  const byId = new Map(folders.map((f) => [f.id, f]));
  const names: string[] = [];
  const seen = new Set<string>();
  let current = byId.get(id);
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    names.unshift(current.name);
    current = current.parent_id ? byId.get(current.parent_id) : undefined;
  }
  return names.join(separator);
}

/** Credentials grouped by their folder's path, for a picker: those at the
 *  top first (label null), then each folder by path; by name inside. A
 *  credential whose folder is gone or in another vault is at the top. */
export function groupByFolder(
  creds: SavedCredential[],
  folders: CredentialFolder[],
): { label: string | null; credentials: SavedCredential[] }[] {
  const byName = (a: SavedCredential, b: SavedCredential) => a.name.localeCompare(b.name);
  const groups = new Map<string | null, SavedCredential[]>();
  for (const c of creds) {
    const f = c.folder_id ? folders.find((x) => x.id === c.folder_id) : undefined;
    const key = f && (f.vault_id ?? null) === (c.vault_id ?? null) ? f.id : null;
    groups.set(key, [...(groups.get(key) ?? []), c]);
  }
  return [...groups.entries()]
    .map(([id, list]) => ({ label: id ? credentialFolderPath(id, folders) : null, credentials: list.sort(byName) }))
    .sort((a, b) => (a.label === null ? -1 : b.label === null ? 1 : a.label.localeCompare(b.label)));
}

/** The credentials a session or folder in `vaultId` can use: same vault only. */
export function credentialsFor(all: SavedCredential[], vaultId: string | null | undefined): SavedCredential[] {
  return all.filter((c) => (c.vault_id ?? null) === (vaultId ?? null));
}

/** What unlocking at start did: keys opened and kept for this run, and keys
 *  left locked (cancelled or a wrong passphrase), which ask at connect. */
export interface KeysUnlocked {
  unlocked: number;
  skipped: number;
  /** Of `unlocked`, opened by passphrases remembered behind the biometric. */
  by_biometric: number;
  /** Passphrases newly remembered behind the biometric. */
  remembered: number;
  remember_error?: string;
}

/** Remember key passphrases with device authentication. Android uses these
 *  texts for opening and remembering. Other systems provide their own. */
export interface KeysBiometric {
  open: PromptText;
  save: PromptText;
}

/** Ask for the passphrase of every passphrase-protected key the saved
 *  credentials use (Settings → Security). The questions arrive through the
 *  login prompt dialog; the passphrases stay in the backend's memory. */
export async function unlockCredentialKeys(biometric?: KeysBiometric): Promise<KeysUnlocked> {
  return invoke<KeysUnlocked>('credential_keys_unlock', { biometric: biometric ?? null });
}

/** Forget the key passphrases remembered behind the biometric. */
export async function forgetBiometricKeys(): Promise<void> {
  return invoke('credential_keys_biometric_forget');
}

/** Tell the backend the setting: off forgets every passphrase kept. */
export async function setKeysUnlockEnabled(enabled: boolean): Promise<void> {
  return invoke('credential_keys_unlock_enabled', { enabled });
}
