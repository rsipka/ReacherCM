import { invoke } from '@tauri-apps/api/core';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

/** A connect that needs a password typed in. */
export const FTP_PASSWORD_NEEDED = 'FTP_PASSWORD_NEEDED';
/** A connect that met a certificate nothing trusts; the fingerprint follows. */
export const FTP_UNTRUSTED_CERT = 'FTP_UNTRUSTED_CERT:';

/** Connect a saved FTP session under `id`, for the file manager. The
 *  address, login and trusted certificates are the stored session's;
 *  `password` is one typed in for this connect. Resolves to the folder
 *  the login starts in. */
export async function ftpConnect(id: string, sessionId: string, password?: string): Promise<{ home: string }> {
  await ensureSessionWireguard(sessionId);
  return invoke('ftp_connect', { id, sessionId, password: password || null });
}

export async function ftpDisconnect(connectionId: string): Promise<void> {
  return invoke('ftp_disconnect', { connectionId });
}

/** Trust a certificate (by its SHA-256 fingerprint) for an FTP session. */
export async function ftpTrustCert(sessionId: string, fingerprint: string): Promise<void> {
  return invoke('ftp_session_cert_trust', { sessionId, fingerprint });
}

/** Make an SSH connection's files go over SCP (an SCP session). */
export async function sshFilesScp(connectionId: string): Promise<void> {
  return invoke('ssh_files_scp', { connectionId });
}
