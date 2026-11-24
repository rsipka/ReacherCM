/** Shared wording for the server management pages. */
import { t } from '#lib/state/i18n.svelte.js';

/** A health as words and a colour class (`ok`, `warn`, `crit`, `none`). */
export function healthClass(h: string | null | undefined): 'ok' | 'warn' | 'crit' | 'none' {
	if (h === 'OK') return 'ok';
	if (h === 'Warning') return 'warn';
	if (h === 'Critical') return 'crit';
	return 'none';
}

export function healthLabel(h: string | null | undefined): string {
	if (h === 'OK' || h === 'Warning' || h === 'Critical') return t(`bmc.health_${h}`);
	return h || t('bmc.health_unknown');
}

/** A Redfish word (`GracefulShutdown`, `Pxe`) under a key prefix, else itself spaced out. */
export function word(prefix: string, value: string | null | undefined): string {
	if (!value) return '—';
	const key = `${prefix}${value}`;
	const said = t(key);
	return said === key ? value.replace(/([a-z])([A-Z])/g, '$1 $2') : said;
}

/** A BMC time, in this computer's way of writing dates. */
export function when(s: string | null | undefined): string {
	if (!s) return '—';
	const d = new Date(s);
	return Number.isNaN(d.getTime()) ? s : d.toLocaleString();
}

export function num(n: number | null | undefined, digits = 0): string {
	return n === null || n === undefined ? '—' : n.toFixed(digits);
}
