/** Shared wording and numbers for the Proxmox pages. */
import { t } from '#lib/state/i18n.svelte.js';

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];

/** Bytes as people read them: `3.2 GiB`. */
export function bytes(n: number | null | undefined): string {
	if (n === null || n === undefined || !Number.isFinite(n)) return '—';
	let v = n;
	let i = 0;
	while (v >= 1024 && i < UNITS.length - 1) {
		v /= 1024;
		i++;
	}
	return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${UNITS[i]}`;
}

/** Used of total as a share, 0 to 100; null when either is missing. */
export function share(used: number | null | undefined, total: number | null | undefined): number | null {
	if (used === null || used === undefined || !total) return null;
	return Math.max(0, Math.min(100, (used / total) * 100));
}

/** A CPU fraction (0 to 1) as a percentage. */
export function cpu(n: number | null | undefined): string {
	return n === null || n === undefined ? '—' : `${(n * 100).toFixed(n < 0.1 ? 1 : 0)} %`;
}

/** Seconds as `3d 4h`, `5h 12m`, `7m`. */
export function uptime(s: number | null | undefined): string {
	if (!s) return '—';
	const d = Math.floor(s / 86400);
	const h = Math.floor((s % 86400) / 3600);
	const m = Math.floor((s % 3600) / 60);
	if (d > 0) return `${d}d ${h}h`;
	if (h > 0) return `${h}h ${m}m`;
	return `${Math.max(m, 1)}m`;
}

/** A Unix time in this computer's way of writing dates. */
export function when(s: number | null | undefined): string {
	if (!s) return '—';
	return new Date(s * 1000).toLocaleString();
}

/** A guest's or node's state as a colour class. */
export function stateClass(status: string | null | undefined): 'ok' | 'warn' | 'crit' | 'none' {
	if (status === 'running' || status === 'online' || status === 'available' || status === 'OK') return 'ok';
	if (status === 'paused' || status === 'suspended' || status === 'unknown') return 'warn';
	if (status === 'offline' || status === 'unavailable') return 'crit';
	return 'none';
}

/** A state in words, under `proxmox.state_<state>`, else as Proxmox wrote it. */
export function stateLabel(status: string | null | undefined): string {
	if (!status) return '—';
	const key = `proxmox.state_${status}`;
	const said = t(key);
	return said === key ? status : said;
}

/** A task type (`qmstart`, `vzdump`) in words, else as Proxmox wrote it. */
export function taskLabel(kind: string): string {
	const key = `proxmox.task_${kind}`;
	const said = t(key);
	return said === key ? kind : said;
}

/** A failed call as the user reads it. */
export function proxmoxError(err: unknown): string {
	const msg = String(err);
	if (msg === 'PVE_CONSOLE_LOGIN') return t('proxmox.console_login_needed');
	if (msg.startsWith('PVE_LOGIN_REFUSED:')) return `${t('proxmox.login_refused')} ${msg.slice('PVE_LOGIN_REFUSED:'.length)}`.trim();
	return msg;
}
