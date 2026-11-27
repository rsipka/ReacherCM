/**
 * What fills the window: the terminal area, or a DevOps workspace.
 *
 * The workspaces used to be pages of their own, picked from a menu at the
 * start of the tab bar. They are tabs now, opened from the Sessions list, the
 * command palette or the tray, so a Kubernetes cluster sits beside the SSH
 * session it runs on, splits with it and is closed the same way. The page is
 * read from the tab in front: a `devops` tab is its workspace's page, any
 * other tab (or none) the terminal.
 */

import { activateTab, closeTab, createTab, getActiveTab, getTabs, lastUsedTab, type DevopsTabTool, type Tab } from '#lib/state/tabs.svelte.js';
import { t } from '#lib/state/i18n.svelte.js';

export type Page = 'terminal' | DevopsTabTool;

export function getActivePage(): Page {
	const tab = getActiveTab();
	return tab?.type === 'devops' && tab.devopsTool ? tab.devopsTool : 'terminal';
}

/**
 * Bring a page to the front. A workspace opens its tab (or brings the open
 * one forward); 'terminal' leaves a workspace tab for the session tab used
 * before it, and does nothing when a session tab is already in front.
 */
export function setActivePage(page: Page): void {
	if (page !== 'terminal') {
		openDevopsTab(page);
		return;
	}
	if (getActiveTab()?.type !== 'devops') return;
	const back = lastSessionTab();
	if (back) activateTab(back.id);
}

/** The session tab most recently in front, for going back from a workspace. */
function lastSessionTab(): Tab | undefined {
	const last = lastUsedTab();
	if (last && last.type !== 'devops') return last;
	return getTabs().find((x) => x.type !== 'devops');
}

/** The open tabs of one workspace. */
export function devopsTabs(tool: DevopsTabTool): Tab[] {
	return getTabs().filter((x) => x.type === 'devops' && x.devopsTool === tool);
}

/** The workspaces with a tab per thing they show (a cluster, a host). */
export type TargetTool = 'kubernetes' | 'containers';

/** What a tab of such a workspace shows, if it has picked one. */
export function tabTarget(tab: Tab): string | undefined {
	return tab.devopsTool === 'kubernetes' ? tab.k8sClusterId : tab.devopsTool === 'containers' ? tab.ctrHostId : undefined;
}

/** Give a tab its cluster or host (unset: back to the picker). */
export function setTabTarget(tab: Tab, id: string | undefined): void {
	if (tab.devopsTool === 'kubernetes') tab.k8sClusterId = id;
	else if (tab.devopsTool === 'containers') tab.ctrHostId = id;
}

/** Another tab of the same workspace already showing `id`. */
export function otherTabShowing(tool: TargetTool, id: string, self?: string): Tab | undefined {
	return devopsTabs(tool).find((x) => x.id !== self && tabTarget(x) === id);
}

/**
 * Open a workspace in a tab, or bring its open tab forward. Kubernetes and
 * Containers have a tab per cluster or host: with `targetId`, that one's tab
 * (made if none is open, or a tab still at its picker takes it); without, any
 * of their tabs. The other workspaces have one tab each, as their state is
 * the app's.
 */
export function openDevopsTab(tool: DevopsTabTool, targetId?: string, targetName?: string): Tab {
	const open = devopsTabs(tool);
	const perTarget = tool === 'kubernetes' || tool === 'containers';
	const existing = perTarget && targetId ? (open.find((x) => tabTarget(x) === targetId) ?? open.find((x) => !tabTarget(x))) : open[0];
	if (existing) {
		if (targetId && perTarget && !tabTarget(existing)) {
			setTabTarget(existing, targetId);
			if (targetName) existing.title = targetName;
		}
		activateTab(existing.id);
		return existing;
	}
	const made = createTab('devops', (perTarget && targetName) || t(`nav.${tool}`));
	// The list holds its own (reactive) copy: set the fields on that one.
	const tab = getTabs().find((x) => x.id === made.id) ?? made;
	tab.devopsTool = tool;
	if (perTarget && targetId) setTabTarget(tab, targetId);
	return tab;
}

/** A workspace switched off: its tabs go. */
export function closeDevopsTabs(tool: DevopsTabTool): void {
	for (const tab of devopsTabs(tool)) closeTab(tab.id);
}

// ---- A new cluster, host, connection or project, asked for from elsewhere --

let pendingNew = $state<DevopsTabTool | null>(null);

/**
 * Open a workspace with its "add" editor showing: New session's DevOps tab
 * uses it. Kubernetes and Containers get a tab at their picker (an open one,
 * or a new one), so the new cluster or host does not replace what a tab
 * already shows.
 */
export function requestNewDevopsItem(tool: DevopsTabTool): void {
	pendingNew = tool;
	if (tool === 'kubernetes' || tool === 'containers') {
		const picker = devopsTabs(tool).find((x) => !tabTarget(x));
		if (picker) {
			activateTab(picker.id);
			return;
		}
		const made = createTab('devops', t(`nav.${tool}`));
		const tab = getTabs().find((x) => x.id === made.id) ?? made;
		tab.devopsTool = tool;
		return;
	}
	openDevopsTab(tool);
}

/** Is an "add" editor asked for in this workspace? */
export function wantsNewDevopsItem(tool: DevopsTabTool): boolean {
	return pendingNew === tool;
}

/** The workspace showing it takes the request, once. */
export function takeNewDevopsItem(tool: DevopsTabTool): boolean {
	if (pendingNew !== tool) return false;
	pendingNew = null;
	return true;
}
