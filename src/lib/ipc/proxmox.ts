import { invoke } from '@tauri-apps/api/core';

/** Proxmox sessions: a Proxmox VE node or cluster over its API. See
 *  src-tauri/src/proxmox. Every page call names the tab's connection id. */

/** A connect that needs a password typed in. */
export const PVE_PASSWORD_NEEDED = 'PVE_PASSWORD_NEEDED';
/** The account has a second factor: a TOTP code or recovery key is needed. */
export const PVE_TFA_NEEDED = 'PVE_TFA_NEEDED';
/** A console needs a user login; the connection has only an API token. */
export const PVE_CONSOLE_LOGIN = 'PVE_CONSOLE_LOGIN';
/** The server refused the login; its message follows. */
export const PVE_LOGIN_REFUSED = 'PVE_LOGIN_REFUSED:';
/** A connect met a certificate nothing trusts; the fingerprint follows. */
export const PVE_UNTRUSTED_CERT = 'PVE_UNTRUSTED_CERT:';

/** A Proxmox session's own settings (snake_case, as stored). */
export interface ProxmoxOptions {
	/** Trusted certificate fingerprints; changed only by the prompt. */
	certs?: string[];
	/** `pam`, `pve`, `pbs` or a directory's name. */
	realm?: string | null;
	/** An API token for the pages: `user@realm!name`. */
	token_id?: string | null;
	token_secret?: string | null;
}

export interface ProxmoxOpened {
	product: 'pve' | 'pbs';
	version: string | null;
	/** Logged in with an API token: the consoles need a user login too. */
	token: boolean;
	user: string;
}

export interface ProxmoxResource {
	/** `node`, `qemu`, `lxc`, `storage`, `pool`, `sdn`. */
	kind: string;
	id: string;
	node: string | null;
	vmid: number | null;
	name: string | null;
	status: string | null;
	cpu: number | null;
	maxcpu: number | null;
	mem: number | null;
	maxmem: number | null;
	disk: number | null;
	maxdisk: number | null;
	uptime: number | null;
	tags: string[];
	template: boolean;
	lock: string | null;
	hastate: string | null;
	pool: string | null;
	storage: string | null;
	plugintype: string | null;
	content: string | null;
	shared: boolean;
}

export interface ProxmoxNode {
	name: string;
	online: boolean;
	ip: string | null;
	pveVersion: string | null;
	kernel: string | null;
	cpuModel: string | null;
	cpus: number | null;
	sockets: number | null;
	cpu: number | null;
	load: number[];
	memUsed: number | null;
	memTotal: number | null;
	swapUsed: number | null;
	swapTotal: number | null;
	rootUsed: number | null;
	rootTotal: number | null;
	uptime: number | null;
	bootMode: string | null;
}

export interface ProxmoxOverview {
	product: string;
	version: string | null;
	release: string | null;
	clusterName: string | null;
	quorate: boolean | null;
	nodes: ProxmoxNode[];
}

export interface ProxmoxSnapshot {
	name: string;
	description: string | null;
	time: number | null;
	parent: string | null;
	vmstate: boolean;
}

export interface ProxmoxGuest {
	node: string;
	kind: 'qemu' | 'lxc';
	vmid: number;
	name: string | null;
	status: string | null;
	qmpstatus: string | null;
	uptime: number | null;
	cpu: number | null;
	cpus: number | null;
	mem: number | null;
	maxmem: number | null;
	disk: number | null;
	maxdisk: number | null;
	ostype: string | null;
	onboot: boolean;
	lock: string | null;
	ha: string | null;
	tags: string[];
	description: string | null;
	agent: boolean;
	/** The display is SPICE (qxl): the SPICE console works. */
	spice: boolean;
	/** Serial ports set up, for the text console (`serial0`...). */
	serials: string[];
	addresses: string[];
	snapshots: ProxmoxSnapshot[];
	template: boolean;
}

export interface ProxmoxTask {
	upid: string;
	node: string;
	kind: string;
	id: string | null;
	user: string | null;
	start: number | null;
	end: number | null;
	status: string | null;
	running: boolean;
}

export interface ProxmoxTaskLog {
	lines: string[];
	running: boolean;
	exitstatus: string | null;
	next: number;
}

/** A console a tab opens (see ConsoleTarget in src-tauri/src/proxmox). */
export interface ProxmoxConsole {
	sessionId: string;
	node: string;
	/** `qemu`, `lxc`, or `node` for a node's shell. */
	kind: 'qemu' | 'lxc' | 'node';
	vmid?: number | null;
	serial?: string | null;
}

export function proxmoxConnect(id: string, sessionId: string, password?: string, otp?: string): Promise<ProxmoxOpened> {
	return invoke('proxmox_connect', { id, sessionId, password: password || null, otp: otp || null });
}

export function proxmoxDisconnect(connectionId: string): Promise<void> {
	return invoke('proxmox_disconnect', { connectionId });
}

export function proxmoxConsoleReady(connectionId: string): Promise<boolean> {
	return invoke('proxmox_console_ready', { connectionId });
}

export function proxmoxConsoleLogin(connectionId: string, password?: string, otp?: string): Promise<void> {
	return invoke('proxmox_console_login', { connectionId, password: password || null, otp: otp || null });
}

export function proxmoxTrustCert(sessionId: string, fingerprint: string): Promise<void> {
	return invoke('proxmox_session_cert_trust', { sessionId, fingerprint });
}

export function proxmoxOverview(connectionId: string): Promise<ProxmoxOverview> {
	return invoke('proxmox_overview', { connectionId });
}

export function proxmoxResources(connectionId: string): Promise<ProxmoxResource[]> {
	return invoke('proxmox_resources', { connectionId });
}

export function proxmoxGuest(connectionId: string, node: string, kind: string, vmid: number): Promise<ProxmoxGuest> {
	return invoke('proxmox_guest', { connectionId, node, kind, vmid });
}

export function proxmoxGuestAction(connectionId: string, node: string, kind: string, vmid: number, action: string): Promise<string> {
	return invoke('proxmox_guest_action', { connectionId, node, kind, vmid, action });
}

export function proxmoxSnapshotCreate(connectionId: string, node: string, kind: string, vmid: number, name: string, description: string, vmstate: boolean): Promise<string> {
	return invoke('proxmox_snapshot_create', { connectionId, node, kind, vmid, name, description, vmstate });
}

export function proxmoxSnapshotRollback(connectionId: string, node: string, kind: string, vmid: number, name: string): Promise<string> {
	return invoke('proxmox_snapshot_rollback', { connectionId, node, kind, vmid, name });
}

export function proxmoxSnapshotDelete(connectionId: string, node: string, kind: string, vmid: number, name: string): Promise<string> {
	return invoke('proxmox_snapshot_delete', { connectionId, node, kind, vmid, name });
}

export function proxmoxNodeAction(connectionId: string, node: string, command: 'reboot' | 'shutdown'): Promise<void> {
	return invoke('proxmox_node_action', { connectionId, node, command });
}

export function proxmoxTasks(connectionId: string): Promise<ProxmoxTask[]> {
	return invoke('proxmox_tasks', { connectionId });
}

export function proxmoxTaskLog(connectionId: string, node: string, upid: string, start: number): Promise<ProxmoxTaskLog> {
	return invoke('proxmox_task_log', { connectionId, node, upid, start });
}

export function proxmoxTaskStop(connectionId: string, node: string, upid: string): Promise<void> {
	return invoke('proxmox_task_stop', { connectionId, node, upid });
}
