/**
 * The local TFTP, FTP, SFTP and HTTP servers (desktop only).
 */

import { invoke } from '@tauri-apps/api/core';

export type ServerKind = 'tftp' | 'ftp' | 'sftp' | 'http';

export const SERVER_KINDS: ServerKind[] = ['tftp', 'ftp', 'sftp', 'http'];

export interface ServerUser {
	username: string;
	password: string;
}

export interface ServerConfig {
	kind: ServerKind;
	root: string;
	bind: string;
	port: number;
	users: ServerUser[];
	anonymous: boolean;
	allowWrite: boolean;
}

export interface LogEntry {
	kind: ServerKind;
	time: string;
	peer: string;
	message: string;
}

export interface ServerStatus {
	kind: ServerKind;
	running: boolean;
	bind: string;
	port: number;
	root: string;
	addresses: string[];
	fingerprint?: string;
	log: LogEntry[];
}

export interface NetInterface {
	name: string;
	ip: string;
	private: boolean;
	loopback: boolean;
}

export type PortProblem = 'in_use' | 'privileged' | 'no_address' | 'other';

export interface PortCheck {
	free: boolean;
	problem?: PortProblem;
	suggestion?: number;
}

export function localServerInterfaces(): Promise<NetInterface[]> {
	return invoke('local_server_interfaces');
}

export function localServerCheckPort(kind: ServerKind, bind: string, port: number): Promise<PortCheck> {
	return invoke('local_server_check_port', { kind, bind, port });
}

export function localServerStart(config: ServerConfig): Promise<ServerStatus> {
	return invoke('local_server_start', { config });
}

export function localServerStop(kind: ServerKind): Promise<ServerStatus> {
	return invoke('local_server_stop', { kind });
}

export function localServerStatus(): Promise<ServerStatus[]> {
	return invoke('local_server_status');
}

export function localServerClearLog(kind: ServerKind): Promise<void> {
	return invoke('local_server_clear_log', { kind });
}

/** The standard port of each kind. */
export const DEFAULT_PORTS: Record<ServerKind, number> = { tftp: 69, ftp: 21, sftp: 22, http: 80 };

export const SERVER_LABELS: Record<ServerKind, string> = { tftp: 'TFTP', ftp: 'FTP', sftp: 'SFTP', http: 'HTTP' };

/**
 * A password for a generated login: 14 letters and digits, without the ones
 * easily misread (0/O, 1/l/I), so it can be typed from the screen.
 */
export function generatePassword(): string {
	const chars = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789';
	const bytes = new Uint32Array(14);
	crypto.getRandomValues(bytes);
	return Array.from(bytes, (b) => chars[b % chars.length]).join('');
}

/** A private (LAN, link-local, VPN) or loopback IPv4 address. */
export function isPrivateIp(ip: string): boolean {
	const [a, b] = ip.split('.').map(Number);
	return (
		a === 10 ||
		a === 127 ||
		(a === 172 && b >= 16 && b < 32) ||
		(a === 192 && b === 168) ||
		(a === 169 && b === 254) ||
		(a === 100 && b >= 64 && b < 128)
	);
}

/** How a client reaches the server, as a URL to copy. */
export function serverUrl(kind: ServerKind, ip: string, port: number, user?: string): string {
	const portPart = port === DEFAULT_PORTS[kind] ? '' : `:${port}`;
	const who = kind === 'sftp' && user ? `${encodeURIComponent(user)}@` : '';
	return `${kind}://${who}${ip}${portPart}/`;
}
