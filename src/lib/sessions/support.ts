import type { SessionConfig } from '#lib/ipc/sessions.js';

/** Where the app runs, as far as which sessions it can open goes. */
export interface Platform {
	mobile: boolean;
	windows: boolean;
}

/**
 * Why this device cannot open a saved session, as an i18n key, or null when
 * it can. Such sessions stay in the list, greyed out, unless Settings >
 * Sessions hides them; they can still be edited, moved and synced.
 */
export function unsupportedReason(session: Pick<SessionConfig, 'kind' | 'files' | 'x11_desktop'>, platform: Platform): string | null {
	if (session.kind === 'wsl') return platform.windows ? null : 'session.wsl_windows_only';
	if (!platform.mobile) return null;
	if (session.kind === 'serial') return 'serial.desktop_only';
	if (session.kind === 'winbox') return 'winbox.desktop_only';
	if (session.x11_desktop) return 'session.x11_desktop_only';
	return null;
}
