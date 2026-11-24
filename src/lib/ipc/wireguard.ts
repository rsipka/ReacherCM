import { invoke } from '@tauri-apps/api/core';

/** A running tunnel's state (see src-tauri/src/wireguard/manager.rs). */
export interface WgTunnelState {
	running: boolean;
	/** Seconds since the last handshake with the server. */
	handshake_secs?: number | null;
	up_secs?: number | null;
	tx_bytes: number;
	rx_bytes: number;
	connections: number;
	error?: string | null;
}

/** A saved profile as the list shows it: no private key. */
export interface WgProfileInfo {
	id: string;
	name: string;
	endpoint?: string | null;
	addresses: string[];
	dns: string[];
	allowed_ips: string[];
	public_key: string;
	/** The file no longer reads as a WireGuard profile. */
	invalid?: string | null;
	state: WgTunnelState;
}

/** The whole profile, with the wg-quick file it came from. */
export interface WgProfile {
	id: string;
	name: string;
	conf: string;
}

export function wireguardList(): Promise<WgProfileInfo[]> {
	return invoke<WgProfileInfo[]>('wireguard_list');
}

export function wireguardGet(id: string): Promise<WgProfile> {
	return invoke<WgProfile>('wireguard_get', { id });
}

/** Rejects with what is wrong with the file. */
export function wireguardCheck(conf: string): Promise<void> {
	return invoke('wireguard_check', { conf });
}

export function wireguardSave(id: string | null, name: string, conf: string): Promise<WgProfileInfo> {
	return invoke<WgProfileInfo>('wireguard_save', { id, name, conf });
}

export function wireguardDelete(id: string): Promise<void> {
	return invoke('wireguard_delete', { id });
}

/** Brings the tunnel up and waits for the server's answer. */
export function wireguardUp(id: string): Promise<WgProfileInfo> {
	return invoke<WgProfileInfo>('wireguard_up', { id });
}

export function wireguardDown(id: string): Promise<void> {
	return invoke('wireguard_down', { id });
}

export function wireguardRunning(): Promise<string[]> {
	return invoke<string[]>('wireguard_running');
}

/** [private, public], base64 as `wg genkey` / `wg pubkey` print them. */
export function wireguardGenerateKey(): Promise<[string, string]> {
	return invoke<[string, string]>('wireguard_generate_key');
}
