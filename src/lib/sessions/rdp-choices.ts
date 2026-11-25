/**
 * The choices the session editor and the bulk editor offer for an RDP
 * session's display, keyboard and idle settings.
 */

import { t } from '#lib/state/i18n.svelte.js';

export const RDP_RESOLUTIONS = ['1280x720', '1280x1024', '1366x768', '1600x900', '1920x1080', '1920x1200', '2560x1440', '3840x2160'];
export const RDP_SCALES = ['100', '125', '150', '175', '200', '250', '300'];
/** Seconds; shown as seconds or minutes. */
export const RDP_INTERVALS = ['30', '60', '120', '240', '540'];
/** Windows keyboard layout ids, each named in its own language. */
export const RDP_KEYBOARDS: [string, string][] = [
	['0409', 'English (US)'],
	['0809', 'English (UK)'],
	['0405', 'Čeština'],
	['041B', 'Slovenčina'],
	['0407', 'Deutsch'],
	['0807', 'Deutsch (Schweiz)'],
	['040C', 'Français'],
	['080C', 'Français (Belgique)'],
	['0410', 'Italiano'],
	['040A', 'Español'],
	['0816', 'Português'],
	['0416', 'Português (Brasil)'],
	['0413', 'Nederlands'],
	['0415', 'Polski'],
	['040E', 'Magyar'],
	['0406', 'Dansk'],
	['0414', 'Norsk'],
	['041D', 'Svenska'],
	['040B', 'Suomi'],
	['0408', 'Ελληνικά'],
	['0402', 'Български'],
	['0419', 'Русский'],
	['0422', 'Українська'],
	['041F', 'Türkçe'],
	['0411', '日本語'],
	['0412', '한국어'],
	['0804', '中文 (简体)'],
];

/** "Every 30 s", "Every 4 min". */
export const rdpIntervalLabel = (secs: string): string =>
	Number(secs) < 120 ? t('session.rdp_interval_secs', { count: secs }) : t('session.rdp_interval_mins', { count: String(Number(secs) / 60) });
