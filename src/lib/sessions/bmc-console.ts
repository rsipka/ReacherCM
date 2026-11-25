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

/** An iDRAC's VNC server port unless set otherwise (VNCServer.1.Port). */
export const BMC_VNC_PORT = 5901;

/** The session as a VNC session to its controller, never saved: same id,
 *  host, the VNC port and the VNC password typed for it. */
export function bmcVncSession(session: SessionConfig, port: number, password: string): SessionConfig {
	return {
		...session,
		kind: 'vnc',
		port: port || BMC_VNC_PORT,
		username: '',
		auth_method: { type: 'Password', password, password_ref: null },
		via_session_id: null,
		jump_chain: null,
		proxy: null,
		alt_addresses: null,
		bmc: null,
	};
}

/** The VNC port last used for a session, kept on this computer only. */
export function rememberedVncPort(sessionId: string): number | null {
	try {
		const v = Number.parseInt(localStorage.getItem(`reach-bmc-vnc:${sessionId}`) ?? '', 10);
		return v > 0 && v < 65536 ? v : null;
	} catch {
		return null;
	}
}

export function rememberVncPort(sessionId: string, port: number, tls: boolean): void {
	try {
		localStorage.setItem(`reach-bmc-vnc:${sessionId}`, String(port));
		localStorage.setItem(`reach-bmc-vnc-tls:${sessionId}`, tls ? '1' : '0');
	} catch {
		// Remembered next time, or not.
	}
}

/** Whether the VNC server wants TLS (an iDRAC with SSL encryption on, the
 *  iDRAC's default): on unless turned off last time. */
export function rememberedVncTls(sessionId: string): boolean {
	try {
		return localStorage.getItem(`reach-bmc-vnc-tls:${sessionId}`) !== '0';
	} catch {
		return true;
	}
}
