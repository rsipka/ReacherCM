/**
 * The pages of Settings and the headings they sit under.
 *
 * The rows themselves still live in the tab files (GeneralTab,
 * AppearanceTab, ...). A page shows one or more slices of them, so moving a
 * setting to another page is a change here and in the page's markup, never
 * a change to where the setting is stored.
 */

export type PageId =
	| 'general'
	| 'appearance'
	| 'shortcuts'
	| 'terminal'
	| 'sessions'
	| 'remote'
	| 'security'
	| 'pwmanagers'
	| 'sync'
	| 'backup'
	| 'ai'
	| 'devops'
	| 'sharing'
	| 'about';

export type GroupId = 'app' | 'connections' | 'data' | 'integrations';

export interface SettingsPage {
	id: PageId;
	/** i18n key of the page's name. */
	label: string;
	group: GroupId | null;
	/** An SVG path, drawn with a 1.8 stroke in a 24 box. */
	icon: string;
	/** Not shown on a phone or tablet. */
	desktopOnly?: boolean;
}

export const GROUPS: { id: GroupId; label: string }[] = [
	{ id: 'app', label: 'settings.group_app' },
	{ id: 'connections', label: 'settings.group_connections' },
	{ id: 'data', label: 'settings.group_data' },
	{ id: 'integrations', label: 'settings.group_integrations' }
];

const ICON = {
	gear: 'M12 15a3 3 0 100-6 3 3 0 000 6zM19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 01-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z',
	sun: 'M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z',
	keyboard: 'M3 6h18a1 1 0 011 1v10a1 1 0 01-1 1H3a1 1 0 01-1-1V7a1 1 0 011-1zM6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10',
	terminal: 'M4 5h16a1 1 0 011 1v12a1 1 0 01-1 1H4a1 1 0 01-1-1V6a1 1 0 011-1zM7 10l3 2-3 2M12 15h5',
	sessions: 'M4 6h16M4 12h16M4 18h10',
	monitor: 'M3 4h18v12H3zM8 20h8M12 16v4',
	shield: 'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z',
	key: 'M21 2l-2 2m-7.61 7.61a5.5 5.5 0 11-7.778 7.778 5.5 5.5 0 017.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4',
	archive: 'M3 4h18v4H3zM5 8v11a1 1 0 001 1h12a1 1 0 001-1V8M10 12h4',
	sync: 'M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15',
	bulb: 'M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z',
	server: 'M4 4h16v6H4zM4 14h16v6H4zM8 7h.01M8 17h.01',
	share: 'M18 8a3 3 0 100-6 3 3 0 000 6zM6 15a3 3 0 100-6 3 3 0 000 6zM18 22a3 3 0 100-6 3 3 0 000 6zM8.6 13.5l6.8 4M15.4 6.5l-6.8 4',
	info: 'M12 22a10 10 0 100-20 10 10 0 000 20zM12 16v-4M12 8h.01'
};

export const PAGES: SettingsPage[] = [
	{ id: 'general', label: 'settings.general', group: 'app', icon: ICON.gear },
	{ id: 'appearance', label: 'settings.appearance', group: 'app', icon: ICON.sun },
	{ id: 'shortcuts', label: 'shortcuts.page', group: 'app', icon: ICON.keyboard, desktopOnly: true },
	{ id: 'terminal', label: 'settings.page_terminal', group: 'connections', icon: ICON.terminal },
	{ id: 'sessions', label: 'settings.page_sessions', group: 'connections', icon: ICON.sessions },
	{ id: 'remote', label: 'settings.rdp_section', group: 'connections', icon: ICON.monitor },
	{ id: 'security', label: 'settings.security', group: 'data', icon: ICON.shield },
	{ id: 'pwmanagers', label: 'settings.password_managers', group: 'data', icon: ICON.key },
	{ id: 'sync', label: 'settings.page_sync', group: 'data', icon: ICON.sync },
	{ id: 'backup', label: 'settings.backup', group: 'data', icon: ICON.archive },
	{ id: 'ai', label: 'settings.page_ai_mcp', group: 'integrations', icon: ICON.bulb },
	{ id: 'devops', label: 'settings.page_devops_plugins', group: 'integrations', icon: ICON.server },
	{ id: 'sharing', label: 'settings.sharing', group: 'integrations', icon: ICON.share },
	{ id: 'about', label: 'settings.page_about', group: null, icon: ICON.info }
];

export function isPageId(value: string): value is PageId {
	return PAGES.some((p) => p.id === value);
}
