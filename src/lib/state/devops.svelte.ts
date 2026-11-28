/**
 * The DevOps workspaces, and which of them are switched on.
 *
 * Everything that shows or reaches a DevOps tool reads this list: the tab bar,
 * the page that mounts it, and Settings → DevOps. Adding a tool is one entry
 * here, a variant in `src-tauri/src/devops.rs`, and a `require` at the top of
 * its commands.
 *
 * Databases, Containers and Kubernetes are built in and always on: they need
 * nothing installed and cost nothing until opened. Ansible and OpenTofu, which
 * run programs from the system, can be switched off; off hides them and makes
 * the backend refuse their commands. Nothing is deleted; projects and runs
 * are there again when it comes back.
 */

import { invoke } from '@tauri-apps/api/core';
import { getSettings, updateSetting } from '#lib/state/settings.svelte.js';
import { closeDevopsTabs, type Page } from '#lib/state/navigation.svelte.js';
import { isCommandRunning as isAnsibleRunning } from '#lib/state/ansible.svelte.js';
import { isCommandRunning as isTofuRunning } from '#lib/state/tofu.svelte.js';
import { isAnyBusy as isDbBusy } from '#lib/state/db.svelte.js';
import { isBusy as isContainersBusy } from '#lib/state/containers.svelte.js';
import { isBusy as isK8sBusy } from '#lib/state/k8s.svelte.js';
import { t } from '#lib/state/i18n.svelte.js';

export type DevopsToolId = 'ansible' | 'tofu' | 'databases' | 'containers' | 'kubernetes';

export interface DevopsTool {
	id: DevopsToolId;
	/** The page it opens. Shares the id, so the tab bar can use either. */
	page: Page & DevopsToolId;
	/** Binary name for `toolchain_check`; empty for a tool Reach carries itself. */
	binary: string;
	label: () => string;
	description: () => string;
	/** A run in progress. The switch waits for it rather than cutting it off. */
	isBusy: () => boolean;
	/** 24×24 stroked icon path, for its tabs and its rows in the Sessions list. */
	icon: string;
	/** Built in and always on: no switch in Settings → DevOps. */
	alwaysOn?: boolean;
}

/** The terminal's icon, beside the workspaces' (the command palette's page list). */
export const TERMINAL_ICON = 'M4 17l6-5-6-5M12 19h8';

export const DEVOPS_TOOLS: DevopsTool[] = [
	{
		id: 'ansible',
		page: 'ansible',
		binary: 'ansible',
		label: () => t('nav.ansible'),
		description: () => t('devops.ansible_desc'),
		isBusy: isAnsibleRunning,
		icon: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM8.5 16.5 12 7l3.5 9.5M9.6 13.5h4.8'
	},
	{
		id: 'tofu',
		page: 'tofu',
		binary: 'tofu',
		label: () => t('nav.tofu'),
		description: () => t('devops.tofu_desc'),
		isBusy: isTofuRunning,
		icon: 'M12 2.5l8.2 4.75v9.5L12 21.5l-8.2-4.75v-9.5z'
	},
	{
		id: 'databases',
		page: 'databases',
		alwaysOn: true,
		binary: '',
		label: () => t('nav.databases'),
		description: () => t('devops.databases_desc'),
		isBusy: isDbBusy,
		icon: 'M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3'
	},
	{
		id: 'containers',
		page: 'containers',
		alwaysOn: true,
		binary: '',
		label: () => t('nav.containers'),
		description: () => t('devops.containers_desc'),
		isBusy: isContainersBusy,
		icon: 'M3 7.5 12 3l9 4.5v9L12 21l-9-4.5zM3 7.5 12 12l9-4.5M12 12v9'
	},
	{
		id: 'kubernetes',
		page: 'kubernetes',
		alwaysOn: true,
		binary: '',
		label: () => t('nav.kubernetes'),
		description: () => t('devops.kubernetes_desc'),
		isBusy: isK8sBusy,
		icon: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7v3M12 14v3M7 12h3M14 12h3M8.5 8.5l2 2M13.5 13.5l2 2M15.5 8.5l-2 2M10.5 13.5l-2 2'
	}
];

export function isDevopsEnabled(id: DevopsToolId): boolean {
	return !!DEVOPS_TOOLS.find((d) => d.id === id)?.alwaysOn || getSettings().devopsTools.includes(id);
}

/** The tools Settings → DevOps has a switch for. */
export function getSwitchableDevopsTools(): DevopsTool[] {
	return DEVOPS_TOOLS.filter((tool) => !tool.alwaysOn);
}

export function getEnabledDevopsTools(): DevopsTool[] {
	return DEVOPS_TOOLS.filter((tool) => isDevopsEnabled(tool.id));
}

/**
 * Switch a tool on or off. Refuses to switch off mid-run and returns false,
 * though Settings disables the control first so this is only a backstop.
 */
export async function setDevopsEnabled(id: DevopsToolId, enabled: boolean): Promise<boolean> {
	const tool = DEVOPS_TOOLS.find((d) => d.id === id);
	if (!tool || tool.alwaysOn) return false;
	if (!enabled && tool.isBusy()) return false;

	const others = getSettings().devopsTools.filter((d) => d !== id);
	updateSetting('devopsTools', enabled ? [...others, id] : others);
	if (!enabled) closeDevopsTabs(tool.page);
	await pushToBackend(id, enabled);
	return true;
}

/** Tell the backend what is on. Call once after `loadSettings()`. */
export async function syncDevopsTools(): Promise<void> {
	await Promise.all(DEVOPS_TOOLS.map((tool) => pushToBackend(tool.id, isDevopsEnabled(tool.id))));
}

async function pushToBackend(tool: DevopsToolId, enabled: boolean): Promise<void> {
	try {
		await invoke('devops_set_enabled', { tool, enabled });
	} catch {
		// Backend not ready yet
	}
}
