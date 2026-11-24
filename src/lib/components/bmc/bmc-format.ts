/** Shared wording for the server management pages. */
import { t } from '#lib/state/i18n.svelte.js';
import { BMC_LICENCE } from '#lib/ipc/bmc.js';

/** A failed call as the user reads it: a missing licence said plainly. */
export function bmcError(err: unknown): string {
	const msg = String(err);
	return msg.startsWith(BMC_LICENCE) ? t('bmc.licence_needed', { message: msg.slice(BMC_LICENCE.length) || '—' }) : msg;
}

/** The kind of controller, for a BMC that does not name its product. */
const VENDORS: Record<string, string> = {
	dell: 'Dell iDRAC',
	hpe: 'HPE iLO',
	lenovo: 'Lenovo XClarity Controller',
	supermicro: 'Supermicro BMC',
	fujitsu: 'Fujitsu iRMC',
	cisco: 'Cisco CIMC',
	huawei: 'Huawei iBMC',
	openbmc: 'OpenBMC',
	asrock: 'ASRock Rack BMC',
	ami: 'AMI MegaRAC'
};

export function vendorName(vendor: string | null | undefined): string | null {
	return (vendor && VENDORS[vendor]) || null;
}

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
