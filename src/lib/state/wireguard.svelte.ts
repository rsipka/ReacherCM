/**
 * WireGuard profiles for the pickers and the Tunnels panel, and bringing a
 * session's tunnel up when it connects: asked first, since turning on a
 * tunnel reaches out to a server the person may not expect.
 */

import { wireguardList, wireguardRunning, wireguardSessionRoute, wireguardUp, type WgProfileInfo } from '#lib/ipc/wireguard.js';
import { askChoice } from '#lib/state/ask.svelte.js';
import { t } from '#lib/state/i18n.svelte.js';

let profiles = $state<WgProfileInfo[]>([]);
let loaded = false;

export function wireguardProfiles(): WgProfileInfo[] {
	if (!loaded) void refreshWireguard();
	return profiles;
}

export async function refreshWireguard(): Promise<WgProfileInfo[]> {
	loaded = true;
	try {
		profiles = await wireguardList();
	} catch {
		// Locked vault: nothing to show yet; asked again next time.
		loaded = false;
	}
	window.dispatchEvent(new CustomEvent('reach:wireguard-changed'));
	return profiles;
}

/** The profiles whose tunnel is on, kept fresh while any is: a tunnel a
 *  connection turned on goes off by itself once unused. */
export function runningWireguard(): WgProfileInfo[] {
	watch();
	return profiles.filter((p) => p.state.running);
}

let watching = false;
function watch(): void {
	if (watching || typeof window === 'undefined') return;
	watching = true;
	if (!loaded) void refreshWireguard();
	window.setInterval(() => {
		if (profiles.some((p) => p.state.running)) void refreshWireguard();
	}, 5000);
}

export function wireguardName(id: string | null | undefined): string {
	if (!id) return '';
	return profiles.find((p) => p.id === id)?.name ?? t('wireguard.missing');
}

/** Tunnels being brought up, so two tabs of the same session ask once. */
const starting = new Map<string, Promise<boolean>>();

/**
 * Makes sure the tunnel of profile `id` is up before a connection through
 * it: nothing to do when it is, else the person is asked and it is brought
 * up. Resolves false when they chose to connect without it (the computer's
 * own VPN is on, or they are on the same network). Rejects when they
 * cancel or the server does not answer.
 */
export function ensureWireguard(id: string): Promise<boolean> {
	const already = starting.get(id);
	if (already) return already;
	const job = (async () => {
		const running = await wireguardRunning().catch(() => [] as string[]);
		if (running.includes(id)) return true;
		if (!loaded || !profiles.some((p) => p.id === id)) await refreshWireguard();
		const p = profiles.find((x) => x.id === id);
		if (!p) throw new Error(t('wireguard.missing_error'));
		const choice = await askChoice(t('wireguard.confirm_up', { name: p.name, endpoint: p.endpoint ?? '' }), {
			confirmLabel: t('wireguard.turn_on'),
			confirmVariant: 'primary',
			otherLabel: t('wireguard.connect_direct')
		});
		if (choice === 'other') return false;
		if (choice === 'cancel') throw new Error(t('wireguard.declined', { name: p.name }));
		await wireguardUp(id, true);
		await refreshWireguard();
		return true;
	})();
	starting.set(id, job);
	job.finally(() => starting.delete(id)).catch(() => {});
	return job;
}

/**
 * For connections whose route the backend reads from the saved session
 * (RDP, VNC, SPICE, web, WinBox): turns that session's tunnel on first,
 * after asking, when it goes through one.
 */
export async function ensureSessionWireguard(sessionId: string | null | undefined, host?: string, port?: number): Promise<void> {
	if (!sessionId) return;
	const id = await wireguardSessionRoute(sessionId, host, port).catch(() => null);
	// Without the tunnel: the backend connects straight when it is off.
	if (id) await ensureWireguard(id);
}
