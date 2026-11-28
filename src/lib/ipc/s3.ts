import { invoke } from '@tauri-apps/api/core';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

/** A connect with no secret key saved: ask for one. */
export const S3_SECRET_NEEDED = 'S3_SECRET_NEEDED';
/** The store refused the keys; why follows. */
export const S3_LOGIN_REFUSED = 'S3_LOGIN_REFUSED:';
/** A connect that met a certificate nothing trusts; the fingerprint follows. */
export const S3_UNTRUSTED_CERT = 'S3_UNTRUSTED_CERT:';

/** An S3 session's own settings. The access key is its user name, the
 *  secret key its password. */
export interface S3Options {
	/** Signed for; from the host name unless set. */
	region?: string | null;
	/** `host/bucket` (true) or `bucket.host` (false); unset: automatic. */
	path_style?: boolean | null;
	/** The bucket it opens in. */
	bucket?: string | null;
	/** Plain HTTP (a store on this network). */
	http?: boolean;
	/** Trusted certificate fingerprints, read only here. */
	certs?: string[];
}

/** Well-known stores, to fill in the host. `{region}` / `{account}` are
 *  for the user to replace. */
export const S3_PROVIDERS: { id: string; label: string; host: string; port: number; http?: boolean }[] = [
	{ id: 'aws', label: 'Amazon S3', host: 's3.us-east-1.amazonaws.com', port: 443 },
	{ id: 'minio', label: 'MinIO', host: 'minio.example.com', port: 9000, http: true },
	{ id: 'r2', label: 'Cloudflare R2', host: 'ACCOUNT_ID.r2.cloudflarestorage.com', port: 443 },
	{ id: 'b2', label: 'Backblaze B2', host: 's3.us-west-004.backblazeb2.com', port: 443 },
	{ id: 'wasabi', label: 'Wasabi', host: 's3.eu-central-1.wasabisys.com', port: 443 },
];

/** Connect a saved S3 session under `id`, for the file manager. Resolves
 *  to the folder it opens in (`/` lists the buckets). */
export async function s3Connect(id: string, sessionId: string, secret?: string): Promise<{ home: string }> {
	await ensureSessionWireguard(sessionId);
	return invoke('s3_connect', { id, sessionId, secret: secret || null });
}

export async function s3Disconnect(connectionId: string): Promise<void> {
	return invoke('s3_disconnect', { connectionId });
}

/** A link that downloads `path` (`/bucket/key`) without keys for `expires` seconds. */
export async function s3Presign(connectionId: string, path: string, expires: number): Promise<string> {
	return invoke('s3_presign', { connectionId, path, expires });
}

/** Trust a certificate (by its SHA-256 fingerprint) for an S3 session. */
export async function s3TrustCert(sessionId: string, fingerprint: string): Promise<void> {
	return invoke('s3_session_cert_trust', { sessionId, fingerprint });
}
