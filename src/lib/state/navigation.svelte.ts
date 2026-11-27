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

/**
 * Open a workspace in a tab, or bring its open tab forward. Kubernetes has a
 * tab per cluster: with `clusterId`, that cluster's tab (made if none is
 * open); without, any Kubernetes tab, the one in front of the others first.
 * The other workspaces have one tab each, as their state is the app's.
 */
export function openDevopsTab(tool: DevopsTabTool, clusterId?: string, clusterName?: string): Tab {
	const open = devopsTabs(tool);
	const existing = tool === 'kubernetes' && clusterId ? (open.find((x) => x.k8sClusterId === clusterId) ?? open.find((x) => !x.k8sClusterId)) : open[0];
	if (existing) {
		if (clusterId && !existing.k8sClusterId) {
			existing.k8sClusterId = clusterId;
			if (clusterName) existing.title = clusterName;
		}
		activateTab(existing.id);
		return existing;
	}
	const made = createTab('devops', clusterName ?? t(`nav.${tool}`));
	// The list holds its own (reactive) copy: set the fields on that one.
	const tab = getTabs().find((x) => x.id === made.id) ?? made;
	tab.devopsTool = tool;
	if (clusterId) tab.k8sClusterId = clusterId;
	return tab;
}

/** A workspace switched off: its tabs go. */
export function closeDevopsTabs(tool: DevopsTabTool): void {
	for (const tab of devopsTabs(tool)) closeTab(tab.id);
}
