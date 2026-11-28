<script lang="ts">
	import { tick } from 'svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import Modal from '#lib/components/shared/Modal.svelte';
	import GeneralTab from './GeneralTab.svelte';
	import AppearanceTab from './AppearanceTab.svelte';
	import SecurityTab from './SecurityTab.svelte';
	import SyncBackendSection from './SyncBackendSection.svelte';
	import SyncStatusSection from './SyncStatusSection.svelte';
	import DeviceLinkSection from './DeviceLinkSection.svelte';
	import BackupKeySection from './BackupKeySection.svelte';
	import AccountTab from './AccountTab.svelte';
	import BackupTab from './BackupTab.svelte';
	import AITab from './AITab.svelte';
	import McpTab from './McpTab.svelte';
	import DevopsTab from './DevopsTab.svelte';
	import PluginsTab from './PluginsTab.svelte';
	import SharingTab from './SharingTab.svelte';
	import PasswordManagersTab from './PasswordManagersTab.svelte';
	import PhoneKeysSetting from './PhoneKeysSetting.svelte';
	import SettingsCard from './SettingsCard.svelte';
	import Advanced from './Advanced.svelte';
	import KeyboardShortcutsTab from './KeyboardShortcutsTab.svelte';
	import NewIndicator from '#lib/components/shared/NewIndicator.svelte';
	import { settingsTabHasNew, markSettingsTabSeen, newSince } from '#lib/state/whats-new.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { isMobile } from '#lib/platform.js';
	import { GROUPS, PAGES, isPageId, type PageId, type SettingsPage } from './settings-pages.js';
	import { SETTINGS_INDEX, searchSettings, type IndexedRow } from './settings-index.js';
	import { syncedMarks } from './synced-marks.js';
	import { getSyncAccount, preferenceSync } from '#lib/state/preference-sync.svelte.js';
	import { PREFERENCE_CATALOG, PREFERENCE_KEYS, unavailablePreferenceKeys } from '#lib/sync/preference-catalog.js';

	interface Props {
		open: boolean;
		onclose: () => void;
		/** A page id or a row's label key to show on opening (openSettings). */
		target?: string;
	}

	let { open, onclose, target }: Props = $props();

	/** A phone, upright or on its side: one screen at a time, iOS style. */
	const phoneLayout = new MediaQuery('(max-width: 640px), (max-height: 480px)');
	let phone = $derived(phoneLayout.current);
	const mobile = isMobile();

	let pageId = $state<PageId>('general');
	/** On a phone, the menu shows until a page is picked. */
	let showingMenu = $state(true);
	let query = $state('');
	let searchEl = $state<HTMLInputElement | null>(null);
	let contentEl = $state<HTMLElement | null>(null);

	let page = $derived(PAGES.find((p) => p.id === pageId) ?? PAGES[0]);
	let results = $derived(searchSettings(query, t, (id) => pageName(id), mobile));
	let searching = $derived(query.trim().length > 0);

	// Rows whose value syncs through the Reacher CM account right now: signed
	// in, preference sync on, the row not kept local on this device, and (once
	// known) supported by the server.
	const unavailableOnDevice = unavailablePreferenceKeys(mobile);
	let synced = $derived({
		labels: new Set(getSyncAccount() && preferenceSync.enabled
			? PREFERENCE_KEYS.filter((key) => !preferenceSync.skipped.includes(key) && !unavailableOnDevice.includes(key)
				&& (preferenceSync.supported === null || preferenceSync.supported.includes(key)))
				.map((key) => t(PREFERENCE_CATALOG[key].label).trim())
			: []),
		tip: t('sync_account.synced_mark'),
	});

	function pageName(id: PageId): string {
		return t(PAGES.find((p) => p.id === id)?.label ?? id);
	}

	function pagesIn(group: string | null): SettingsPage[] {
		return PAGES.filter((p) => p.group === group && !(mobile && p.desktopOnly));
	}

	// Opening: a target goes straight to its page (and row); otherwise a
	// desktop puts the cursor in search, so Ctrl+, then typing finds a
	// setting, and a phone starts at the menu.
	$effect(() => {
		if (!open) return;
		const wanted = target;
		query = '';
		if (wanted) {
			void goTo(wanted);
		} else if (phone) {
			showingMenu = true;
		} else {
			void tick().then(() => searchEl?.focus());
		}
	});

	// Opening a page is what clears its badge.
	$effect(() => {
		if (open && !(phone && showingMenu)) markSettingsTabSeen(pageId);
	});

	function showPage(id: PageId): void {
		pageId = id;
		showingMenu = false;
		query = '';
		contentEl?.scrollTo({ top: 0 });
	}

	/** Open a page, or the page of a row and point at the row. */
	async function goTo(where: string): Promise<void> {
		if (isPageId(where)) {
			showPage(where);
			return;
		}
		const row = SETTINGS_INDEX.find((r) => r.label === where);
		if (!row) return;
		showPage(row.page);
		await tick();
		highlight(row);
	}

	/** Find a row by the text of its label, open its Advanced section if it
	 *  is shut, bring it into view and flash it. */
	async function highlight(row: IndexedRow): Promise<void> {
		const text = t(row.label).trim();
		let el: HTMLElement | null = null;
		for (let tries = 0; tries < 10 && !el; tries++) {
			const labels = contentEl?.querySelectorAll<HTMLElement>('.setting-label, .section-title, .section-label, .card-title') ?? [];
			el = Array.from(labels).find((l) => l.textContent?.trim() === text) ?? null;
			if (!el) await new Promise((r) => setTimeout(r, 50));
		}
		if (!el) return;
		const details = el.closest('details');
		if (details && !details.open) {
			details.open = true;
			await tick();
		}
		const target = el.closest<HTMLElement>('.setting-row, .setting-section, .settings-card') ?? el;
		target.scrollIntoView({ block: 'center', behavior: 'smooth' });
		target.classList.remove('settings-flash');
		void target.offsetWidth;
		target.classList.add('settings-flash');
		setTimeout(() => target.classList.remove('settings-flash'), 1800);
	}

	function onSearchKeydown(e: KeyboardEvent): void {
		if (e.key === 'Enter' && results.length > 0) {
			e.preventDefault();
			void goTo(results[0].label);
		} else if (e.key === 'Escape' && query) {
			// The first Escape clears the search; the next closes Settings.
			e.preventDefault();
			e.stopPropagation();
			query = '';
		}
	}
</script>

{#snippet icon(path: string)}
	<svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
		<path d={path} stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
	</svg>
{/snippet}

{#snippet searchBox()}
	<div class="search">
		<svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
			<path d="M11 19a8 8 0 100-16 8 8 0 000 16zM21 21l-4.35-4.35" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
		</svg>
		<input
			bind:this={searchEl}
			bind:value={query}
			type="search"
			placeholder={t('settings.search_placeholder')}
			aria-label={t('settings.search_placeholder')}
			spellcheck="false"
			autocomplete="off"
			onkeydown={onSearchKeydown}
		/>
	</div>
{/snippet}

{#snippet resultList()}
	<div class="results">
		{#if results.length === 0}
			<p class="no-results">{t('settings.search_none', { query })}</p>
		{:else}
			{#each results as row (row.page + row.label)}
				<button class="result" onclick={() => goTo(row.label)}>
					<span class="result-label">{t(row.label)}</span>
					<span class="result-path">
						{pageName(row.page)}{#if row.advanced}{' › '}{t('settings.advanced')}{/if}
					</span>
				</button>
			{/each}
		{/if}
	</div>
{/snippet}

{#snippet menuItem(p: SettingsPage)}
	<button
		class="menu-item"
		class:active={!phone && pageId === p.id}
		class:has-new={settingsTabHasNew(p.id)}
		aria-current={!phone && pageId === p.id ? 'page' : undefined}
		onclick={() => showPage(p.id)}
	>
		{@render icon(p.icon)}
		<span class="menu-label">{t(p.label)}</span>
		{#if settingsTabHasNew(p.id)}
			<NewIndicator variant="badge" since={newSince({ kind: 'settings', tab: p.id })} />
		{/if}
		{#if phone}
			<svg class="chevron" width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
				<path d="M9 6l6 6-6 6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
			</svg>
		{/if}
	</button>
{/snippet}

{#snippet menu()}
	<nav class="settings-menu" aria-label={t('settings.title')}>
		{@render searchBox()}
		{#if !(phone && searching)}
			{#each GROUPS as group (group.id)}
				<div class="menu-group">
					<span class="group-label">{t(group.label)}</span>
					<div class="group-items">
						{#each pagesIn(group.id) as p (p.id)}
							{@render menuItem(p)}
						{/each}
					</div>
				</div>
			{/each}
			<div class="menu-group">
				<div class="group-items">
					{#each pagesIn(null) as p (p.id)}
						{@render menuItem(p)}
					{/each}
				</div>
			</div>
		{:else}
			{@render resultList()}
		{/if}
	</nav>
{/snippet}

{#snippet pageBody(id: PageId)}
	{#if id === 'general'}
		<SettingsCard><GeneralTab part="general" /></SettingsCard>
	{:else if id === 'appearance'}
		<SettingsCard><AppearanceTab part="appearance" /></SettingsCard>
	{:else if id === 'shortcuts'}
		<KeyboardShortcutsTab />
	{:else if id === 'terminal'}
		<SettingsCard title={t('settings.section_text')}><AppearanceTab part="text" /></SettingsCard>
		<SettingsCard title={t('settings.section_scrollback')}><AppearanceTab part="scrollback" /></SettingsCard>
		<SettingsCard title={t('settings.section_paste')}><GeneralTab part="paste" /></SettingsCard>
		<SettingsCard title={t('settings.section_shell')}><GeneralTab part="shell" /></SettingsCard>
		<SettingsCard title={t('settings.section_bell')}><GeneralTab part="bell" /></SettingsCard>
		{#if mobile}
			<SettingsCard title={t('settings.section_phone')}><PhoneKeysSetting /></SettingsCard>
		{/if}
		<Advanced>
			<GeneralTab part="paste-adv" />
			<AppearanceTab part="term-adv" />
		</Advanced>
	{:else if id === 'sessions'}
		<SettingsCard><GeneralTab part="sessions" /></SettingsCard>
		<SettingsCard title={t('settings.log_section')}><AppearanceTab part="logging" /></SettingsCard>
		<SettingsCard title={t('history.settings_section')}><AppearanceTab part="history" /></SettingsCard>
		<SettingsCard title={t('settings.tmux_section')}><GeneralTab part="tmux" /></SettingsCard>
	{:else if id === 'remote'}
		<SettingsCard><AppearanceTab part="rdp" /></SettingsCard>
		<Advanced><AppearanceTab part="rdp-adv" /></Advanced>
	{:else if id === 'security'}
		<SettingsCard><SecurityTab /></SettingsCard>
	{:else if id === 'pwmanagers'}
		<PasswordManagersTab />
	{:else if id === 'sync'}
		<AccountTab />
		<SettingsCard title={t('sync.backend_title')}>
			<SyncBackendSection onOpenVaultLinks={() => void goTo('sync.join_device')} />
		</SettingsCard>
		<SettingsCard title={t('sync.this_device')}><DeviceLinkSection /></SettingsCard>
		<SettingsCard title={t('sync.status_title')}><SyncStatusSection /></SettingsCard>
	{:else if id === 'backup'}
		<SettingsCard title={t('settings.backup')}><BackupTab /></SettingsCard>
		<SettingsCard title={t('sync.backup_key_title')}><BackupKeySection /></SettingsCard>
	{:else if id === 'ai'}
		<SettingsCard title={t('settings.ai')}><AITab /></SettingsCard>
		<SettingsCard title={t('settings.mcp')}><McpTab /></SettingsCard>
	{:else if id === 'devops'}
		<SettingsCard title={t('settings.devops')}><DevopsTab /></SettingsCard>
		<SettingsCard title={t('settings.plugins')}><PluginsTab /></SettingsCard>
	{:else if id === 'sharing'}
		<SettingsCard><SharingTab /></SettingsCard>
	{:else if id === 'about'}
		<SettingsCard><GeneralTab part="updates" /></SettingsCard>
		<SettingsCard><GeneralTab part="about" /></SettingsCard>
	{/if}
{/snippet}

<Modal
	{open}
	{onclose}
	title={phone ? '' : t('settings.title')}
	maxWidth="960px"
	height="min(680px, 90vh)"
	padded={false}
	fullOnPhone
>
	{#snippet children()}
		{#if phone}
			<!-- One screen at a time: the menu, or a page with a way back. -->
			<header class="phone-head">
				{#if !showingMenu}
					<button class="back" onclick={() => (showingMenu = true)}>
						<svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
							<path d="M15 6l-6 6 6 6" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
						</svg>
						{t('settings.title')}
					</button>
				{/if}
				<h2 class="phone-title">{showingMenu ? t('settings.title') : t(page.label)}</h2>
				<button class="close" onclick={onclose} aria-label={t('common.close_dialog')}>
					<svg width="14" height="14" viewBox="0 0 14 14" fill="none">
						<path d="M1 1L13 13M13 1L1 13" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
					</svg>
				</button>
			</header>
			{#if showingMenu}
				<div class="phone-scroll">{@render menu()}</div>
			{:else}
				<div class="settings-content phone-scroll" bind:this={contentEl} use:syncedMarks={synced}>
					{@render pageBody(pageId)}
				</div>
			{/if}
		{:else}
			<div class="settings-layout">
				{@render menu()}
				<div class="settings-content" bind:this={contentEl} use:syncedMarks={synced}>
					{#if searching}
						<h2 class="page-title">{t('settings.search_results')}</h2>
						{@render resultList()}
					{:else}
						<h2 class="page-title">{t(page.label)}</h2>
						{@render pageBody(pageId)}
					{/if}
				</div>
			</div>
		{/if}
	{/snippet}
</Modal>

<style>
	.settings-content :global(.synced-mark) {
		display: inline-flex;
		vertical-align: -1px;
		margin-left: 6px;
		color: var(--color-text-secondary);
		opacity: 0.8;
	}

	/* The frame is fixed by the Modal; the two columns fill it and the content
	   column scrolls on its own. Switching pages never moves an edge. */
	.settings-layout {
		display: flex;
		flex: 1;
		min-height: 0;
	}

	.settings-menu {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 10px 8px 12px;
		width: 212px;
		flex-shrink: 0;
		overflow-y: auto;
		border-right: 1px solid var(--color-border);
		background-color: var(--color-bg-secondary);
		border-radius: 0 0 0 var(--radius-modal, 12px);
	}

	.search {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 0 8px;
		height: 30px;
		color: var(--color-text-tertiary);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong, var(--color-border));
		border-radius: 7px;
	}

	.search:focus-within {
		border-color: var(--color-focus, var(--color-accent));
	}

	.search input {
		flex: 1;
		min-width: 0;
		height: 100%;
		padding: 0;
		font-family: inherit;
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		background: transparent;
		border: none;
		outline: none;
	}

	.search input::-webkit-search-cancel-button {
		cursor: pointer;
	}

	.menu-group {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.group-label {
		padding: 0 10px 2px;
		font-size: 0.6875rem;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--color-text-tertiary);
	}

	.group-items {
		display: flex;
		flex-direction: column;
		gap: 1px;
	}

	.menu-item {
		display: flex;
		align-items: center;
		gap: 9px;
		width: 100%;
		padding: 6px 10px;
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		font-weight: 500;
		text-align: left;
		color: var(--color-text-secondary);
		background: transparent;
		border: none;
		border-radius: 6px;
		cursor: pointer;
		white-space: nowrap;
		transition:
			color var(--duration-default) var(--ease-default),
			background-color var(--duration-default) var(--ease-default);
	}

	.menu-item:hover {
		color: var(--color-text-primary);
		background-color: var(--color-surface-hover);
	}

	.menu-item.active {
		color: var(--color-text-primary);
		background-color: var(--color-accent-subtle, var(--color-surface-active));
		box-shadow: inset 2px 0 0 var(--color-accent);
	}

	/* A page with something new in it takes the accent colour, so the row
	   reads as new before you get to the badge on the end. */
	.menu-item.has-new:not(.active) {
		color: var(--color-accent);
	}

	.menu-label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.menu-item svg {
		flex-shrink: 0;
		opacity: 0.75;
	}

	.menu-item.active svg {
		opacity: 1;
	}

	.settings-content {
		flex: 1;
		padding: 18px 24px 24px;
		overflow-y: auto;
		min-width: 0;
		/* A page that scrolls must not sit 10px left of one that does not. */
		scrollbar-gutter: stable;
	}

	.page-title {
		margin: 0 0 14px;
		font-size: 1.125rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.results {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.result {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 2px;
		padding: 8px 12px;
		text-align: left;
		font-family: inherit;
		background: transparent;
		border: none;
		border-radius: 8px;
		cursor: pointer;
	}

	.result:hover,
	.result:focus-visible {
		background: var(--color-surface-hover);
	}

	.result-label {
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--color-text-primary);
	}

	.result-path {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.no-results {
		margin: 8px 12px;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	/* ─── Phone ─── */

	.phone-head {
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		align-items: center;
		height: 48px;
		padding: 0 8px;
		flex-shrink: 0;
		border-bottom: 1px solid var(--color-border);
		background: var(--color-bg-secondary);
	}

	.phone-title {
		grid-column: 2;
		margin: 0;
		font-size: 1rem;
		font-weight: 600;
		color: var(--color-text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.back {
		grid-column: 1;
		justify-self: start;
		display: inline-flex;
		align-items: center;
		gap: 2px;
		padding: 8px 6px;
		font-family: inherit;
		font-size: 0.9375rem;
		color: var(--color-accent);
		background: none;
		border: none;
		cursor: pointer;
	}

	.close {
		grid-column: 3;
		justify-self: end;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 40px;
		height: 40px;
		color: var(--color-text-secondary);
		background: none;
		border: none;
		cursor: pointer;
	}

	.phone-scroll {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}

	.phone-scroll .settings-menu {
		width: auto;
		gap: 18px;
		padding: 12px 16px 24px;
		overflow: visible;
		border: none;
		border-radius: 0;
		background: var(--color-bg-primary);
	}

	.phone-scroll .search {
		height: 38px;
		border-radius: 10px;
	}

	.phone-scroll .search input {
		font-size: 1rem;
	}

	/* iOS-style grouped list: one rounded box per heading, rows divided by
	   hairlines, a chevron on the end of each. */
	.phone-scroll .group-items {
		gap: 0;
		overflow: hidden;
		background: var(--color-bg-secondary);
		border: 1px solid var(--color-border);
		border-radius: 12px;
	}

	.phone-scroll .group-label {
		padding: 0 14px 6px;
	}

	.phone-scroll .menu-item {
		padding: 12px 14px;
		gap: 12px;
		font-size: 0.9375rem;
		color: var(--color-text-primary);
		border-radius: 0;
	}

	.phone-scroll .menu-item + .menu-item {
		border-top: 1px solid var(--color-border);
	}

	.phone-scroll .menu-item svg {
		opacity: 0.85;
		color: var(--color-accent);
	}

	.phone-scroll .menu-item .chevron {
		color: var(--color-text-tertiary);
	}

	.phone-scroll.settings-content {
		padding: 14px 14px 28px;
		background: var(--color-bg-primary);
	}

	.phone-scroll .results {
		background: var(--color-bg-secondary);
		border: 1px solid var(--color-border);
		border-radius: 12px;
		overflow: hidden;
		gap: 0;
	}

	.phone-scroll .result {
		padding: 11px 14px;
		border-radius: 0;
	}

	.phone-scroll .result + .result {
		border-top: 1px solid var(--color-border);
	}
</style>
