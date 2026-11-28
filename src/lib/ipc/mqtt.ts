import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

/** A connect the broker refused without a password: ask for one. */
export const MQTT_PASSWORD_NEEDED = 'MQTT_PASSWORD_NEEDED';
/** The broker refused the login given; why follows. */
export const MQTT_LOGIN_REFUSED = 'MQTT_LOGIN_REFUSED:';
/** A connect that met a certificate nothing trusts; the fingerprint follows. */
export const MQTT_UNTRUSTED_CERT = 'MQTT_UNTRUSTED_CERT:';

/** An MQTT session's own settings, as stored. */
export interface MqttOptions {
	/** 'tcp' (unset), 'tls', 'ws' or 'wss'. */
	transport?: string | null;
	ws_path?: string | null;
	client_id?: string | null;
	keep_alive?: number | null;
	/** Topic filters subscribed to on connect; `#` when none. */
	subscriptions?: string[];
	/** PEM client certificate (with its chain) and key. */
	client_cert?: string | null;
	client_key?: string | null;
	/** Trusted certificate fingerprints, read only here. */
	certs?: string[];
}

/** The port each transport usually listens on. */
export const MQTT_PORTS: Record<string, number> = { tcp: 1883, tls: 8883, ws: 80, wss: 443 };

/** One message, as the backend hands it over. */
export interface MqttMessage {
	topic: string;
	/** Base64, at most 256 KiB of it. */
	payload: string;
	/** The whole payload's size. */
	size: number;
	qos: number;
	retain: boolean;
	/** Milliseconds since 1970. */
	time: number;
}

export type MqttEvent = { type: 'messages'; messages: MqttMessage[] } | { type: 'closed'; error: string | null };

/** Hear what a connection receives; call before connecting it. */
export function onMqttEvent(id: string, handler: (e: MqttEvent) => void): Promise<UnlistenFn> {
	return listen<MqttEvent>(`mqtt-event-${id}`, (e) => handler(e.payload));
}

/** Connect a saved MQTT session under `id`. */
export async function mqttConnect(id: string, sessionId: string, password?: string): Promise<{ clientId: string; filters: string[] }> {
	await ensureSessionWireguard(sessionId);
	return invoke('mqtt_connect', { id, sessionId, password: password || null });
}

export async function mqttDisconnect(connectionId: string): Promise<void> {
	return invoke('mqtt_disconnect', { connectionId });
}

/** Publish `payload` (bytes) to a topic. */
export async function mqttPublish(connectionId: string, topic: string, payload: Uint8Array, qos: number, retain: boolean): Promise<void> {
	return invoke('mqtt_publish', { connectionId, topic, payload: bytesToBase64(payload), qos, retain });
}

/** Subscribe; the QoS granted per filter (128: refused). */
export async function mqttSubscribe(connectionId: string, filters: string[], qos = 1): Promise<number[]> {
	return invoke('mqtt_subscribe', { connectionId, filters, qos });
}

export async function mqttUnsubscribe(connectionId: string, filters: string[]): Promise<void> {
	return invoke('mqtt_unsubscribe', { connectionId, filters });
}

/** Trust a broker certificate (by its SHA-256 fingerprint) for a session. */
export async function mqttTrustCert(sessionId: string, fingerprint: string): Promise<void> {
	return invoke('mqtt_session_cert_trust', { sessionId, fingerprint });
}

export function bytesToBase64(bytes: Uint8Array): string {
	let s = '';
	for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
	return btoa(s);
}

export function base64ToBytes(b64: string): Uint8Array {
	const s = atob(b64);
	const out = new Uint8Array(s.length);
	for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
	return out;
}
