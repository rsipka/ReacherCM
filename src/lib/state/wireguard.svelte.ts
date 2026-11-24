/**
 * WireGuard profiles for the pickers and the Tunnels panel, and bringing a
 * session's tunnel up when it connects: asked first, since turning on a
 * tunnel reaches out to a server the person may not expect.
 */

import { wireguardList, wireguardRunning, wireguardUp, type WgProfileInfo } from '#lib/ipc/wireguard.js';
import { ask } from '#lib/state/ask.svelte.js';
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
		// Locked vault: nothing to show yet.
	}
	window.dispatchEvent(new CustomEvent('reach:wireguard-changed'));
	return profiles;
}

export function wireguardName(id: string | null | undefined): string {
	if (!id) return '';
	return profiles.find((p) => p.id === id)?.name ?? t('wireguard.missing');
}

/** Tunnels being brought up, so two tabs of the same session ask once. */
const starting = new Map<string, Promise<void>>();

/**
 * Makes sure the tunnel of profile `id` is up before a connection through
 * it: nothing to do when it is, else the person is asked and it is brought
 * up. Rejects when they say no or the server does not answer.
 */
export function ensureWireguard(id: string): Promise<void> {
	const already = starting.get(id);
	if (already) return already;
	const job = (async () => {
		const running = await wireguardRunning().catch(() => [] as string[]);
		if (running.includes(id)) return;
		if (!loaded || !profiles.some((p) => p.id === id)) await refreshWireguard();
		const p = profiles.find((x) => x.id === id);
		if (!p) throw new Error(t('wireguard.missing_error'));
		const ok = await ask(t('wireguard.confirm_up', { name: p.name, endpoint: p.endpoint ?? '' }), { confirmLabel: t('wireguard.turn_on') });
		if (!ok) throw new Error(t('wireguard.declined', { name: p.name }));
		await wireguardUp(id);
		await refreshWireguard();
	})();
	starting.set(id, job);
	job.finally(() => starting.delete(id)).catch(() => {});
	return job;
}
