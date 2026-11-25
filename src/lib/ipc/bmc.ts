import type { BmcVnc } from '#lib/ipc/sessions.js';
import { invoke } from '@tauri-apps/api/core';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

/** Server management sessions: a server's BMC (iDRAC, iLO...) over Redfish.
 *  See src-tauri/src/bmc. Every page call names the tab's connection id. */

/** A connect that needs a password typed in. */
export const BMC_PASSWORD_NEEDED = 'BMC_PASSWORD_NEEDED';
/** The BMC refused the login; its message follows. */
export const BMC_LOGIN_REFUSED = 'BMC_LOGIN_REFUSED:';
/** A connect that met a certificate nothing trusts; the fingerprint follows. */
export const BMC_UNTRUSTED_CERT = 'BMC_UNTRUSTED_CERT:';
/** The BMC needs a licence for what was asked; its message follows. */
export const BMC_LICENCE = 'BMC_LICENCE:';

/** `OK`, `Warning` or `Critical` (Redfish's Status.Health). */
export type Health = 'OK' | 'Warning' | 'Critical' | string;

export interface BmcOpened {
	vendor: 'dell' | 'hpe' | 'supermicro' | 'lenovo' | 'fujitsu' | 'cisco' | 'huawei' | 'openbmc' | 'asrock' | 'ami' | 'other';
	product: string | null;
	/** Typed after an SSH login for the serial console; empty: none known. */
	consoleCommand: string;
}

export interface BmcOverview {
	vendor: string;
	product: string | null;
	redfishVersion: string | null;
	model: string | null;
	manufacturer: string | null;
	serial: string | null;
	sku: string | null;
	biosVersion: string | null;
	hostName: string | null;
	powerState: string | null;
	health: Health | null;
	healthRollup: Health | null;
	cpuCount: number | null;
	cpuModel: string | null;
	memoryGib: number | null;
	identify: boolean | null;
	bmcModel: string | null;
	bmcFirmware: string | null;
	bmcTime: string | null;
	powerWatts: number | null;
	inletTemp: number | null;
	resetTypes: string[];
	bootTargets: string[];
	bootTarget: string | null;
	bootEnabled: string | null;
	canResetBmc: boolean;
}

export interface BmcSensor {
	name: string;
	reading: number | null;
	units: string;
	health: Health | null;
	state: string | null;
	warning: number | null;
	critical: number | null;
}

/** One line of a list: a part and what it is. */
export interface BmcItem {
	name: string;
	detail: string;
	health: Health | null;
	state: string | null;
}

export interface BmcSensors {
	temperatures: BmcSensor[];
	fans: BmcSensor[];
	voltages: BmcSensor[];
	powerSupplies: BmcItem[];
	powerWatts: number | null;
}

export interface BmcHardware {
	processors: BmcItem[];
	memory: BmcItem[];
	storage: BmcItem[];
	drives: BmcItem[];
	volumes: BmcItem[];
	network: BmcItem[];
	firmware: BmcItem[];
}

export interface BmcLogService {
	path: string;
	name: string;
	canClear: boolean;
}

export interface BmcLogEntry {
	id: string;
	created: string | null;
	severity: Health | null;
	message: string;
	messageId: string | null;
	sensor: string | null;
}

export interface BmcJob {
	id: string;
	name: string;
	state: string | null;
	percent: number | null;
	message: string | null;
	start: string | null;
	end: string | null;
}

export interface BmcVirtualMedia {
	path: string;
	name: string;
	mediaTypes: string[];
	image: string | null;
	inserted: boolean;
}

export async function bmcConnect(id: string, sessionId: string, password?: string): Promise<BmcOpened> {
	await ensureSessionWireguard(sessionId);
	return invoke('bmc_connect', { id, sessionId, password: password || null });
}

export async function bmcDisconnect(connectionId: string): Promise<void> {
	return invoke('bmc_disconnect', { connectionId });
}

/** Trust a certificate (by its SHA-256 fingerprint) for a session. */
export async function bmcTrustCert(sessionId: string, fingerprint: string): Promise<void> {
	return invoke('bmc_session_cert_trust', { sessionId, fingerprint });
}

/** Save how a session reaches its controller's VNC server (`null` forgets it). */
export async function bmcSaveVnc(sessionId: string, vnc: BmcVnc | null): Promise<void> {
	return invoke('bmc_session_vnc_save', { sessionId, vnc });
}

export const bmcOverview = (connectionId: string) => invoke<BmcOverview>('bmc_overview', { connectionId });
export const bmcSensors = (connectionId: string) => invoke<BmcSensors>('bmc_sensors', { connectionId });
export const bmcHardware = (connectionId: string) => invoke<BmcHardware>('bmc_hardware', { connectionId });
export const bmcLogServices = (connectionId: string) => invoke<BmcLogService[]>('bmc_log_services', { connectionId });
export const bmcLogEntries = (connectionId: string, path: string) => invoke<BmcLogEntry[]>('bmc_log_entries', { connectionId, path });
export const bmcLogClear = (connectionId: string, path: string) => invoke<void>('bmc_log_clear', { connectionId, path });
export const bmcJobs = (connectionId: string) => invoke<BmcJob[]>('bmc_jobs', { connectionId });
export const bmcVirtualMedia = (connectionId: string) => invoke<BmcVirtualMedia[]>('bmc_virtual_media', { connectionId });
/** Press the power button; resolves to the BMC's message, if any. */
export const bmcPower = (connectionId: string, resetType: string) => invoke<string | null>('bmc_power', { connectionId, resetType });
/** Boot once from `target`; `None` cancels. */
export const bmcBootOnce = (connectionId: string, target: string) => invoke<void>('bmc_boot_once', { connectionId, target });
export const bmcIdentify = (connectionId: string, on: boolean) => invoke<void>('bmc_identify', { connectionId, on });
export const bmcMediaInsert = (connectionId: string, path: string, image: string) => invoke<void>('bmc_media_insert', { connectionId, path, image });
export const bmcMediaEject = (connectionId: string, path: string) => invoke<void>('bmc_media_eject', { connectionId, path });
/** Restart the BMC; its login ends, so the tab connects again later. */
export const bmcResetBmc = (connectionId: string) => invoke<void>('bmc_reset_bmc', { connectionId });
/** Open the controller's remote console in a web window of its own
 *  (desktop): the iDRAC's HTML5 console, or the others' web interface. */
export const bmcRemoteConsole = (connectionId: string, sessionId: string) => invoke<string>('bmc_remote_console', { connectionId, sessionId });
