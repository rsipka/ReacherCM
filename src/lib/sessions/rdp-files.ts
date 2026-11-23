/**
 * The file manager of an RDP session (session menu > Open file manager):
 * the same machine over SFTP, logged in to as the RDP session logs in.
 * Windows has an SFTP server in its OpenSSH Server feature; Linux desktops
 * served by xrdp or GNOME Remote Desktop usually run sshd anyway.
 *
 * The backend lets the session's password manager entries go to this
 * connect only when it matches what is built here (see
 * `check_matches_session` in src-tauri/src/ipc/ssh_commands.rs).
 */
import type { SessionConfig } from '#lib/ipc/sessions.js';

/** The SSH port used when the session names none. */
export const RDP_FILES_PORT = 22;

/** The SSH login: the RDP user, behind the RDP domain as Windows OpenSSH
 *  takes it (`DOMAIN\user`), unless the user name already names one.
 *  Mirrors `rdp_files_username` in the backend. */
export function rdpFilesUsername(session: Pick<SessionConfig, 'username' | 'domain'>): string {
	const user = session.username.trim();
	const domain = session.domain?.trim();
	return domain && !/[\\@]/.test(user) ? `${domain}\\${user}` : user;
}

export function rdpFilesPort(session: Pick<SessionConfig, 'rdp'>): number {
	return session.rdp?.files_port || RDP_FILES_PORT;
}

/** The RDP session as an SFTP session to connect, never saved: same id
 *  (its password manager entries and the server keys it trusts), same
 *  host and password, the files port, and nothing of RDP's own. */
export function rdpFilesSession(session: SessionConfig): SessionConfig {
	return {
		...session,
		kind: 'ssh',
		port: rdpFilesPort(session),
		username: rdpFilesUsername(session),
		files: 'sftp',
		jump_chain: null,
		proxy: null,
		alt_addresses: null,
		connect_mode: null,
		ssh_options: null,
		shell: null,
		tmux: null,
		x11_desktop: null,
		try_agent_keys: null,
		share_path: null,
		rdp: null,
	};
}
