/**
 * The text console of a server management session (its tab > Text console):
 * SSH to the controller itself, logged in to as the Redfish session logs
 * in, then its serial-over-LAN command typed (`console com2` on an iDRAC,
 * `vsp` on an iLO...). The backend lets the session's password manager
 * entries go to this connect only when it is to the same controller as
 * the same user (see `check_matches_session` in
 * src-tauri/src/ipc/ssh_commands.rs).
 */
import type { SessionConfig } from '#lib/ipc/sessions.js';

/** The controller's SSH port unless the person names another. */
export const BMC_SSH_PORT = 22;

/** The session as an SSH session to connect, never saved: same id (its
 *  password manager entries, the host keys it trusts), host, user and
 *  password, the SSH port, and nothing of a shell's own setup. */
export function bmcConsoleSession(session: SessionConfig, port: number): SessionConfig {
	return {
		...session,
		kind: 'ssh',
		port: port || BMC_SSH_PORT,
		files: null,
		jump_chain: null,
		proxy: null,
		alt_addresses: null,
		connect_mode: null,
		ssh_options: null,
		shell: null,
		tmux: { mode: 'off' },
		x11_desktop: null,
		try_agent_keys: null,
		share_path: null,
		monitoring: false,
		bmc: null,
	};
}

/** The last port and command used for a session's text console, kept on
 *  this computer only. */
export function rememberedConsole(sessionId: string): { port: number; command: string } | null {
	try {
		const raw = localStorage.getItem(`reach-bmc-console:${sessionId}`);
		if (!raw) return null;
		const v = JSON.parse(raw);
		return typeof v?.port === 'number' && typeof v?.command === 'string' ? { port: v.port, command: v.command } : null;
	} catch {
		return null;
	}
}

export function rememberConsole(sessionId: string, port: number, command: string): void {
	try {
		localStorage.setItem(`reach-bmc-console:${sessionId}`, JSON.stringify({ port, command }));
	} catch {
		// Remembered next time, or not.
	}
}
