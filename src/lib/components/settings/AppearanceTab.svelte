<script lang="ts">
	import Toggle from '#lib/components/shared/Toggle.svelte';
	import Dropdown from '#lib/components/shared/Dropdown.svelte';
	import { CURSOR_STYLES, CURSOR_WIDTHS, INACTIVE_CURSOR_STYLES, cursorStyle, cursorWidth, inactiveCursorStyle } from '#lib/terminal/cursor.js';
	import { SCROLLBACK_CHOICES, UNLIMITED_SCROLLBACK, scrollbackSetting } from '#lib/terminal/scrollback.js';
	import TerminalThemeSettings from './TerminalThemeSettings.svelte';
	import { HIGHLIGHT_KINDS, highlightKinds, parseCustomRules, type HighlightKind } from '#lib/terminal/highlight.js';
	import { themeState, applyTheme, themeFor, loadInstalledThemes, DARK, LIGHT, BUILT_IN_IDS, BUNDLED_IDS, systemAppearance } from '#lib/state/theme.svelte.js';
	import { pairTheme } from '#lib/themes/resolve.js';
	import { onMount } from 'svelte';
	import { themeFetchRegistry, themeInstall, themeUninstall, type ThemeEntry } from '#lib/ipc/theme.js';
	import { validateTheme } from '#lib/state/theme.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { getSettings, updateSetting, DEFAULT_LOG_NAME } from '#lib/state/settings.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { matchesQuery } from '#lib/utils/search.js';
	import { isMobile } from '#lib/platform.js';
	import { stepUiZoom, UI_ZOOM_STEPS } from '#lib/state/ui-zoom.js';
	import { actionLabel } from '#lib/state/shortcuts.svelte.js';
	import { sessionLogDefaultDir } from '#lib/ipc/ssh.js';
	import { historyForget, historyStats, type HistoryStats } from '#lib/ipc/history.js';
	import { open as pickFolder } from '@tauri-apps/plugin-dialog';
	import { fontsListInstalled, type InstalledFont } from '#lib/ipc/fonts.js';
	import { freerdpFind } from '#lib/ipc/freerdp.js';
	import { cssFontFamily } from '#lib/utils/font.js';

	/** Which slice of these rows to show: Settings arranges them by topic,
	 *  so one tab file feeds several pages (see settings-pages.ts). */
	type Part = 'appearance' | 'text' | 'scrollback' | 'term-adv' | 'logging' | 'history' | 'rdp' | 'rdp-adv';
	let { part }: { part: Part } = $props();

	const settings = getSettings();
	let freerdpFound = $state<string | null>(null);
	let detectingFreerdp = $state(false);

	$effect(() => {
		if (part !== 'rdp' || isMobile()) return;
		const path = settings.freerdpPath;
		let current = true;
		detectingFreerdp = true;
		void freerdpFind(path)
			.catch(() => null)
			.then((found) => {
				if (!current) return;
				freerdpFound = found;
				detectingFreerdp = false;
			});
		return () => { current = false; };
	});

	async function browseFreerdp(): Promise<void> {
		try {
			const picked = await pickFolder({ multiple: false, directory: false, title: t('settings.freerdp_path') });
			if (typeof picked === 'string') updateSetting('freerdpPath', picked);
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	let cursorStyleOptions = $derived(CURSOR_STYLES.map((s) => ({ label: t(`cursor.style_${s}`), value: s })));
	let cursorInactiveOptions = $derived(
		INACTIVE_CURSOR_STYLES.map((s) => ({
			label: s === 'outline' || s === 'none' ? t(`cursor.inactive_${s}`) : t(`cursor.style_${s}`),
			value: s
		}))
	);
	// A number set some other way (a synced vault) is offered too.
	let scrollbackOptions = $derived(
		[...new Set<number>([...SCROLLBACK_CHOICES.filter((n) => n !== UNLIMITED_SCROLLBACK), settings.terminalScrollback])]
			.filter((n) => n !== UNLIMITED_SCROLLBACK)
			.sort((a, b) => a - b)
			.map((n) => ({ label: t('settings.scrollback_lines', { count: n.toLocaleString() }), value: String(n) }))
			.concat([{ label: t('settings.scrollback_unlimited'), value: String(UNLIMITED_SCROLLBACK) }])
	);
	let cursorWidthOptions = $derived(CURSOR_WIDTHS.map((w) => ({ label: t('cursor.width_px', { count: w }), value: String(w) })));
	let shownKinds = $derived(highlightKinds(settings.terminalHighlightKinds));
	/** Lines of the custom patterns field that are not valid, shown under it;
	 *  the valid ones work meanwhile. */
	let customErrors = $derived(parseCustomRules(settings.terminalHighlightCustom).errors);

	function toggleKind(kind: HighlightKind, on: boolean): void {
		const next = new Set(shownKinds);
		if (on) next.add(kind);
		else next.delete(kind);
		updateSetting('terminalHighlightKinds', HIGHLIGHT_KINDS.filter((k) => next.has(k)));
	}
	let currentFont = $derived(settings.fontFamily || 'monospace');
	let currentSize = $derived(settings.fontSize || 14);
	let uiZoom = $derived(settings.uiZoom || 1);

	/** The terminal's text size, here too, as a phone has no Ctrl+wheel; open
	 *  terminals follow the setting. Same 8 to 32 px as the terminal's zoom. */
	function stepFontSize(step: 1 | -1): void {
		updateSetting('fontSize', Math.min(Math.max(currentSize + step, 8), 32));
	}

	let registry = $state<ThemeEntry[]>([]);
	let registryError = $state<string | null>(null);
	let loadingRegistry = $state(false);
	let installingId = $state<string | null>(null);

	let installedIds = $derived(new Set(themeState.installed.map((t) => t.id)));

	/**
	 * The registry is a public, PR-gated index with no size limit, so this list
	 * has to stay bounded: rendering every entry builds one DOM row per theme
	 * and would stall the settings panel long before the registry got
	 * interesting. Search narrows, and the list grows a page at a time on
	 * request — cheaper and far less fragile than virtualising rows whose
	 * height varies with whether a theme carries a description.
	 */
	const PAGE = 40;

	let themeQuery = $state('');
	let shown = $state(PAGE);

	let matching = $derived(
		registry.filter((e) => matchesQuery(themeQuery, [e.name, e.description, e.author, e.appearance]))
	);
	let visible = $derived(matching.slice(0, shown));

	// A new search starts from the top of its own results.
	$effect(() => {
		themeQuery;
		shown = PAGE;
	});

	async function loadRegistry(): Promise<void> {
		loadingRegistry = true;
		registryError = null;
		try {
			// A theme the app already ships is not offered again.
			registry = (await themeFetchRegistry()).filter((e) => !BUILT_IN_IDS.has(e.id));
		} catch (err) {
			registryError = String(err);
		} finally {
			loadingRegistry = false;
		}
	}

	async function install(entry: ThemeEntry): Promise<void> {
		installingId = entry.id;
		try {
			const doc = await themeInstall(entry);
			const result = validateTheme(doc);
			if (!result.ok) {
				addToast(result.error, 'error');
				return;
			}
			themeState.installed = [
				...themeState.installed.filter((t) => t.id !== result.theme.id),
				result.theme
			];
			addToast(t('themes.installed'), 'info');
		} catch (err) {
			addToast(String(err), 'error');
		} finally {
			installingId = null;
		}
	}

	async function remove(id: string): Promise<void> {
		try {
			await themeUninstall(id);
			themeState.installed = themeState.installed.filter((th) => th.id !== id);
			// Fall back if the theme in use was the one removed.
			if (settings.themeId === id) selectTheme(SYSTEM);
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	// Shown as the folder field's placeholder, so an empty field says where
	// logs actually go.
	let logDefaultDir = $state('');

	/** What the session history holds; null while the vault is locked. */
	let historyInfo = $state<HistoryStats | null>(null);
	/** Forgetting everything takes a second click. */
	let forgetArmed = $state(false);

	async function forgetAllHistory(): Promise<void> {
		if (!forgetArmed) {
			forgetArmed = true;
			setTimeout(() => (forgetArmed = false), 4000);
			return;
		}
		forgetArmed = false;
		try {
			await historyForget({ what: 'all' });
			historyInfo = await historyStats();
			addToast(t('history.forgot_all'), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	async function browseLogFolder(): Promise<void> {
		const picked = await pickFolder({ directory: true });
		if (typeof picked === 'string') updateSetting('sessionLogFolder', picked);
	}

	onMount(() => {
		// Each page loads only what its rows show.
		if (part === 'logging') sessionLogDefaultDir().then((d) => (logDefaultDir = d)).catch(() => {});
		if (part === 'history') historyStats().then((s) => (historyInfo = s)).catch(() => {});
		if (part === 'appearance') {
			// Re-read from disk in case a theme was installed or edited since startup.
			loadInstalledThemes();
			loadRegistry();
		}
	});

	/** "System" follows the OS; every other entry is a real theme. */
	const SYSTEM = '__system__';

	let choices = $derived([
		{ id: SYSTEM, name: t('settings.theme_system'), swatch: null },
		...themeState.all.map((th) => ({ id: th.id, name: th.name, swatch: th }))
	]);

	/** The bundled themes stay folded until asked for; the one in use always shows. */
	let showAllThemes = $state(false);
	let shownChoices = $derived(
		showAllThemes ? choices : choices.filter((c) => !BUNDLED_IDS.has(c.id) || c.id === selectedId)
	);
	let foldedCount = $derived(
		choices.filter((c) => BUNDLED_IDS.has(c.id) && c.id !== selectedId).length
	);

	// Settings from before themes had ids say only dark or light: that is
	// Reach's own, and its card is the one marked.
	let selectedId = $derived(
		settings.themeId ?? (settings.theme === 'system' ? SYSTEM : settings.theme === 'light' ? LIGHT.id : DARK.id)
	);

	/** The two halves of System, as they would resolve now. */
	let pairLight = $derived(pairTheme(settings, false, themeState.all, { light: LIGHT, dark: DARK }));
	let pairDark = $derived(pairTheme(settings, true, themeState.all, { light: LIGHT, dark: DARK }));

	/** Each half offers the themes of its own appearance. */
	const pairOptions = (appearance: 'light' | 'dark') =>
		themeState.all
			.filter((th) => th.appearance === appearance)
			.map((th) => ({ label: th.name, value: th.id }));
	let lightOptions = $derived(pairOptions('light'));
	let darkOptions = $derived(pairOptions('dark'));

	function selectTheme(id: string) {
		if (id === SYSTEM) {
			// Coming from a theme picked by hand: it becomes that half of the
			// pair, unless the pair already says otherwise. Picking Dracula and
			// then System keeps Dracula for the night.
			const previous = settings.themeId ? themeState.all.find((th) => th.id === settings.themeId) : undefined;
			if (previous && previous.id !== DARK.id && previous.id !== LIGHT.id) {
				const key = previous.appearance === 'dark' ? 'themeDark' : 'themeLight';
				if (!settings[key]) updateSetting(key, previous.id);
			}
			updateSetting('themeId', undefined);
			updateSetting('theme', 'system');
			applyTheme(themeFor(settings));
			return;
		}
		const theme = themeState.all.find((t2) => t2.id === id);
		if (!theme) return;
		updateSetting('themeId', theme.id === DARK.id || theme.id === LIGHT.id ? undefined : theme.id);
		// Keep `theme` consistent so anything still reading it, and the fallback
		// path when a themeId no longer resolves, stay sensible.
		updateSetting('theme', theme.appearance);
		applyTheme(theme);
	}

	function selectPair(half: 'themeLight' | 'themeDark', id: string) {
		updateSetting(half, id);
		applyTheme(themeFor(settings));
	}


	// Local fonts only — NO network (Google Fonts) fetches. `JetBrains Mono`
	// is bundled (app.css @font-face) and `monospace` is the OS's own choice;
	// the rest is what this computer has installed, read from its font folders
	// (Rust `fonts_list_installed`). Where that list is empty (the phones) or
	// fails, the known names below stand in: they work wherever the OS has
	// them. Any other family can still be entered by name.
	const BUILT_IN_FONTS = [
		{ name: 'JetBrains Mono (bundled)', value: 'JetBrains Mono' },
		{ name: 'System Monospace', value: 'monospace' },
	];
	const KNOWN_FONTS = [
		'SF Mono', 'Menlo', 'Monaco', 'Cascadia Code', 'Cascadia Mono', 'Consolas',
		'Courier New', 'DejaVu Sans Mono', 'Liberation Mono', 'Ubuntu Mono',
		'Noto Sans Mono', 'Source Code Pro',
	];
	/** Rows drawn per group at most: each row loads its own font to show it,
	 *  and a computer can have a thousand families. Search narrows. */
	const FONT_ROWS = 200;

	// Mixes ASCII, box-drawing, halfwidth kana, fullwidth latin and CJK so the
	// user can check fullwidth/halfwidth column alignment of the chosen font.
	const PREVIEW_TEXT =
		'user@server:~$ ls -la\n' +
		'0123456789  abcXYZ  {}[]()<>=>|\n' +
		'CJK 日本語 中文 한국어   Kana ｱｲｳﾊﾝｶｸ\n' +
		'全角ＡＢＣ 漢字  ┌─┬─┐│x│y│└─┴─┘';

	let fontSearch = $state('');
	let fontDropdownOpen = $state(false);
	/** Undefined until the first open asks; null when asking failed. */
	let installedFonts = $state<InstalledFont[] | null | undefined>(undefined);
	let fontsLoading = $state(false);
	let showAllFonts = $state(false);

	async function toggleFontDropdown(): Promise<void> {
		fontDropdownOpen = !fontDropdownOpen;
		if (!fontDropdownOpen || installedFonts !== undefined || fontsLoading) return;
		fontsLoading = true;
		try {
			installedFonts = await fontsListInstalled();
		} catch (e) {
			console.warn('Could not list installed fonts:', e);
			installedFonts = null;
		} finally {
			fontsLoading = false;
		}
	}

	/** The installed list when there is one, else the known names. */
	let haveInstalled = $derived(!!installedFonts && installedFonts.length > 0);

	let fontGroups = $derived.by(() => {
		const q = fontSearch.trim().toLowerCase();
		const hit = (name: string) => name.toLowerCase().includes(q);
		const builtIn = BUILT_IN_FONTS.filter((f) => hit(f.name));
		const bundled = new Set(BUILT_IN_FONTS.map((f) => f.value.toLowerCase()));
		let mono: string[];
		let other: string[] = [];
		if (haveInstalled) {
			const fonts = installedFonts!.filter((f) => !bundled.has(f.name.toLowerCase()) && hit(f.name));
			mono = fonts.filter((f) => f.monospace).map((f) => f.name);
			if (showAllFonts) other = fonts.filter((f) => !f.monospace).map((f) => f.name);
		} else {
			// Not asked yet or still loading: the list is about to come.
			mono = installedFonts === undefined ? [] : KNOWN_FONTS.filter(hit);
		}
		return { builtIn, mono, other };
	});

	// Full configurability: if the query names no listed font, offer it as a
	// custom font family. Proportional fonts count as listed even while
	// hidden, so the hint below points at the switch instead.
	let customFont = $derived.by(() => {
		const q = fontSearch.trim();
		if (!q) return null;
		const lq = q.toLowerCase();
		const names = [
			...BUILT_IN_FONTS.flatMap((f) => [f.name, f.value]),
			...(haveInstalled ? installedFonts!.map((f) => f.name) : KNOWN_FONTS),
		];
		return names.some((n) => n.toLowerCase() === lq) ? null : q;
	});

	/** Proportional fonts the search would find with the switch on. */
	let hiddenMatches = $derived.by(() => {
		const q = fontSearch.trim().toLowerCase();
		if (!q || showAllFonts || !haveInstalled) return 0;
		return installedFonts!.filter((f) => !f.monospace && f.name.toLowerCase().includes(q)).length;
	});

	function selectFont(value: string): void {
		updateSetting('fontFamily', value);
		fontDropdownOpen = false;
		fontSearch = '';
	}
</script>

<div class="tab-content">

	{#if part === 'appearance'}
	<div class="setting-section">
		<span class="section-label">{t('settings.theme')}</span>
		<div class="theme-cards">
			{#each shownChoices as choice (choice.id)}
				<button
					class="theme-card"
					class:active={selectedId === choice.id}
					onclick={() => selectTheme(choice.id)}
					title={choice.name}
				>
					{#if choice.swatch}
						<!-- Preview drawn from the theme's own tokens, so an installed
						     theme shows what it actually looks like. -->
						<div
							class="theme-preview"
							style="background: {choice.swatch.colors['bg-primary']}; border-color: {choice.swatch.colors.border};"
						>
							<span class="swatch-bar" style="background: {choice.swatch.colors['bg-secondary']};"></span>
							<span class="swatch-dot" style="background: {choice.swatch.colors.accent};"></span>
						</div>
					{:else}
						<!-- System: the light half of the pair over the dark one. -->
						<div
							class="theme-preview preview-system"
							style="background: linear-gradient(135deg, {pairLight.colors['bg-primary']} 50%, {pairDark.colors['bg-primary']} 50%); border-color: {pairLight.colors.border};"
						>
							<span class="swatch-dot pair-light" style="background: {pairLight.colors.accent};"></span>
							<span class="swatch-dot pair-dark" style="background: {pairDark.colors.accent};"></span>
						</div>
					{/if}
					<span class="theme-label">{choice.name}</span>
				</button>
			{/each}
		</div>
		{#if selectedId === SYSTEM}
			<div class="theme-pair">
				<p class="pair-hint">{t('themes.pair_hint')}</p>
				<div class="pair-row">
					<span class="pair-label">
						<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
							<circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
						</svg>
						{t('themes.pair_light')}
					</span>
					<Dropdown options={lightOptions} selected={pairLight.id} onchange={(v) => selectPair('themeLight', v)} />
				</div>
				<div class="pair-row">
					<span class="pair-label">
						<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
							<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
						</svg>
						{t('themes.pair_dark')}
					</span>
					<Dropdown options={darkOptions} selected={pairDark.id} onchange={(v) => selectPair('themeDark', v)} />
				</div>
				<p class="pair-now">{t(systemAppearance.dark ? 'themes.pair_now_dark' : 'themes.pair_now_light')}</p>
			</div>
		{/if}
		{#if foldedCount > 0}
			<button
				type="button"
				class="link-btn more-themes"
				aria-expanded={showAllThemes}
				onclick={() => (showAllThemes = !showAllThemes)}
			>
				{showAllThemes ? t('themes.fewer_builtin') : t('themes.more_builtin', { count: foldedCount })}
			</button>
		{/if}
	</div>

	<div class="setting-section">
		<div class="section-head">
			<span class="section-label">{t('themes.marketplace')}</span>
			<button class="link-btn" onclick={loadRegistry} disabled={loadingRegistry}>
				{t('themes.refresh')}
			</button>
		</div>

		{#if registryError}
			<p class="registry-msg">{t('themes.error')}</p>
			<p class="registry-detail">{registryError}</p>
		{:else if registry.length === 0}
			<p class="registry-msg">{t('themes.empty')}</p>
		{:else}
			{#if registry.length > PAGE}
				<input
					bind:value={themeQuery}
					class="theme-search"
					type="text"
					autocomplete="off"
					spellcheck="false"
					placeholder={t('themes.search')}
					aria-label={t('themes.search')}
				/>
			{/if}

			{#if matching.length === 0}
				<p class="registry-msg">{t('themes.no_match')}</p>
			{/if}

			<div class="theme-rows">
				{#each visible as entry (entry.id)}
					{@const isInstalled = installedIds.has(entry.id)}
					<div class="theme-row">
						<div class="row-info">
							<span class="row-name">{entry.name}</span>
							{#if entry.description}
								<span class="row-desc">{entry.description}</span>
							{/if}
						</div>
						{#if isInstalled}
							<button class="row-btn" onclick={() => remove(entry.id)}>{t('themes.remove')}</button>
						{:else}
							<button
								class="row-btn primary"
								onclick={() => install(entry)}
								disabled={installingId !== null}
							>
								{installingId === entry.id ? t('themes.installing') : t('themes.install')}
							</button>
						{/if}
					</div>
				{/each}
			</div>

			{#if matching.length > visible.length}
				<button class="show-more" onclick={() => (shown += PAGE)}>
					{t('themes.show_more', {
						shown: visible.length,
						total: matching.length
					})}
				</button>
			{/if}
		{/if}
	</div>

	<!-- The whole interface, not only the terminal: a laptop screen or a 4K
	     one at the wrong scale. A phone follows its own text size instead. -->
	{#if !isMobile()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.ui_zoom')}</span>
				<span class="setting-description">{t('settings.ui_zoom_hint', { in: actionLabel('zoom.in'), out: actionLabel('zoom.out'), reset: actionLabel('zoom.reset') })}</span>
			</div>
			<div class="setting-control size-stepper">
				<button class="size-btn" aria-label={t('keys.smaller')} disabled={uiZoom <= UI_ZOOM_STEPS[0]} onclick={() => stepUiZoom(-1)}>−</button>
				<button class="font-size-badge zoom-reset" title={t('settings.ui_zoom_reset')} onclick={() => stepUiZoom(0)}>{Math.round(uiZoom * 100)}%</button>
				<button class="size-btn" aria-label={t('keys.larger')} disabled={uiZoom >= UI_ZOOM_STEPS[UI_ZOOM_STEPS.length - 1]} onclick={() => stepUiZoom(1)}>+</button>
			</div>
		</div>
	{/if}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.compact_layout')}</span>
			<span class="setting-description">{t('settings.compact_layout_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.compactLayout}
				label={t('settings.compact_layout')}
				onchange={(v) => updateSetting('compactLayout', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('termbar.setting')}</span>
			<span class="setting-description">{t('termbar.setting_desc')}</span>
		</div>
		<div class="setting-control">
			<select class="log-field" value={settings.terminalBar} aria-label={t('termbar.setting')} onchange={(e) => updateSetting('terminalBar', (e.currentTarget as HTMLSelectElement).value as 'always' | 'hover' | 'hidden')}>
				<option value="always">{t('termbar.always')}</option>
				<option value="hover">{t('termbar.hover')}</option>
				<option value="hidden">{t('termbar.hidden')}</option>
			</select>
		</div>
	</div>
	{/if}

	{#if part === 'text'}
	<div class="setting-row font-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.terminal_font')}</span>
			<span class="setting-description">{t('settings.font_desc')}</span>
		</div>
		<div class="setting-control">
			<div class="font-picker">
				<button
					class="font-picker-btn"
					style:font-family={cssFontFamily(currentFont)}
					onclick={toggleFontDropdown}
				>
					{currentFont}
					<svg width="10" height="10" viewBox="0 0 10 10" fill="none" class="chevron" class:open={fontDropdownOpen}>
						<path d="M2 4l3 3 3-3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
					</svg>
				</button>

				{#if fontDropdownOpen}
					<div class="font-dropdown">
						<input
							class="font-search"
							type="text"
							placeholder={t('settings.search_fonts')}
							bind:value={fontSearch}
						/>
						<div class="font-list">
							{#snippet fontRow(name: string, value: string)}
								<button
									class="font-option"
									class:active={currentFont === value}
									style:font-family={cssFontFamily(value)}
									onclick={() => selectFont(value)}
								>
									{name}
								</button>
							{/snippet}
							{#snippet moreRows(total: number)}
								{#if total > FONT_ROWS}
									<div class="font-empty">{t('settings.more_fonts', { count: total - FONT_ROWS })}</div>
								{/if}
							{/snippet}

							{#if fontGroups.builtIn.length > 0}
								<div class="font-group-label">{t('settings.builtin_fonts')}</div>
								{#each fontGroups.builtIn as font (font.value)}
									{@render fontRow(font.name, font.value)}
								{/each}
							{/if}
							{#if fontsLoading}
								<div class="font-empty">{t('settings.fonts_loading')}</div>
							{/if}
							{#if fontGroups.mono.length > 0}
								<div class="font-group-label">
									{haveInstalled ? t('settings.installed_mono_fonts') : t('settings.system_fonts')}
								</div>
								{#each fontGroups.mono.slice(0, FONT_ROWS) as name (name)}
									{@render fontRow(name, name)}
								{/each}
								{@render moreRows(fontGroups.mono.length)}
							{/if}
							{#if fontGroups.other.length > 0}
								<div class="font-group-label">{t('settings.installed_other_fonts')}</div>
								{#each fontGroups.other.slice(0, FONT_ROWS) as name (name)}
									{@render fontRow(name, name)}
								{/each}
								{@render moreRows(fontGroups.other.length)}
							{/if}
							{#if customFont}
								<div class="font-group-label">{t('settings.custom_font')}</div>
								{@render fontRow(t('settings.use_font', { font: customFont }), customFont)}
							{/if}
							{#if hiddenMatches > 0}
								<div class="font-empty">{t('settings.fonts_hidden', { count: hiddenMatches })}</div>
							{:else if !fontsLoading && fontGroups.builtIn.length + fontGroups.mono.length + fontGroups.other.length === 0 && !customFont}
								<div class="font-empty">{t('settings.no_fonts')}</div>
							{/if}
						</div>
						{#if haveInstalled}
							<label class="font-all">
								<input type="checkbox" bind:checked={showAllFonts} />
								{t('settings.show_all_fonts')}
							</label>
						{/if}
					</div>
				{/if}
			</div>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.font_size')}</span>
			<span class="setting-description">{t('settings.font_size_hint')}</span>
		</div>
		<div class="setting-control size-stepper">
			<button class="size-btn" aria-label={t('keys.smaller')} disabled={currentSize <= 8} onclick={() => stepFontSize(-1)}>A−</button>
			<span class="font-size-badge">{currentSize}px</span>
			<button class="size-btn" aria-label={t('keys.larger')} disabled={currentSize >= 32} onclick={() => stepFontSize(1)}>A+</button>
		</div>
	</div>

	<div class="font-preview-box">
		<span class="preview-label">{t('settings.preview')} — {currentFont} @ {currentSize}px</span>
		<pre class="preview-pre" style:font-family={cssFontFamily(currentFont)} style:font-size="{currentSize}px">{PREVIEW_TEXT}</pre>
	</div>

	<TerminalThemeSettings />

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('cursor.style')}</span>
			<span class="setting-description">{t('cursor.style_desc')}</span>
		</div>
		<div class="setting-control">
			<Dropdown
				options={cursorStyleOptions}
				selected={settings.terminalCursorStyle}
				onchange={(v) => updateSetting('terminalCursorStyle', cursorStyle(v))}
			/>
		</div>
	</div>

	{#if settings.terminalCursorStyle === 'bar'}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('cursor.width')}</span>
				<span class="setting-description">{t('cursor.width_desc')}</span>
			</div>
			<div class="setting-control">
				<Dropdown
					options={cursorWidthOptions}
					selected={String(settings.terminalCursorWidth)}
					onchange={(v) => updateSetting('terminalCursorWidth', cursorWidth(Number(v)))}
				/>
			</div>
		</div>
	{/if}

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('cursor.blink')}</span>
			<span class="setting-description">{t('cursor.blink_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.terminalCursorBlink}
				label={t('cursor.blink')}
				onchange={(v) => updateSetting('terminalCursorBlink', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('cursor.inactive')}</span>
			<span class="setting-description">{t('cursor.inactive_desc')}</span>
		</div>
		<div class="setting-control">
			<Dropdown
				options={cursorInactiveOptions}
				selected={settings.terminalCursorInactive}
				onchange={(v) => updateSetting('terminalCursorInactive', inactiveCursorStyle(v))}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('highlight.setting')}</span>
			<span class="setting-description">{t('highlight.setting_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.terminalHighlight}
				label={t('highlight.setting')}
				onchange={(v) => updateSetting('terminalHighlight', v)}
			/>
		</div>
	</div>

	{#if settings.terminalHighlight}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('highlight.kinds')}</span>
				<span class="setting-description">{t('highlight.kinds_desc')}</span>
			</div>
			<div class="highlight-kinds" role="group" aria-label={t('highlight.kinds')}>
				{#each HIGHLIGHT_KINDS as kind (kind)}
					<label class="highlight-kind {kind}">
						<input type="checkbox" checked={shownKinds.includes(kind)} onchange={(e) => toggleKind(kind, e.currentTarget.checked)} />
						{t(`highlight.kind_${kind}`)}
					</label>
				{/each}
			</div>
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('highlight.custom')}</span>
				<span class="setting-description">{t('highlight.custom_desc')}</span>
			</div>
			<div class="highlight-custom">
				<textarea
					class="highlight-custom-input"
					rows="4"
					spellcheck="false"
					autocomplete="off"
					autocapitalize="off"
					aria-label={t('highlight.custom')}
					aria-invalid={customErrors.length > 0}
					placeholder={'/vlan\\s*\\d+/i\n#ff8800 \\bCRC errors?\\b'}
					value={settings.terminalHighlightCustom}
					oninput={(e) => updateSetting('terminalHighlightCustom', e.currentTarget.value)}
				></textarea>
				{#each customErrors as err (err.line)}
					<span class="highlight-custom-error">{t('highlight.custom_error', { line: err.line, message: err.message })}</span>
				{/each}
			</div>
		</div>
	{/if}
	{/if}

	{#if part === 'scrollback'}
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.scrollback')}</span>
			<span class="setting-description">{t('settings.scrollback_desc')}</span>
		</div>
		<div class="setting-control">
			<Dropdown
				options={scrollbackOptions}
				selected={String(settings.terminalScrollback)}
				onchange={(v) => updateSetting('terminalScrollback', scrollbackSetting(Number(v)))}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.line_numbers')}</span>
			<span class="setting-description">{t('settings.line_numbers_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.terminalLineNumbers}
				label={t('settings.line_numbers')}
				onchange={(v) => updateSetting('terminalLineNumbers', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.timestamps')}</span>
			<span class="setting-description">{t('settings.timestamps_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.terminalTimestamps}
				label={t('settings.timestamps')}
				onchange={(v) => updateSetting('terminalTimestamps', v)}
			/>
		</div>
	</div>
	{/if}

	{#if part === 'term-adv'}
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.shell_colors')}</span>
			<span class="setting-description">{t('settings.shell_colors_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.typeShellSetup}
				label={t('settings.shell_colors')}
				onchange={(v) => updateSetting('typeShellSetup', v)}
			/>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.login_message')}</span>
			<span class="setting-description">{t('settings.login_message_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle
				hideLabel
				checked={settings.showLoginMessage}
				label={t('settings.login_message')}
				onchange={(v) => updateSetting('showLoginMessage', v)}
			/>
		</div>
	</div>
	{/if}

	{#if part === 'logging'}
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.log_mode')}</span>
			<span class="setting-description">{t('settings.log_mode_desc')}</span>
		</div>
		<div class="setting-control">
			<select
				class="log-field"
				aria-label={t('settings.log_mode')}
				value={settings.sessionLogMode}
				onchange={(e) =>
					updateSetting('sessionLogMode', e.currentTarget.value as 'off' | 'printable' | 'all')}
			>
				<option value="off">{t('settings.log_off')}</option>
				<option value="printable">{t('settings.log_printable')}</option>
				<option value="all">{t('settings.log_all')}</option>
			</select>
		</div>
	</div>

	{#if settings.sessionLogMode !== 'off'}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.log_folder')}</span>
				<!-- Android: only the app's own storage can be written without a
				     system picker, so the folder is fixed there. -->
				<span class="setting-description">
					{isMobile() ? logDefaultDir : t('settings.log_folder_desc')}
				</span>
			</div>
			{#if !isMobile()}
				<div class="setting-control log-folder">
					<input
						class="log-field"
						type="text"
						spellcheck="false"
						placeholder={logDefaultDir}
						title={settings.sessionLogFolder || logDefaultDir}
						aria-label={t('settings.log_folder')}
						value={settings.sessionLogFolder}
						onchange={(e) => updateSetting('sessionLogFolder', e.currentTarget.value.trim())}
					/>
					<button class="log-browse" type="button" onclick={browseLogFolder}>
						{t('settings.log_browse')}
					</button>
				</div>
			{/if}
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.log_name')}</span>
				<span class="setting-description">{t('settings.log_name_desc')}</span>
			</div>
			<div class="setting-control">
				<input
					class="log-field"
					type="text"
					spellcheck="false"
					placeholder={DEFAULT_LOG_NAME}
					aria-label={t('settings.log_name')}
					value={settings.sessionLogName}
					onchange={(e) => updateSetting('sessionLogName', e.currentTarget.value.trim() || DEFAULT_LOG_NAME)}
				/>
			</div>
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.log_append')}</span>
				<span class="setting-description">{t('settings.log_append_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.sessionLogAppend}
					label={t('settings.log_append')}
					onchange={(v) => updateSetting('sessionLogAppend', v)}
				/>
			</div>
		</div>

		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.log_header')}</span>
				<span class="setting-description">{t('settings.log_header_desc')}</span>
			</div>
			<div class="setting-control">
				<Toggle
					hideLabel
					checked={settings.sessionLogHeader}
					label={t('settings.log_header')}
					onchange={(v) => updateSetting('sessionLogHeader', v)}
				/>
			</div>
		</div>
	{/if}
	{/if}

	{#if part === 'history'}
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('history.settings_mode')}</span>
			<span class="setting-description">{t('history.settings_mode_desc')}</span>
		</div>
		<div class="setting-control">
			<select
				class="log-field"
				aria-label={t('history.settings_mode')}
				value={settings.historyMode}
				onchange={(e) => updateSetting('historyMode', e.currentTarget.value as 'off' | 'commands' | 'output')}
			>
				<option value="off">{t('history.mode_off')}</option>
				<option value="commands">{t('history.mode_commands')}</option>
				<option value="output">{t('history.mode_output')}</option>
			</select>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('history.settings_keep')}</span>
			<span class="setting-description">{t('history.settings_keep_desc')}</span>
		</div>
		<div class="setting-control history-keep">
			<label>
				{t('history.settings_output_days')}
				<input class="log-field num" type="number" min="0" value={settings.historyOutputDays}
					onchange={(e) => updateSetting('historyOutputDays', Math.max(0, Math.floor(Number(e.currentTarget.value) || 0)))} />
			</label>
			<label>
				{t('history.settings_command_days')}
				<input class="log-field num" type="number" min="0" value={settings.historyCommandDays}
					onchange={(e) => updateSetting('historyCommandDays', Math.max(0, Math.floor(Number(e.currentTarget.value) || 0)))} />
			</label>
			<label>
				{t('history.settings_max_mb')}
				<input class="log-field num" type="number" min="0" step="256" value={settings.historyMaxMb}
					onchange={(e) => updateSetting('historyMaxMb', Math.max(0, Math.floor(Number(e.currentTarget.value) || 0)))} />
			</label>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('history.settings_patterns')}</span>
			<span class="setting-description">{t('history.settings_patterns_desc')}</span>
		</div>
		<div class="setting-control">
			<textarea
				class="log-field patterns"
				rows="3"
				spellcheck="false"
				aria-label={t('history.settings_patterns')}
				placeholder="CORP-[0-9]{6}"
				value={settings.historyPatterns}
				onchange={(e) => updateSetting('historyPatterns', e.currentTarget.value)}
			></textarea>
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('history.settings_stored')}</span>
			<span class="setting-description">
				{#if historyInfo}
					{t('history.settings_stored_desc', { commands: String(historyInfo.commands), outputs: String(historyInfo.outputs), mb: (historyInfo.bytes / 1048576).toFixed(1) })}
				{:else}
					{t('history.settings_stored_locked')}
				{/if}
			</span>
		</div>
		<div class="setting-control">
			<button class="log-browse" type="button" disabled={!historyInfo || historyInfo.runs === 0} onclick={forgetAllHistory}>
				{forgetArmed ? t('history.forget_all_confirm') : t('history.forget_all')}
			</button>
		</div>
	</div>
	{/if}

	{#if part === 'rdp'}
	{#if !isMobile()}
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('settings.freerdp_path')}</span>
				<span class="setting-description">{t('settings.freerdp_path_desc')}</span>
				<span class="setting-description freerdp-status" role="status">
					{#if !detectingFreerdp}
						{freerdpFound
							? t('settings.freerdp_found', { path: freerdpFound })
							: t(settings.freerdpPath ? 'settings.freerdp_path_bad' : 'settings.freerdp_not_found')}
					{/if}
				</span>
			</div>
			<div class="setting-control log-folder">
				<input
					class="log-field"
					type="text"
					spellcheck="false"
					placeholder={t('settings.freerdp_path_auto')}
					aria-label={t('settings.freerdp_path')}
					value={settings.freerdpPath}
					onchange={(e) => updateSetting('freerdpPath', e.currentTarget.value.trim())}
				/>
				<button class="log-browse" type="button" onclick={browseFreerdp}>{t('settings.log_browse')}</button>
			</div>
		</div>
	{/if}
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.rdp_hw_render')}</span>
			<span class="setting-description">{t('settings.rdp_hw_render_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle hideLabel checked={settings.rdpHardwareRendering} label={t('settings.rdp_hw_render')} onchange={(v) => updateSetting('rdpHardwareRendering', v)} />
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.rdp_fs_button')}</span>
			<span class="setting-description">{t('settings.rdp_fs_button_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle hideLabel checked={settings.rdpFullscreenButton} label={t('settings.rdp_fs_button')} onchange={(v) => updateSetting('rdpFullscreenButton', v)} />
		</div>
	</div>

	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.rdp_bar_pinned')}</span>
			<span class="setting-description">{t('settings.rdp_bar_pinned_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle hideLabel checked={settings.rdpBarPinned} label={t('settings.rdp_bar_pinned')} onchange={(v) => updateSetting('rdpBarPinned', v)} />
		</div>
	</div>
	{/if}

	{#if part === 'rdp-adv'}
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.rdp_gfx')}</span>
			<span class="setting-description">{t('settings.rdp_gfx_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle hideLabel checked={settings.rdpGraphicsPipeline} label={t('settings.rdp_gfx')} onchange={(v) => updateSetting('rdpGraphicsPipeline', v)} />
		</div>
	</div>
	<div class="setting-row">
		<div class="setting-info">
			<span class="setting-label">{t('settings.rdp_gfx_new')}</span>
			<span class="setting-description">{t('settings.rdp_gfx_new_desc')}</span>
		</div>
		<div class="setting-control">
			<Toggle hideLabel checked={settings.rdpNewGraphicsFormats} label={t('settings.rdp_gfx_new')} onchange={(v) => updateSetting('rdpNewGraphicsFormats', v)} />
		</div>
	</div>
	{/if}

</div>

<style>
	.history-keep {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
		font-size: 0.75rem;
	}

	.history-keep label {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.log-field.num {
		width: 90px;
	}

	.log-field.patterns {
		font-family: var(--font-mono, monospace);
		resize: vertical;
	}

	.log-field {
		width: 100%;
		min-width: 0;
		padding: 7px 10px;
		border-radius: var(--radius-btn);
		border: 1px solid var(--color-border-strong);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-family: inherit;
		font-size: 0.8125rem;
	}

	.log-field:focus {
		outline: none;
		border-color: var(--color-accent);
	}

	.freerdp-status {
		overflow-wrap: anywhere;
	}

	.log-folder {
		display: flex;
		gap: var(--space-2);
	}

	.log-browse {
		flex-shrink: 0;
		padding: 7px 12px;
		border-radius: var(--radius-btn);
		border: 1px solid var(--color-border);
		background: var(--color-surface-sunken);
		color: var(--color-text-primary);
		font-family: inherit;
		font-size: 0.8125rem;
		cursor: pointer;
	}

	.log-browse:hover {
		border-color: var(--color-accent);
	}

	.setting-section {
		padding: 12px 0;
		border-bottom: 1px solid var(--color-border);
	}

	.section-label {
		display: block;
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--color-text-primary);
		margin-bottom: 12px;
	}

	.swatch-bar {
		position: absolute;
		left: 0;
		top: 0;
		bottom: 0;
		width: 34%;
	}

	.swatch-dot {
		position: absolute;
		right: 7px;
		bottom: 7px;
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}

	.section-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
	}

	/* The label keeps its gap to the rows below; Refresh lines up with it
	   instead of with that margin. */
	.section-head {
		margin-bottom: 12px;
	}

	.section-head .section-label {
		margin-bottom: 0;
	}

	.link-btn {
		padding: 0;
		font-family: var(--font-sans);
		font-size: var(--text-xs);
		color: var(--color-accent);
		background: none;
		border: none;
		cursor: pointer;
	}

	.link-btn:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.registry-msg {
		margin: var(--space-2) 0 0;
		font-size: var(--text-sm);
		color: var(--color-text-secondary);
	}

	.registry-detail {
		margin: var(--space-1) 0 0;
		font-size: var(--text-xs);
		color: var(--color-text-tertiary);
	}

	.theme-search {
		width: 100%;
		padding: 7px var(--space-2);
		margin-bottom: var(--space-2);
		font-size: var(--text-sm);
		font-family: inherit;
		color: var(--color-text-primary);
		background: var(--color-surface-sunken);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-sm);
		outline: none;
	}

	.theme-search:focus {
		border-color: var(--color-accent);
	}

	.theme-search::placeholder {
		color: var(--color-text-tertiary);
	}

	.show-more {
		width: 100%;
		margin-top: var(--space-2);
		padding: 8px;
		font-size: var(--text-xs);
		font-family: inherit;
		color: var(--color-text-secondary);
		background: none;
		border: 1px dashed var(--color-border);
		border-radius: var(--radius-sm);
		cursor: pointer;
	}

	.show-more:hover {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.theme-rows {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		margin-top: var(--space-2);
	}

	.theme-row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		background: var(--color-bg-secondary);
	}

	.row-info {
		display: flex;
		flex-direction: column;
		gap: 2px;
		flex: 1;
		min-width: 0;
	}

	.row-name {
		font-size: var(--text-sm);
		font-weight: 500;
		color: var(--color-text-primary);
	}

	.row-desc {
		font-size: var(--text-xs);
		color: var(--color-text-secondary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.row-btn {
		flex: none;
		padding: var(--space-1) var(--space-3);
		font-family: var(--font-sans);
		font-size: var(--text-xs);
		font-weight: 500;
		color: var(--color-text-primary);
		background: var(--color-surface-hover);
		border: none;
		border-radius: var(--radius-sm);
		cursor: pointer;
	}

	/* Tinted, not filled: one solid accent button per row made a list of
	   themes a column of competing calls to action. */
	.row-btn.primary {
		color: var(--color-accent);
		background: color-mix(in srgb, var(--color-accent) 12%, transparent);
	}

	.row-btn.primary:hover:not(:disabled) {
		background: color-mix(in srgb, var(--color-accent) 20%, transparent);
	}

	.row-btn:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.more-themes { display: inline-block; margin-top: 10px; font-size: var(--text-sm); }

	.theme-cards {
		display: grid;
		/* 20-odd themes ship now: as many across as fit, rather than a tall
		   column of threes. */
		grid-template-columns: repeat(auto-fill, minmax(112px, 1fr));
		gap: 10px;
	}

	.theme-card {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		padding: 12px;
		background: transparent;
		border: 2px solid var(--color-border);
		border-radius: var(--radius-card);
		cursor: pointer;
		transition: border-color var(--duration-default) var(--ease-default), background-color var(--duration-default) var(--ease-default);
		font-family: var(--font-sans);
	}

	.theme-card:hover { background-color: var(--color-surface-hover); }
	.theme-card.active { border-color: var(--color-accent); background-color: rgba(10, 132, 255, 0.08); }

	.theme-preview {
		position: relative;
		overflow: hidden;
		width: 48px; height: 48px; border-radius: 10px;
		display: flex; align-items: center; justify-content: center;
	}

	.preview-system { border: 1px solid; }
	.preview-system .pair-light { top: 9px; left: 9px; right: auto; bottom: auto; }
	.preview-system .pair-dark { bottom: 9px; right: 9px; }

	.theme-pair {
		margin-top: 12px;
		padding: 12px 14px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card);
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.pair-hint, .pair-now { margin: 0; font-size: var(--text-sm); color: var(--color-text-secondary); }
	.pair-now { color: var(--color-text-tertiary); }
	.pair-row { display: flex; align-items: center; gap: 12px; }
	.pair-label {
		display: inline-flex; align-items: center; gap: 6px;
		min-width: 110px; font-size: var(--text-sm); color: var(--color-text-primary);
	}
	.pair-row :global(.dropdown) { flex: 1; min-width: 0; max-width: 320px; }

	.theme-label { font-size: 0.75rem; font-weight: 500; color: var(--color-text-primary); }

	.font-row { align-items: flex-start; }

	.font-size-badge {
		font-size: 0.875rem;
		font-weight: 600;
		color: var(--color-accent);
		font-variant-numeric: tabular-nums;
	}

	/* The percentage is the reset: clicking 125% puts it back to 100%. */
	.zoom-reset {
		min-width: 3.5em;
		padding: 4px 2px;
		border: none;
		border-radius: var(--radius-sm);
		background: none;
		font-family: inherit;
		text-align: center;
		cursor: pointer;
	}

	.zoom-reset:hover {
		background: var(--color-surface-hover);
	}

	.size-stepper {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.size-btn {
		min-width: 40px;
		height: 32px;
		padding: 0 8px;
		font-family: var(--font-mono);
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border);
		border-radius: 6px;
		cursor: pointer;
	}

	.size-btn:hover:not(:disabled) {
		background: var(--color-surface-hover);
	}

	.size-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}

	/* Font picker */
	.font-picker { position: relative; }

	.font-picker-btn {
		display: flex; align-items: center; justify-content: space-between; gap: 8px;
		width: 100%; padding: 7px 10px;
		background: var(--color-bg-primary); border: 1px solid var(--color-border);
		border-radius: 6px; color: var(--color-text-primary);
		font-size: 0.8125rem; cursor: pointer; text-align: left;
		transition: border-color 0.15s ease;
	}
	.font-picker-btn:hover { border-color: var(--color-accent); }

	.chevron { transition: transform 0.15s ease; flex-shrink: 0; color: var(--color-text-secondary); }
	.chevron.open { transform: rotate(180deg); }

	/* Opens under the button, within the window: right-aligned it went off
	   the left edge of a phone, where the button sits at the left. */
	.font-dropdown {
		position: absolute; top: calc(100% + 4px); right: 0; width: min(260px, calc(100vw - 64px));
		background: var(--color-bg-elevated); border: 1px solid var(--color-border);
		border-radius: 8px; box-shadow: var(--shadow-elevated);
		z-index: 100; overflow: hidden;
	}

	.font-search {
		width: 100%; padding: 8px 10px; border: none;
		border-bottom: 1px solid var(--color-border);
		background: transparent; color: var(--color-text-primary);
		font-family: var(--font-sans); font-size: 0.75rem; outline: none;
		box-sizing: border-box;
	}
	.font-search::placeholder { color: var(--color-text-secondary); opacity: 0.5; }

	@media (max-width: 640px) {
		.font-dropdown { left: 0; right: auto; }
	}

	.font-list {
		max-height: min(280px, 40vh); overflow-y: auto; padding: 4px 0;
	}

	.font-group-label {
		padding: 6px 12px 3px; font-size: 0.625rem; font-weight: 600;
		text-transform: uppercase; letter-spacing: 0.05em;
		color: var(--color-text-secondary); opacity: 0.6;
	}

	.font-option {
		display: block; width: 100%; padding: 6px 12px; border: none;
		background: transparent; color: var(--color-text-primary);
		font-size: 0.8125rem; cursor: pointer; text-align: left;
		transition: background-color 0.1s ease;
	}
	.font-option:hover { background-color: var(--color-surface-hover); }
	.font-option.active { background-color: rgba(10, 132, 255, 0.12); color: var(--color-accent); }

	/* Preview box */
	.font-preview-box {
		margin-top: 12px;
		border: 1px solid var(--color-border);
		border-radius: 8px;
		overflow: hidden;
	}

	.preview-label {
		display: block; font-size: 0.625rem; font-weight: 600;
		text-transform: uppercase; letter-spacing: 0.05em;
		color: var(--color-text-secondary); padding: 8px 12px 0;
		font-family: var(--font-sans);
	}

	.preview-pre {
		margin: 0;
		padding: 12px;
		min-height: 96px;
		line-height: 1.6;
		white-space: pre;
		overflow-x: auto;
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border-radius: 0 0 8px 8px;
	}

	.font-all {
		display: flex; align-items: center; gap: 6px;
		padding: 7px 12px; border-top: 1px solid var(--color-border);
		font-family: var(--font-sans); font-size: 0.75rem;
		color: var(--color-text-secondary); cursor: pointer;
	}

	.font-empty {
		padding: 10px 12px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		font-family: var(--font-sans);
	}


	/* The kinds take the row's full width under its label, not the narrow
	   control column. */
	.highlight-kinds {
		grid-column: 1 / -1;
		/* Over the settings rows' own end alignment for a bare control. */
		justify-self: stretch !important;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
		gap: 8px 16px;
		margin-top: 10px;
	}

	.highlight-custom {
		grid-column: 1 / -1;
		justify-self: stretch !important;
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin-top: 10px;
	}

	.highlight-custom-input {
		width: 100%;
		box-sizing: border-box;
		resize: vertical;
		min-height: 84px;
		padding: 8px 10px;
		font-family: var(--font-mono, monospace);
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		background: var(--color-bg-tertiary, var(--color-bg-secondary));
		border: 1px solid var(--color-border);
		border-radius: 6px;
	}

	.highlight-custom-input:focus {
		outline: none;
		border-color: var(--color-accent);
	}

	.highlight-custom-input[aria-invalid='true'] {
		border-color: var(--color-danger);
	}

	.highlight-custom-error {
		font-size: 0.75rem;
		color: var(--color-danger);
	}

	.highlight-kind {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: var(--text-sm, 0.8125rem);
		color: var(--color-text-primary);
		cursor: pointer;
	}
</style>
