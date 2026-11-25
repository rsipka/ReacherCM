/**
 * A Proxmox console in a tab of its own: SPICE in the SPICE player, a
 * guest's display over VNC, or a text console (a container, a VM's serial
 * port, a node's shell) in a terminal. The tab names only the guest; the
 * backend opens the console through the Proxmox session's open connection
 * each time the tab connects, so a reconnect gets a fresh ticket.
 */
import { createTab } from '#lib/state/tabs.svelte.js';
import { setActivePage } from '#lib/state/navigation.svelte.js';
import { DEFAULT_TELNET_OPTIONS } from '#lib/ipc/telnet.js';
import type { ProxmoxConsole } from '#lib/ipc/proxmox.js';

export type ConsoleKind = 'spice' | 'vnc' | 'term';

/** Where the Proxmox session is, for the tab's title and address. */
export interface ConsoleHost {
	host: string;
	port: number;
}

/** A console's tab title: the guest's name and id, or the node's. */
export function consoleTitle(target: ProxmoxConsole, name: string | null | undefined): string {
	if (target.kind === 'node') return `${name || target.node} shell`;
	const base = name ? `${name} (${target.vmid})` : String(target.vmid);
	return target.serial ? `${base} ${target.serial}` : base;
}

export function openProxmoxConsole(kind: ConsoleKind, target: ProxmoxConsole, at: ConsoleHost, title: string): void {
	setActivePage('terminal');
	if (kind === 'spice') {
		const tab = createTab('spice', at.host, undefined, title, null);
		tab.spiceConnectParams = { id: tab.id, host: at.host, port: at.port, password: '', sessionId: target.sessionId, proxmox: target };
	} else if (kind === 'vnc') {
		const tab = createTab('vnc', at.host, undefined, title, null);
		tab.vncConnectParams = { id: tab.id, host: at.host, port: at.port, password: '', sessionId: target.sessionId, proxmox: target };
	} else {
		const tab = createTab('telnet', at.host, undefined, title, null);
		tab.telnetConnectParams = {
			host: at.host,
			port: at.port,
			options: { ...DEFAULT_TELNET_OPTIONS },
			sessionId: target.sessionId,
			proxmox: target
		};
	}
}
