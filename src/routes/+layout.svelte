<script lang="ts">
	import { pushHistoryRetention } from '#lib/state/history.svelte.js';
	import { applyTheme, themeFor, loadInstalledThemes } from '#lib/state/theme.svelte.js';
	import { loadCustomTerminalThemes } from '#lib/state/terminal-themes.svelte.js';
	import type { Snippet } from 'svelte';
	import '../app.css';
	import AppShell from '#lib/components/layout/AppShell.svelte';
	import FloatShell from '#lib/components/layout/FloatShell.svelte';
	import { isFloatWindow, startWindows, announceLock, announceSettings, onOtherSettings, takeKeptWindows } from '#lib/state/windows.svelte.js';
	import { openKept } from '#lib/state/layouts.svelte.js';
	import { readState } from '#lib/state/vault.svelte.js';
	import { listen } from '@tauri-apps/api/event';
	import WelcomeScreen from '#lib/components/setup/WelcomeScreen.svelte';
	import { setupWizard } from '#lib/state/setup-wizard.svelte.js';
	import CommunityPrompt from '#lib/components/shared/CommunityPrompt.svelte';
	import { SHOW_COMMUNITY } from '#lib/brand.js';
	import LockScreen from '#lib/components/shared/LockScreen.svelte';
	import CommandPalette from '#lib/components/shared/CommandPalette.svelte';
	import AppContextMenu from '#lib/components/shared/AppContextMenu.svelte';
	import AskHost from '#lib/components/shared/AskHost.svelte';
	import PasswordPrompt from '#lib/components/shared/PasswordPrompt.svelte';
	import { applyUiZoom } from '#lib/state/ui-zoom.js';
	import { startAutoLock } from '#lib/state/autolock.svelte.js';
	import { startTray } from '#lib/state/tray.svelte.js';
	import { loadSettings, getSettings, syncTraySettings, recordLaunch, markFeaturesSeen, onSettingsSaved } from '#lib/state/settings.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { syncDevopsTools } from '#lib/state/devops.svelte.js';
	import { loadAISettings } from '#lib/state/ai.svelte.js';
	import { initShortcuts, cleanupShortcuts } from '#lib/state/shortcuts.svelte.js';
	import { startupUpdateCheck, startPeriodicChecks, stopPeriodicChecks } from '#lib/state/updater.svelte.js';
	import { changeLocale, t } from '#lib/state/i18n.svelte.js';
	import { loadSnippets } from '#lib/state/snippets.svelte.js';
	import { biometricName, biometricPrompt, vaultState } from '#lib/state/vault.svelte.js';
	import { unlockMethods } from '#lib/ipc/vault.js';
	import * as mcp from '#lib/state/mcp.svelte.js';
	import * as share from '#lib/state/share.svelte.js';
	import { unlockCredentialKeys, setKeysUnlockEnabled } from '#lib/ipc/savedCredentials.js';
	import { installSelectMenus } from '#lib/utils/selectMenu.js';
	import { installTooltips } from '#lib/utils/tooltip.js';
	import { initScreenEdges } from '#lib/edges.js';
	import { initDevice } from '#lib/device.svelte.js';
	import { initKeyboardViewport } from '#lib/keyboard-viewport.js';
	import { onMount } from 'svelte';
	import { checkConfiguredLogFolder } from '#lib/state/log-folder.svelte.js';
	import { startPreferenceSync } from '#lib/state/preference-sync.svelte.js';

	let { children }: { children: Snippet } = $props();

	// A seamless RemoteApp's window is as bare as the editor's: no tray, lock screen or setup.
	const isEditorWindow = typeof window !== 'undefined' && ['editor', 'railview'].some((k) => new URLSearchParams(window.location.search).has(k));
	const settings = getSettings();
	let settingsLoaded = $state(false);
	/** A floating session window (#lib/state/windows): a slim shell, and the main window owns the rest. */
	const isFloat = isFloatWindow();

	// Settings changed in another window: read again, without telling the
	// others back (that is where they came from).
	let quiet = false;
	function loadQuietly(): void {
		quiet = true;
		loadSettings();
		setTimeout(() => (quiet = false), 100);
	}

	/** Floating windows open when the app last closed (see takeKeptWindows). */
	let kept = isFloat ? [] : takeKeptWindows();

	function startFloat(): () => void {
		loadSettings();
		settingsLoaded = true;
		loadInstalledThemes();
		void readState();
		// The main window locked or opened the vault.
		const stopLock = listen('float-lock', () => void readState());
		onOtherSettings(loadQuietly);
		onSettingsSaved(() => {
			if (!quiet) announceSettings();
		});
		const stopSelectMenus = installSelectMenus();
		const stopTooltips = installTooltips();
		void startWindows();
		window.dispatchEvent(new CustomEvent('app-ready'));
		return () => {
			void stopLock.then((stop) => stop());
			stopSelectMenus();
			stopTooltips();
		};
	}

	// Compact layout (Settings > Appearance): one class on the page that the
	// rows read, so it applies the moment it is switched.
	$effect(() => {
		document.documentElement.classList.toggle('compact', settings.compactLayout);
	});

	onMount(() => {
		if (isFloat) return startFloat();
		loadSettings();
		onOtherSettings(loadQuietly);
		onSettingsSaved(() => {
			if (!quiet) announceSettings();
		});
		void startWindows();
		settingsLoaded = true;
		startAutoLock();
		// The tray's tooltip and menu follow the main window's state.
		if (!isEditorWindow) startTray();
		const stopPreferenceSync = !isEditorWindow ? startPreferenceSync() : undefined;
		// After loadSettings, or the count read back as zero and reset itself
		// every launch, which would hold the community prompt off forever.
		recordLaunch();
		// Before the theme effect below resolves settings.themeId: until these
		// are in, themeState.all is just the two built-ins, so an installed
		// theme silently fell back to dark/light on every launch.
		loadInstalledThemes();
		syncDevopsTools();
		// Someone updating has not met the DevOps switches; say once where
		// Databases lives. A fresh install is seeded with 'devops' as seen.
		if (settings.setupComplete && !settings.seenFeatures.includes('devops') && !settings.seenFeatures.includes('devops-notice')) {
			// After the locale has loaded and the window has painted.
			setTimeout(() => addToast(t('devops.new_notice'), 'info', 12000), 2500);
			markFeaturesSeen(['devops-notice']);
		}
		// The same, once, for Containers and Kubernetes (0.7.4).
		if (settings.setupComplete && !settings.seenFeatures.includes('containers-k8s') && !settings.seenFeatures.includes('containers-k8s-notice')) {
			setTimeout(() => addToast(t('devops.containers_k8s_notice'), 'info', 12000), 3500);
			markFeaturesSeen(['containers-k8s-notice']);
		}
		loadAISettings();
		initShortcuts();
		// Every <select> opens a theme-drawn list, not the platform popup (#110).
		const stopSelectMenus = installSelectMenus();
		// Every title shows in a theme-drawn tooltip, not the platform's yellow one.
		const stopTooltips = installTooltips();
		// Android's status bar, navigation bar and camera cutout, for --safe-*.
		initScreenEdges();
		// Android: a hardware keyboard, and Samsung DeX.
		initDevice();
		// iOS: the app fits above the on-screen keyboard instead of scrolling up.
		const stopKeyboardViewport = initKeyboardViewport();
		startupUpdateCheck();
		startPeriodicChecks();

		// The Svelte app has loaded and mounted — signal real readiness. The
		// preloader listener in app.html reacts to this (no timer, no fake delay).
		window.dispatchEvent(new CustomEvent('app-ready'));

		return () => {
			cleanupShortcuts();
			stopSelectMenus();
			stopTooltips();
			stopKeyboardViewport();
			stopPeriodicChecks();
			stopPreferenceSync?.();
		};
	});

	$effect(() => {
		if (settingsLoaded) applyUiZoom();
	});

	$effect(() => {
		if (settingsLoaded && !isFloat && !isEditorWindow) void syncTraySettings();
	});

	$effect(() => {
		if (settingsLoaded && !isFloat && !isEditorWindow) pushHistoryRetention();
	});

	$effect(() => {
		if (settingsLoaded && !isFloat && !isEditorWindow) checkConfiguredLogFolder();
	});

	$effect(() => {
		changeLocale(settings.locale);
	});

	// Load snippets once vault is unlocked
	$effect(() => {
		if (!vaultState.locked) {
			loadSnippets();
			// Custom terminal themes live in the settings vault, which syncs.
			loadCustomTerminalThemes();
		}
	});

	// MCP settings live in the vault, so they can only be restored once it is
	// open — reading them at process start would silently find nothing. This
	// restores the agent, mode and port, and restarts the server if it was
	// explicitly enabled before. Sharing is never restored: bringing the
	// server back honours a decision the user made, whereas bringing a shared
	// session back would be making one for them.
	let mcpRestored = false;
	$effect(() => {
		if (isFloat || isEditorWindow) return;
		if (!vaultState.locked && !mcpRestored) {
			mcpRestored = true;
			mcp.restore();
			// Same reason: the switch and the ICE list live in the vault.
			share.load();
		}
	});

	// Settings → Security → "Unlock credential keys at start": once the vault
	// is open, ask for the passphrases of the saved credentials' keys. Asked
	// again after a lock, which forgets them, and at once when the setting is
	// switched on. Off: the backend forgets what it kept.
	let keysAsked = false;
	$effect(() => {
		if (isEditorWindow || isFloat) return;
		const wanted = settings.unlockKeysAtStart;
		const open = !vaultState.locked && !vaultState.held;
		if (!wanted || !open) {
			keysAsked = false;
			if (!wanted) void setKeysUnlockEnabled(false).catch(() => {});
			return;
		}
		if (keysAsked) return;
		keysAsked = true;
		const biometric = settings.unlockKeysWithBiometrics
			? {
					open: biometricPrompt(t('security.keys_biometric_open')),
					save: biometricPrompt(t('security.keys_biometric_save'))
				}
			: undefined;
		unlockCredentialKeys(biometric)
			.then(async (r) => {
				if (r.unlocked > 0) addToast(t('security.keys_unlocked', { count: r.unlocked }), 'success', 3000);
				if (r.remember_error) addToast(t('toast.secret_save_failed', { error: r.remember_error }), 'error', 6000);
				if (r.remembered > 0) {
					const m = await unlockMethods().catch(() => null);
					const method = biometricName(m?.platform, m?.platform_label);
					addToast(t('security.keys_biometric_remembered', { method, count: r.remembered }), 'success', 4000);
				}
			})
			.catch((err) => console.error('Unlocking credential keys failed:', err));
	});

	$effect(() => {
		document.documentElement.style.setProperty('--app-font-size', `${settings.fontSize}px`);
	});

	// Floating windows cover their sessions while the vault is held locked.
	$effect(() => {
		const held = vaultState.held;
		const locked = vaultState.locked;
		if (!isFloat && !isEditorWindow) announceLock(held || locked);
	});

	// Settings > General > "Reopen floating windows": once the vault is open
	// (their sessions need it), the windows open when the app last closed
	// open again, one after another, where they were.
	$effect(() => {
		if (isFloat || isEditorWindow || vaultState.locked || kept.length === 0) return;
		const list = kept;
		kept = [];
		if (!settings.reopenFloatWindows) return;
		// After the session list has mounted and can connect.
		setTimeout(async () => {
			for (const w of list) await openKept(w);
		}, 1500);
	});

	// Fonts are local/system only — no Google Fonts network fetch. The terminal
	// font resolves from what the OS has installed (JetBrains Mono is bundled
	// via app.css @font-face), falling back to `monospace`.

	// Resolve the setting to an actual theme and apply it. Previously this only
	// toggled a class, so the palette lived in two places (CSS for the app,
	// hardcoded JS for the terminal) and could not be extended past two options.
	// "System" follows the OS live, with the light and dark theme of the pair
	// picked in Settings > Appearance (see #lib/themes/resolve).
	$effect(() => {
		applyTheme(
			themeFor({
				theme: settings.theme,
				themeId: settings.themeId,
				themeLight: settings.themeLight,
				themeDark: settings.themeDark
			})
		);
	});
</script>

<!-- Every right-click the app has no menu for: never the webview's own. -->
<AppContextMenu />

{#if isEditorWindow}
	{@render children()}
{:else if isFloat}
	<FloatShell>
		{@render children()}
	</FloatShell>
{:else}
	<!-- Run again from Settings, setupComplete stays true, so the prompts and
	     the lock screen below stay mounted under it. -->
	{#if !settings.setupComplete || setupWizard.rerun}
		<WelcomeScreen />
	{/if}

	<AppShell>
		{@render children()}
	</AppShell>

	<!-- Below the welcome screen's stacking order on purpose: a first-run user
	     is setting the app up, and must never meet this on top of that. -->
	{#if settings.setupComplete}
		<CommandPalette />
		{#if SHOW_COMMUNITY}
			<CommunityPrompt />
		{/if}
		<PasswordPrompt />
		<AskHost />
		<LockScreen />
	{/if}
{/if}
