<script lang="ts">
	import { untrack } from 'svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import KeyPicker from './KeyPicker.svelte';
	import OsIconSelect from './OsIconSelect.svelte';
	import { osChoices } from '#lib/data/distro-icons.js';
	import Input from '#lib/components/shared/Input.svelte';
	import { sessionCreate, sessionList, sessionUpdate, sessionKind, wslListDistros, type SessionConfig, type SessionKind, type AuthMethod, type JumpHostConfig, type Folder, type SshOptions, type CustomField, type TmuxSetting, type X11DesktopKind, X11_DESKTOPS, type TerminalOptions, TERMINAL_TYPES, TERMINAL_ENCODINGS, type ConnectMode, type SessionAddress, type WebOptions, type WebLogin, type WebCert, webSessionCertAdd, webSessionCertRemove, webSessionClearData } from '#lib/ipc/sessions.js';
	import { defaultTmuxName, folderTmux, validTmuxName } from '#lib/terminal/tmux.js';
	import { isIos, isMobile, isWindows } from '#lib/platform.js';

	/** From this window width the editor is wide, with two columns. */
	const WIDE_MIN_WINDOW = 1000;
	const WIDE_DIALOG = '940px';
	/** One height whatever the protocol, so switching protocols does not
	 *  resize the dialog and move its headers. */
	const DIALOG_HEIGHT = 'min(85vh, 880px)';
	import { APP_FILES_SHARE } from '#lib/ipc/rdp.js';
	import SshOptionsSection from './SshOptionsSection.svelte';
	import SessionHostKeys from './SessionHostKeys.svelte';
	import PasswordHistory from './PasswordHistory.svelte';
	import SessionAddresses, { type AddressRow } from './SessionAddresses.svelte';
	import Disclosure from '#lib/components/shared/Disclosure.svelte';
	import TerminalThemeList from '#lib/components/terminal/TerminalThemeList.svelte';
	import { findTerminalTheme, terminalPalette } from '#lib/state/terminal-themes.svelte.js';
	import { setSessionTerminalTheme, setSessionMonitoring, setSessionCursor } from '#lib/state/tabs.svelte.js';
	import { getSettings } from '#lib/state/settings.svelte.js';
	import SecretRefField from '#lib/components/pwmanager/SecretRefField.svelte';
	import type { EntrySummary, SecretRef } from '#lib/ipc/pwmanager.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { folderPath } from '#lib/sessions/folder-tree.js';
	import { open as openDialog } from '@tauri-apps/plugin-dialog';
	import { savedCredentialList, credentialsFor, credentialFolderList, groupByFolder, type SavedCredential, type CredentialFolder } from '#lib/ipc/savedCredentials.js';
	import { DEFAULT_TELNET_OPTIONS, type TelnetOptions } from '#lib/ipc/telnet.js';
	import { DEFAULT_SERIAL_OPTIONS, SERIAL_BAUD_RATES, serialListPorts, type SerialOptions, type SerialPortInfo } from '#lib/ipc/serial.js';
	import type { SpiceOptions } from '#lib/ipc/spice.js';
	import type { HistoryMode } from '#lib/ipc/history.js';
	import type { CursorStyle } from '#lib/terminal/cursor.js';
	import { jumpChainToSave } from '#lib/utils/route.js';

	interface Props {
		open: boolean;
		editSession?: SessionConfig;
		vaultId?: string | null; // Which vault to save to (null = private)
		folders?: Folder[];
		/** The folder a new session starts in (a folder's "New session here"). */
		folderId?: string | null;
		/** A session to copy: the form starts as it, and Save makes a new one. */
		cloneSession?: SessionConfig;
		onsave?: () => void;
	}

	let { open = $bindable(), editSession, vaultId = null, folders = [], folderId = null, cloneSession, onsave }: Props = $props();
	/** The session the form was filled from: the one edited, or the one copied. */
	let source = $derived(editSession ?? cloneSession);

	/**
	 * SSH or RDP. The two share a name, a host, a port and a username; RDP has
	 * a password and a logon domain and nothing else, because keys, agents,
	 * jump hosts, proxies and login shells are all SSH ideas. Switching moves
	 * the port between the two defaults only if it still is one.
	 *
	 * VNC has less still: a password, and a user name only for a Mac, which
	 * logs in with a macOS account instead of a VNC password. What it has
	 * instead is the way in: VNC is not encrypted, so it can go through an
	 * SSH session saved here.
	 */
	let kind = $state<SessionKind>('ssh');
	let domain = $state('');
	/** RDP only: the protocol cannot say what the machine is (xrdp claims to be Windows), so the user does. */
	let os = $state<'windows' | 'linux' | 'gnome'>('windows');
	// The OS found on the last connect is kept for good; Clear forgets it, so
	// the next connect looks again. (From the ReachQ fork by CPTProgrammer.)
	let osCleared = $state(false);
	// An icon picked by hand ('' = found on connect for SSH, none otherwise).
	// A picked one is kept: connecting never replaces it.
	let osChoice = $state('');
	const osOptions = osChoices();
	let autoOs = $derived(source && !source.os_manual && kind === sessionKind(source) && !osCleared ? (source.detected_os ?? '') : '');
	let osTitle = (id: string) => (id === 'unknown' ? t('session.os_none') : (osOptions.find((o) => o.id === id)?.title ?? id));
	// The OS icon list: Automatic (SSH only, showing what was detected), No
	// icon, a value no logo covers (kept as typed), then every logo.
	let osIconOptions = $derived([
		...(kind === 'ssh'
			? [
					{ value: '', label: autoOs ? t('session.os_auto_detected', { os: osTitle(autoOs) }) : t('session.os_auto'), osId: autoOs || null },
					{ value: 'unknown', label: t('session.os_none'), osId: null }
				]
			: [{ value: '', label: t('session.os_none'), osId: null }]),
		...(osChoice && osChoice !== 'unknown' && !osOptions.some((o) => o.id === osChoice) ? [{ value: osChoice, label: osChoice, osId: osChoice }] : []),
		...osOptions.map((o) => ({ value: o.id, label: o.title, osId: o.id }))
	]);
	let sharePath = $state('');
	/** iPhone and iPad: no folder can be named, the app's own folder in Files can be shared. */
	const onIos = isIos();
	/** What the RDP clipboard carries: '' for everything, 'text' for no files, 'off'. */
	let rdpClipboard = $state<'' | 'text' | 'off'>('');
	let rdpClient = $state<'ironrdp' | 'freerdp'>('ironrdp');
	let rdpForwardLockKeys = $state(true);
	/** VNC only: the saved SSH session to go through, or '' for direct. */
	let viaSessionId = $state('');
	let wslDistro = $state('');
	/** Telnet only: how it talks to the device. */
	let telnetOptions = $state<TelnetOptions>({ ...DEFAULT_TELNET_OPTIONS });
	/** SPICE only: the TLS port ('' for none), the CA and the certificate subject. */
	let spiceTlsPort = $state('');
	let spiceCa = $state('');
	let spiceSubject = $state('');
	/** Web only: see WebOptions. Certificates are changed by their own commands. */
	let webScheme = $state<'https' | 'http'>('https');
	let webPath = $state('');
	let webOpenIn = $state<'' | 'browser'>('');
	let webStorage = $state<'' | 'profile' | 'temporary'>('');
	let webProfile = $state('');
	let webLogin = $state<WebLogin>('auto');
	let webUserSel = $state('');
	let webPassSel = $state('');
	let webSubmitSel = $state('');
	let webSubmit = $state(true);
	let webCerts = $state<WebCert[]>([]);
	let webCertTyped = $state('');
	let webCertError = $state('');
	let webDataCleared = $state(false);

	/** A whole address pasted into the host field is split into its parts. */
	function splitWebAddress(): void {
		const text = host.trim();
		if (!/^https?:\/\//i.test(text)) return;
		try {
			const u = new URL(text);
			webScheme = u.protocol === 'http:' ? 'http' : 'https';
			host = u.hostname.replace(/^\[|\]$/g, '');
			portStr = u.port || (webScheme === 'http' ? '80' : '443');
			const rest = `${u.pathname}${u.search}`;
			webPath = rest === '/' ? '' : rest;
		} catch {
			/* left as typed; the backend says what is wrong */
		}
	}

	function webToSave(): WebOptions {
		const o: WebOptions = {};
		if (webScheme === 'http') o.scheme = 'http';
		if (webPath.trim() && webPath.trim() !== '/') o.path = webPath.trim();
		if (webOpenIn) o.open_in = webOpenIn;
		if (webStorage === 'temporary') o.storage = 'temporary';
		if (webStorage === 'profile' && webProfile.trim()) {
			o.storage = 'profile';
			o.profile = webProfile.trim();
		}
		if (webLogin !== 'auto') o.login = webLogin;
		if (webLogin === 'custom') {
			o.username_selector = webUserSel.trim() || null;
			o.password_selector = webPassSel.trim() || null;
			o.submit_selector = webSubmitSel.trim() || null;
		}
		if (!webSubmit) o.submit = false;
		return o;
	}

	async function addWebCert(): Promise<void> {
		if (!editSession) return;
		webCertError = '';
		try {
			webCerts = await webSessionCertAdd(editSession.id, webCertTyped, host.trim());
			webCertTyped = '';
		} catch (e) {
			webCertError = String(e);
		}
	}

	async function removeWebCert(fp?: string): Promise<void> {
		if (!editSession) return;
		try {
			webCerts = await webSessionCertRemove(editSession.id, fp);
		} catch (e) {
			webCertError = String(e);
		}
	}

	async function clearWebData(): Promise<void> {
		if (!editSession) return;
		webCertError = '';
		try {
			await webSessionClearData(editSession.id);
			webDataCleared = true;
		} catch (e) {
			webCertError = String(e);
		}
	}
	/** Serial only: line settings. The port name is kept in `host`. */
	let serialOptions = $state<SerialOptions>({ ...DEFAULT_SERIAL_OPTIONS });
	let serialPorts = $state<SerialPortInfo[]>([]);
	let serialPortsLoading = $state(false);
	/** The port is typed rather than picked (one the system does not list). */
	let serialPortTyped = $state(false);
	/** The speeds offered, with the session's own if it is not a usual one. */
	let baudRates = $derived(
		SERIAL_BAUD_RATES.includes(serialOptions.baud_rate) ? SERIAL_BAUD_RATES : [...SERIAL_BAUD_RATES, serialOptions.baud_rate].sort((a, b) => a - b)
	);
	let availableDistros = $state<string[]>([]);
	let sshSessions = $state<SessionConfig[]>([]);

	/**
	 * Which login the session uses: '' its folder's saved credential if one
	 * applies, else its own; OWN_LOGIN its own even then; or a saved
	 * credential's id. Only credentials in the session's vault are offered.
	 */
	const OWN_LOGIN = 'own';
	let credentialId = $state('');
	let allCredentials = $state<SavedCredential[]>([]);
	let sessionVault = $derived(source ? (source.vault_id ?? null) : vaultId);
	/** The folders this session can go in: its own vault's. One it is
	 *  already in from another vault (older versions allowed that) stays
	 *  listed so saving does not move it out unasked. */
	let vaultFolders = $derived(
		folders.filter((f) => (f.vault_id ?? null) === sessionVault || (!!source?.folder_id && f.id === source.folder_id))
	);
	let credentials = $derived(credentialsFor(allCredentials, sessionVault));
	let credentialFolders = $state<CredentialFolder[]>([]);
	let credentialGroups = $derived(groupByFolder(credentials, credentialFolders));

	$effect(() => {
		if (open) {
			savedCredentialList()
				.then((list) => (allCredentials = list))
				.catch(() => (allCredentials = []));
			credentialFolderList()
				.then((list) => (credentialFolders = list))
				.catch(() => (credentialFolders = []));
		}
	});

	/** The credential the nearest folder names, for the folder picked here. */
	let folderCredential = $derived.by(() => {
		const seen = new Set<string>();
		let id: string | null = folderIdStr || null;
		while (id && !seen.has(id)) {
			seen.add(id);
			const f = folders.find((x) => x.id === id);
			if (!f) return null;
			if (f.credential_id) {
				return { folder: f, credential: credentials.find((c) => c.id === f.credential_id) ?? null };
			}
			id = f.parent_id;
		}
		return null;
	});

	/** The saved credential the login comes from; null: the fields below. */
	let usedCredential = $derived(
		credentialId === OWN_LOGIN
			? null
			: credentialId
				? (credentials.find((c) => c.id === credentialId) ?? null)
				: (folderCredential?.credential ?? null)
	);

	/** The saved credential a jump host logs in with; null: its own fields. */
	function hopCredential(h: { credentialId: string }): SavedCredential | null {
		return h.credentialId ? (credentials.find((c) => c.id === h.credentialId) ?? null) : null;
	}

	function pickHopCredential(h: (typeof jumpHops)[number], next: string): void {
		const wasFilled = !!h.read?.credential && !h.read.credential.missing;
		h.credentialId = next;
		if (wasFilled && !hopCredential(h)) {
			h.authType = 'Password';
			h.password = '';
			h.passwordRef = null;
			h.keyPassphrase = '';
			h.keyPassphraseRef = null;
		}
	}

	function pickCredential(next: string): void {
		// The fields held the credential's login when the session was read
		// with one; going back to the session's own shows it blank, so what
		// is saved is what is seen.
		const wasFilled = !!editSession?.credential && !editSession.credential.missing;
		credentialId = next;
		if (wasFilled && !usedCredential) {
			authType = 'Password';
			password = '';
			passwordRef = null;
			keyPassphrase = '';
			keyPassphraseRef = null;
		}
	}

	$effect(() => {
		if (open && kind === 'wsl') {
			wslListDistros()
				.then((list) => {
					availableDistros = list;
					if (list.length > 0) {
						if (!wslDistro || !list.includes(wslDistro)) {
							wslDistro = list[0];
						}
					}
				})
				.catch(() => (availableDistros = []));
		}
	});

	async function loadSerialPorts(): Promise<void> {
		serialPortsLoading = true;
		try {
			serialPorts = await serialListPorts();
		} catch {
			serialPorts = [];
		} finally {
			serialPortsLoading = false;
		}
		// A new session takes the first port found; a saved one keeps its own,
		// typed in when it is not plugged in now.
		untrack(() => {
			if (!host.trim() && serialPorts.length > 0) host = serialPorts[0].name;
			serialPortTyped = !!host.trim() && !serialPorts.some((p) => p.name === host);
			if (!host.trim() && serialPorts.length === 0) serialPortTyped = true;
		});
	}

	$effect(() => {
		if (open && kind === 'serial') void loadSerialPorts();
	});

	$effect(() => {
		if (open && (kind === 'vnc' || kind === 'spice' || kind === 'ssh')) {
			sessionList()
				.then((all) => (sshSessions = all.filter((s) => sessionKind(s) === 'ssh')))
				.catch(() => (sshSessions = []));
		}
	});

	let name = $state('');
	let host = $state('');
	let portStr = $state('22');
	let username = $state('root');
	// Other addresses of the same server (SSH), see SessionAddresses.
	let hostLabel = $state('');
	let addressRows = $state<AddressRow[]>([]);
	let addressMode = $state<ConnectMode>('auto');
	let addressCheck = $state<'' | 'on' | 'off'>('');
	const addressCheckToSave = () => (addressCheck === '' ? null : addressCheck === 'on');

	/** The other addresses as they are saved: rows without a host dropped. */
	function addressesToSave(): SessionAddress[] | null {
		if (kind !== 'ssh') return null;
		const list = addressRows
			.filter((r) => r.host.trim())
			.map((r) => {
				const p = parseInt(String(r.port), 10);
				const a: SessionAddress = { host: r.host.trim() };
				if (r.label.trim()) a.label = r.label.trim();
				if (p > 0 && p < 65536) a.port = p;
				if (r.via) a.via_session_id = r.via;
				else if (r.direct) a.direct = true;
				return a;
			});
		return list.length ? list : null;
	}

	const DEFAULT_PORT: Record<SessionKind, string> = { ssh: '22', rdp: '3389', vnc: '5900', spice: '5900', wsl: '0', telnet: '23', serial: '0', winbox: '8291', web: '443' };

	/** The session kinds by what they are, in the editor's first row. A group
	 *  with nothing to offer here (Transfers, until it has protocols) is not shown. */
	const KIND_GROUPS: { id: 'terminal' | 'desktop' | 'transfer'; kinds: SessionKind[] }[] = [
		{ id: 'terminal', kinds: ['ssh', 'telnet', 'serial', 'wsl'] },
		{ id: 'desktop', kinds: ['rdp', 'vnc', 'spice', 'winbox', 'web'] },
		{ id: 'transfer', kinds: [] }
	];

	/** Whether this computer can open a kind. The one being edited always shows. */
	function kindAvailable(k: SessionKind): boolean {
		if (k === kind) return true;
		// WinBox and serial ports are programs and ports on this computer: none on a phone.
		if (k === 'winbox' || k === 'serial') return !isMobile();
		if (k === 'wsl') return isWindows();
		return true;
	}

	let visibleGroups = $derived(KIND_GROUPS.filter((g) => g.kinds.some(kindAvailable)));
	let group = $derived(KIND_GROUPS.find((g) => g.kinds.includes(kind))?.id ?? 'terminal');
	let groupKinds = $derived((KIND_GROUPS.find((g) => g.id === group)?.kinds ?? []).filter(kindAvailable));

	/** A group's tab: its first protocol, unless the session already is one of its. */
	function pickGroup(id: (typeof KIND_GROUPS)[number]['id']): void {
		if (id === group) return;
		const first = KIND_GROUPS.find((g) => g.id === id)?.kinds.find(kindAvailable);
		if (first) setKind(first);
	}

	function setKind(next: SessionKind): void {
		if (next === kind) return;
		if (portStr === DEFAULT_PORT[kind] || portStr.trim() === '') portStr = DEFAULT_PORT[next];
		if (next !== 'ssh' && username === 'root') username = '';
		if (next === 'ssh' && username === '') username = 'root';
		if (next === 'winbox' && username === '') username = 'admin';
		// A VNC user name means a Mac's own login: not one carried over.
		if (next === 'vnc' || next === 'spice' || next === 'serial') username = '';
		// A port name is not a host name, nor the other way round.
		if (next === 'serial' || kind === 'serial') host = '';
		kind = next;
	}
	/** 'Manager' is a password from a password manager: saved as a Password login with a reference. */
	let authType = $state<'Password' | 'Key' | 'Agent' | 'Manager'>('Password');
	let password = $state('');
	let keyPath = $state('');
	/** An imported key, used instead of keyPath. */
	let keyId = $state('');
	let keyPassphrase = $state('');
	/** Password-manager entries for the password and the passphrase; they win over the typed ones. */
	let passwordRef = $state<SecretRef | null>(null);
	let keyPassphraseRef = $state<SecretRef | null>(null);
	/** Key sessions: also offer the agent's keys if this key is refused. Off by default: see build_auth. */
	let tryAgentKeys = $state(false);
	let shell = $state('');
	let tagsStr = $state('');
	let folderIdStr = $state('');
	/** Documentation: free notes and named fields, like RDM's Documentation tab. */
	let notes = $state('');
	let customFields = $state<Array<CustomField & { shown: boolean }>>([]);
	let docsOpen = $state(false);
	let docsSummary = $derived.by(() => {
		const fields = customFields.filter((f) => f.name.trim() || f.value.trim()).length;
		const parts = [notes.trim() ? t('session.docs_has_notes') : '', fields ? t('session.docs_field_count', { count: String(fields) }) : ''].filter(Boolean);
		return parts.length ? parts.join(' · ') : t('session.docs_empty');
	});
	/** The terminal theme its tabs use; empty: the default from Settings. */
	let terminalTheme = $state('');
	/** The cursor in its tabs: '' the default from Settings. */
	let cursorChoice = $state<'' | CursorStyle>('');
	let blinkChoice = $state<'' | 'on' | 'off'>('');
	/** Server monitoring for its tabs: '' the default from Settings. */
	let monitoringChoice = $state<'' | 'on' | 'off'>('');
	/** What its tabs record into the session history; empty: the default from Settings. */
	let historyChoice = $state<'' | HistoryMode>('');
	/** SSH: start a desktop over X11 instead of a shell; '' for a terminal. */
	let x11Desktop = $state<'' | X11DesktopKind>('');
	let x11Command = $state('');
	/** SSH: terminal type, Backspace and encoding; empty is the default. */
	let termType = $state('');
	let backspaceChoice = $state<'' | 'ctrlh'>('');
	let encodingChoice = $state('');
	/** tmux on the server: '' as the folder says. See #lib/terminal/tmux. */
	let tmuxChoice = $state<'' | 'attach' | 'ask' | 'off'>('');
	let tmuxMouse = $state(false);
	let tmuxName = $state('');
	let tmuxFolderOn = $derived(folderTmux(folderIdStr || null, folders) === true);
	let tmuxOn = $derived(tmuxChoice === 'attach' || tmuxChoice === 'ask' || (tmuxChoice === '' && tmuxFolderOn));
	/** tmux runs on an SSH server, or inside a WSL distribution. */
	let tmuxKind = $derived((kind === 'ssh' && !x11Desktop) || kind === 'wsl');
	let colorsOpen = $state(false);
	let colorsSummary = $derived(
		findTerminalTheme(terminalTheme)?.name ?? (terminalTheme ? t('termtheme.missing') : t('termtheme.session_default'))
	);
	let jumpEnabled = $state(false);
	let jumpHops = $state<Array<{host: string; port: string; username: string; authType: 'Password' | 'Key' | 'Agent' | 'Manager'; password: string; keyPath: string; keyPassphrase: string; passwordRef: SecretRef | null; keyPassphraseRef: SecretRef | null; credentialId: string; read: JumpHostConfig | null}>>([]);
	let proxyEnabled = $state(false);
	let proxyType = $state<'socks5' | 'socks4' | 'http'>('socks5');
	let proxyHost = $state('127.0.0.1');
	let proxyPort = $state('9050');
	let proxyUsername = $state('');
	let proxyPassword = $state('');
	let proxyPasswordRef = $state<SecretRef | null>(null);
	/** ssh_config settings: imported files, lines set here, approvals. */
	let sshOptions = $state<SshOptions>({});
	let sshSection = $state<ReturnType<typeof SshOptionsSection> | undefined>();
	let saving = $state(false);
	let error = $state<string | undefined>();

	let isEditing = $derived(!!editSession);

	/** The window's width, for the two-column layout. */
	let windowWidth = $state(typeof window === 'undefined' ? 0 : window.innerWidth);
	/** Wide enough for two columns: the connection on the left, how the
	 *  session looks and is filed on the right. Never on a phone. */
	let wide = $derived(!isMobile() && windowWidth >= WIDE_MIN_WINDOW);
	/** Whether this kind opens in a terminal, so its right column holds
	 *  terminal settings too. */
	let opensTerminal = $derived(kind !== 'rdp' && kind !== 'vnc' && kind !== 'spice' && kind !== 'winbox' && kind !== 'web');
	let canSave = $derived(
		name.trim().length > 0 &&
		(kind === 'wsl' ? wslDistro.trim().length > 0 : host.trim().length > 0) &&
		(kind === 'vnc' || kind === 'spice' || kind === 'wsl' || kind === 'telnet' || kind === 'serial' || kind === 'web' || !!usedCredential || username.trim().length > 0) &&
		!saving
	);

	// Populate fields when editing, reset when creating.
	//
	// Only opening the editor, or a different session (or none, for a new
	// one), may fill the form again, so the filling runs untracked. Tracked,
	// it read state it had just written (docsOpen from notes and
	// customFields), ran itself again and again until Svelte gave up, and
	// each run put the fields back: every click in the editor was undone.
	$effect(() => {
		const session = editSession;
		const copy = cloneSession;
		if (!open) return;
		untrack(() => {
			fillForm(session ?? copy);
			if (!session && copy) startClone(copy);
		});
	});

	/**
	 * A copy starts as its original, under a new name. A login a saved
	 * credential filled in stays linked to that credential, but its secrets
	 * are not copied into the new session as its own.
	 */
	function startClone(copy: SessionConfig): void {
		name = t('session.clone_name', { name: copy.name });
		if (copy.credential && !copy.credential.missing) {
			authType = 'Password';
			password = '';
			passwordRef = null;
			keyPath = '';
			keyId = '';
			keyPassphrase = '';
			keyPassphraseRef = null;
		}
		for (const h of jumpHops) {
			if (h.read?.credential && !h.read.credential.missing) {
				h.authType = 'Password';
				h.password = '';
				h.passwordRef = null;
				h.keyPath = '';
				h.keyPassphrase = '';
				h.keyPassphraseRef = null;
			}
			h.read = null;
		}
	}

	function fillWeb(w: WebOptions | null): void {
		webScheme = w?.scheme === 'http' ? 'http' : 'https';
		webPath = w?.path ?? '';
		webOpenIn = w?.open_in ?? '';
		webStorage = w?.storage ?? '';
		webProfile = w?.profile ?? '';
		webLogin = w?.login ?? 'auto';
		webUserSel = w?.username_selector ?? '';
		webPassSel = w?.password_selector ?? '';
		webSubmitSel = w?.submit_selector ?? '';
		webSubmit = w?.submit !== false;
		webCerts = w?.certs ?? [];
		webCertTyped = '';
		webCertError = '';
		webDataCleared = false;
	}

	function fillForm(editSession: SessionConfig | undefined): void {
		osCleared = false;
		osChoice = editSession?.os_manual ? (editSession.detected_os ?? '') : '';
		if (editSession) {
			kind = sessionKind(editSession);
			domain = editSession.domain ?? '';
			os = editSession.detected_os === 'linux' || editSession.detected_os === 'gnome' ? editSession.detected_os : 'windows';
			sharePath = editSession.share_path ?? '';
			rdpClipboard = editSession.rdp_clipboard ?? '';
			rdpClient = editSession.rdp_use_freerdp ? 'freerdp' : 'ironrdp';
			rdpForwardLockKeys = editSession.rdp_forward_lock_keys ?? true;
			viaSessionId = editSession.via_session_id ?? '';
			wslDistro = editSession.wsl_distro ?? '';
			telnetOptions = { ...DEFAULT_TELNET_OPTIONS, ...(editSession.telnet ?? {}) };
			serialOptions = { ...DEFAULT_SERIAL_OPTIONS, ...(editSession.serial ?? {}) };
			spiceTlsPort = editSession.spice?.tls_port ? String(editSession.spice.tls_port) : '';
			spiceCa = editSession.spice?.ca ?? '';
			spiceSubject = editSession.spice?.host_subject ?? '';
			fillWeb(editSession.web ?? null);
			credentialId = editSession.credential_id ?? '';
			name = editSession.name;
			host = editSession.host;
			portStr = String(editSession.port);
			hostLabel = editSession.host_label ?? '';
			addressRows = (editSession.alt_addresses ?? []).map((a) => ({ label: a.label ?? '', host: a.host, port: a.port ? String(a.port) : '', direct: !!a.direct, via: a.via_session_id ?? '' }));
			addressMode = editSession.connect_mode ?? 'auto';
			addressCheck = editSession.address_check == null ? '' : editSession.address_check ? 'on' : 'off';
			username = editSession.username;
			authType = editSession.auth_method.type === 'Password' && editSession.auth_method.password_ref ? 'Manager' : editSession.auth_method.type;
			password = editSession.auth_method.password ?? '';
			keyPath = editSession.auth_method.path ?? '';
			keyId = editSession.auth_method.key_id ?? '';
			keyPassphrase = editSession.auth_method.passphrase ?? '';
			passwordRef = editSession.auth_method.type === 'Password' ? (editSession.auth_method.password_ref ?? null) : null;
			keyPassphraseRef = editSession.auth_method.type === 'Key' ? (editSession.auth_method.passphrase_ref ?? null) : null;
			tryAgentKeys = editSession.try_agent_keys === true;
			shell = editSession.shell ?? '';
			sshOptions = editSession.ssh_options ?? {};
			tagsStr = editSession.tags.join(', ');
			folderIdStr = editSession.folder_id ?? '';
			notes = editSession.notes ?? '';
			customFields = (editSession.custom_fields ?? []).map((f) => ({ ...f, hidden: !!f.hidden, shown: false }));
			docsOpen = !!notes.trim() || customFields.length > 0;
			terminalTheme = editSession.terminal_theme ?? '';
			monitoringChoice = editSession.monitoring == null ? '' : editSession.monitoring ? 'on' : 'off';
			cursorChoice = editSession.cursor_style ?? '';
			blinkChoice = editSession.cursor_blink == null ? '' : editSession.cursor_blink ? 'on' : 'off';
			historyChoice = editSession.history ?? '';
			tmuxChoice = editSession.tmux?.mode === 'attach' || editSession.tmux?.mode === 'ask' || editSession.tmux?.mode === 'off' ? editSession.tmux.mode : '';
			tmuxMouse = !!editSession.tmux?.mouse;
			x11Desktop = editSession.x11_desktop?.desktop ?? '';
			x11Command = editSession.x11_desktop?.command ?? '';
			termType = editSession.terminal?.term ?? '';
			backspaceChoice = editSession.terminal?.backspace === 'ctrlh' ? 'ctrlh' : '';
			encodingChoice = editSession.terminal?.encoding ?? '';
			tmuxName = editSession.tmux?.name ?? '';
			colorsOpen = false;
			if (editSession.jump_chain && editSession.jump_chain.length > 0) {
				jumpEnabled = true;
				jumpHops = editSession.jump_chain.map(j => ({
					host: j.host,
					port: String(j.port),
					username: j.username,
					authType: j.auth_method.type === 'Password' && j.auth_method.password_ref ? 'Manager' : j.auth_method.type,
					password: j.auth_method.type === 'Password' ? (j.auth_method.password ?? '') : '',
					keyPath: j.auth_method.type === 'Key' ? (j.auth_method.path ?? '') : '',
					keyPassphrase: j.auth_method.type === 'Key' ? (j.auth_method.passphrase ?? '') : '',
					passwordRef: j.auth_method.type === 'Password' ? (j.auth_method.password_ref ?? null) : null,
					keyPassphraseRef: j.auth_method.type === 'Key' ? (j.auth_method.passphrase_ref ?? null) : null,
					credentialId: j.credential_id ?? '',
					read: j,
				}));
			} else {
				jumpEnabled = false;
				jumpHops = [];
			}
		if (editSession.proxy) {
			proxyEnabled = true;
			proxyType = (editSession.proxy.proxy_type as 'socks5' | 'socks4' | 'http') ?? 'socks5';
			proxyHost = editSession.proxy.host ?? '127.0.0.1';
			proxyPort = String(editSession.proxy.port ?? 9050);
			proxyUsername = editSession.proxy.username ?? '';
			proxyPassword = editSession.proxy.password ?? '';
			proxyPasswordRef = editSession.proxy.password_ref ?? null;
		} else {
			proxyEnabled = false;
			proxyPasswordRef = null;
		}
		} else {
			kind = 'ssh';
			domain = '';
			os = 'windows';
			sharePath = '';
			rdpClipboard = '';
			rdpClient = 'ironrdp';
			rdpForwardLockKeys = true;
			viaSessionId = '';
			telnetOptions = { ...DEFAULT_TELNET_OPTIONS };
			serialOptions = { ...DEFAULT_SERIAL_OPTIONS };
			spiceTlsPort = '';
			spiceCa = '';
			spiceSubject = '';
			credentialId = '';
			name = '';
			host = '';
			portStr = '22';
			hostLabel = '';
			addressRows = [];
			addressMode = 'auto';
			addressCheck = '';
			username = 'root';
			authType = 'Password';
			password = '';
			keyPath = '';
			keyId = '';
			keyPassphrase = '';
			passwordRef = null;
			keyPassphraseRef = null;
			tryAgentKeys = false;
			shell = '';
			sshOptions = {};
			tagsStr = '';
			folderIdStr = folderId ?? '';
			notes = '';
			customFields = [];
			docsOpen = false;
			terminalTheme = '';
			monitoringChoice = '';
			cursorChoice = '';
			blinkChoice = '';
			historyChoice = '';
			tmuxChoice = '';
			tmuxMouse = false;
			x11Desktop = '';
			x11Command = '';
			termType = '';
			backspaceChoice = '';
			encodingChoice = '';
			tmuxName = '';
			colorsOpen = false;
			jumpEnabled = false;
			jumpHops = [];
			proxyEnabled = false;
			proxyType = 'socks5';
			proxyHost = '127.0.0.1';
			proxyPort = '9050';
			proxyUsername = '';
			proxyPassword = '';
			wslDistro = '';
			proxyPasswordRef = null;
		}
		error = undefined;
	}

	async function handleSave(): Promise<void> {
		if (!canSave) return;
		// "Password manager" without a chosen entry would save a login with
		// no password at all.
		const ssh = kind === 'ssh';
		if (ssh && ((authType === 'Manager' && !passwordRef && !usedCredential) || (jumpEnabled && jumpHops.some((h) => h.authType === 'Manager' && !h.passwordRef && !hopCredential(h))))) {
			error = t('session.manager_pick_entry');
			return;
		}
		saving = true;
		error = undefined;

		// A whole address pasted into a web session's host field is split first.
		if (kind === 'web') splitWebAddress();
		const port = kind === 'wsl' || kind === 'serial' ? 0 : (parseInt(portStr, 10) || parseInt(DEFAULT_PORT[kind], 10) || 22);
		const rdp = kind === 'rdp';
		const vnc = kind === 'vnc';
		const spice = kind === 'spice';
		const wsl = kind === 'wsl';
		const telnet = kind === 'telnet';
		const serial = kind === 'serial';
		const winbox = kind === 'winbox';
		const web = kind === 'web';
		/** Jump hosts, proxies, login shells and ssh_config are SSH's alone. */
		const desktop = rdp || vnc || spice || wsl || telnet || serial || winbox || web;
		const via = vnc || spice ? viaSessionId || null : null;
		const tlsPort = parseInt(spiceTlsPort, 10);
		const spiceToSave: SpiceOptions | null = spice
			? {
					tls_port: tlsPort > 0 && tlsPort < 65536 ? tlsPort : null,
					ca: spiceCa.trim() ? spiceCa.trim() + '\n' : null,
					host_subject: spiceSubject.trim() || null,
				}
			: null;
		/** WSL and VNC have no username to share; their login stays their own. */
		const credential = rdp || telnet || winbox || web || kind === 'ssh' ? credentialId || null : null;
		// RDP and VNC are password logons; the toggle below is SSH-only and
		// its value is ignored here rather than trusted. WSL signs in by
		// itself: nothing to keep, so a password left in the form is not saved.
		const authMethod: AuthMethod = wsl || serial
			? { type: 'Password' }
			: desktop
			? passwordAuth(password, passwordRef)
			: authType === 'Password'
			? passwordAuth(password, null)
			: authType === 'Manager'
			? passwordAuth('', passwordRef)
			: authType === 'Key'
				? {
						type: 'Key',
						// One source or the other, never both: the picker clears
						// whichever the user did not choose.
						path: keyId ? undefined : keyPath.trim(),
						key_id: keyId || undefined,
						// A passphrase from a password manager wins; the typed one is
						// not kept beside it. An imported key carries its own.
						passphrase: keyPassphraseRef && !keyId ? undefined : keyPassphrase || undefined,
						passphrase_ref: keyId ? undefined : (keyPassphraseRef ?? undefined)
					}
				: { type: 'Agent' };
		const tags = tagsStr.split(',').map(t => t.trim()).filter(Boolean);
		const notesToSave = notes.trim() ? notes : null;
		// Remote desktops paint their own screen, and WinBox is its own
		// program: no terminal colours.
		const noTerminal = kind === 'rdp' || kind === 'vnc' || kind === 'spice' || winbox;
		const themeToSave = noTerminal ? null : terminalTheme || null;
		const cursorToSave = noTerminal ? null : cursorChoice || null;
		const blinkToSave = noTerminal || !blinkChoice ? null : blinkChoice === 'on';
		// Only SSH and WSL sessions have a server to monitor.
		const monitoringToSave = (kind === 'ssh' || kind === 'wsl') && monitoringChoice ? monitoringChoice === 'on' : null;
		const historyToSave = noTerminal ? null : historyChoice || null;
		// tmux runs on an SSH server only. A name is kept only beside a choice
		// or a folder that turns it on.
		const x11ToSave = kind === 'ssh' && x11Desktop ? { desktop: x11Desktop, command: x11Desktop === 'custom' ? x11Command.trim() : null } : null;
		const terminalToSave: TerminalOptions | null = kind === 'ssh' && (termType.trim() || backspaceChoice || encodingChoice)
			? { term: termType.trim() || null, backspace: backspaceChoice || null, encoding: encodingChoice || null }
			: null;
		if (terminalToSave?.term && !/^[\x21-\x7e]{1,64}$/.test(terminalToSave.term)) {
			error = t('session.term_type_invalid');
			return;
		}
		if (x11ToSave?.desktop === 'custom' && !x11ToSave.command) {
			error = t('x11desktop.command_needed');
			return;
		}
		const tmuxNameToSave = tmuxName.trim();
		if (tmuxKind && tmuxOn && tmuxNameToSave && !validTmuxName(tmuxNameToSave)) {
			error = t('tmux.name_invalid');
			saving = false;
			return;
		}
		const tmuxToSave: TmuxSetting | null =
			tmuxKind && (tmuxChoice || tmuxNameToSave || tmuxMouse)
				? {
						// WSL has no picker: asking there means attach.
						mode: tmuxChoice === 'ask' && kind !== 'ssh' ? 'attach' : tmuxChoice || 'inherit',
						name: tmuxNameToSave || null,
						...(tmuxMouse ? { mouse: true } : {})
					}
				: null;
		const fieldsToSave: CustomField[] = customFields
			.filter((f) => f.name.trim() || f.value.trim())
			.map((f) => (f.hidden ? { name: f.name.trim(), value: f.value, hidden: true } : { name: f.name.trim(), value: f.value }));

		const jumpChain: JumpHostConfig[] | null = jumpChainToSave(desktop, jumpEnabled, jumpHops.map(h => {
				const hopAuth: AuthMethod = h.authType === 'Password'
					? passwordAuth(h.password, null)
					: h.authType === 'Manager'
					? passwordAuth('', h.passwordRef)
					: h.authType === 'Key'
						? {
								type: 'Key',
								path: h.keyPath.trim(),
								passphrase: h.keyPassphraseRef ? undefined : h.keyPassphrase || undefined,
								passphrase_ref: h.keyPassphraseRef ?? undefined
							}
						: { type: 'Agent' };
				// As for the session's own login: a login a credential filled in
				// goes back as it came, so the backend keeps the hop's own.
				const filled = !!hopCredential(h) && !!h.read?.credential && !h.read.credential.missing;
				return {
					host: h.host.trim(),
					port: parseInt(h.port, 10) || 22,
					username: filled && h.read ? h.read.username : h.username.trim(),
					auth_method: filled && h.read ? h.read.auth_method : hopAuth,
					credential_id: h.credentialId || null,
				};
			}));

		// The weakenings in force are stored with the session, those written
		// here included, so the session list can flag a weaker session.
		const mine = [...new Set([...(sshOptions.my_accepted_weakenings ?? []), ...(sshSection?.acceptedNow() ?? [])])];
		const finalOptions: SshOptions = { ...sshOptions, my_accepted_weakenings: mine };
		const hasOptions = !desktop && (!!finalOptions.imported || (finalOptions.lines ?? []).length > 0);
		const sshOptionsToSave = hasOptions ? finalOptions : null;

		const proxyConfig = !desktop && proxyEnabled ? {
			proxy_type: proxyType,
			host: proxyHost.trim(),
			port: parseInt(proxyPort, 10) || 9050,
			username: proxyUsername.trim() || null,
			password: proxyPasswordRef ? null : (proxyPassword || null),
			password_ref: proxyPasswordRef ?? undefined,
		} : null;

		const altToSave = addressesToSave();
		try {
			if (isEditing && editSession) {
				// A login a credential filled in goes back as it came, so the
				// backend knows it is not the session's own and keeps that.
				const filled = !!usedCredential && !!editSession.credential && !editSession.credential.missing;
				await sessionUpdate({
					...editSession,
					name: name.trim(),
					host: kind === 'wsl' ? (host.trim() || 'wsl') : host.trim(),
					port,
					username: filled ? editSession.username : username.trim(),
					auth_method: filled ? editSession.auth_method : authMethod,
					folder_id: folderIdStr || null,
					tags,
					jump_chain: jumpChain,
					proxy: proxyConfig,
					shell: desktop ? null : (shell.trim() || null),
					kind,
					domain: filled ? (editSession.domain ?? null) : rdp ? (domain.trim() || null) : null,
					// A session that changed protocol changed machine type too.
					// Picked by hand: kept. Automatic: what the last connect
					// found, unless cleared, switched from a picked one, or
					// the protocol changed (so did the machine type).
					detected_os: rdp ? os : osChoice ? osChoice : autoOs || (winbox ? 'mikrotik' : null),
					os_manual: !rdp && !!osChoice,
					share_path: rdp ? (sharePath.trim() || null) : null,
					rdp_clipboard: rdp && rdpClipboard ? rdpClipboard : null,
					rdp_use_freerdp: rdp && rdpClient === 'freerdp',
					rdp_forward_lock_keys: rdp ? rdpForwardLockKeys : null,
					via_session_id: via,
					try_agent_keys: !desktop && authType === 'Key' && tryAgentKeys ? true : null,
					wsl_distro: kind === 'wsl' ? wslDistro.trim() : null,
					telnet: telnet ? { ...telnetOptions } : null,
					serial: serial ? { ...serialOptions } : null,
					web: web ? { ...webToSave(), certs: webCerts } : null,
					spice: spiceToSave,
					ssh_options: sshOptionsToSave,
					credential_id: credential,
					credential: null,
					notes: notesToSave,
					custom_fields: fieldsToSave.length ? fieldsToSave : null,
					terminal_theme: themeToSave,
					cursor_style: cursorToSave,
					cursor_blink: blinkToSave,
					monitoring: monitoringToSave,
					history: historyToSave,
					tmux: tmuxToSave,
					x11_desktop: x11ToSave,
					terminal: terminalToSave,
					host_label: altToSave ? (hostLabel.trim() || null) : null,
					alt_addresses: altToSave,
					connect_mode: altToSave ? addressMode : null,
					address_check: altToSave ? addressCheckToSave() : null,
				});
				// Its open tabs take the new colours (and monitoring choice) at once.
				setSessionTerminalTheme(editSession.id, themeToSave);
				setSessionMonitoring(editSession.id, monitoringToSave);
				setSessionCursor(editSession.id, cursorToSave, blinkToSave);
			} else {
				await sessionCreate({
					name: name.trim(),
					host: kind === 'wsl' ? (host.trim() || 'wsl') : host.trim(),
					port,
					username: username.trim(),
					authMethod: authMethod,
					folderId: folderIdStr || null,
					tags,
					vaultId: sessionVault,
					jumpChain,
					proxy: proxyConfig,
					shell: desktop ? null : (shell.trim() || null),
					kind,
					domain: rdp ? domain : null,
					detectedOs: rdp ? os : (osChoice || autoOs || (winbox ? 'mikrotik' : null)),
					osManual: !rdp && !!osChoice,
					sharePath: rdp ? sharePath : null,
					rdpClipboard: rdp && rdpClipboard ? rdpClipboard : null,
					rdpUseFreerdp: rdp && rdpClient === 'freerdp',
					rdpForwardLockKeys: rdp ? rdpForwardLockKeys : null,
					viaSessionId: via,
					tryAgentKeys: !desktop && authType === 'Key' && tryAgentKeys,
					sshOptions: sshOptionsToSave,
					wslDistro: kind === 'wsl' ? wslDistro.trim() : null,
					telnet: telnet ? { ...telnetOptions } : null,
					serial: serial ? { ...serialOptions } : null,
					web: web ? webToSave() : null,
					spice: spiceToSave,
					credentialId: credential,
					notes: notesToSave,
					customFields: fieldsToSave,
					terminalTheme: themeToSave,
					cursorStyle: cursorToSave,
					cursorBlink: blinkToSave,
					monitoring: monitoringToSave,
					history: historyToSave,
					tmux: tmuxToSave,
					x11Desktop: x11ToSave,
					terminal: terminalToSave,
					hostLabel: altToSave ? hostLabel : null,
					altAddresses: altToSave,
					connectMode: altToSave ? addressMode : null,
					addressCheck: altToSave ? addressCheckToSave() : null
				});
			}
			onsave?.();
			open = false;
		} catch (err) {
			error = String(err);
		} finally {
			saving = false;
		}
	}

	/** A password logon: the entry from a password manager, or the typed password. */
	function passwordAuth(typed: string, ref: SecretRef | null): AuthMethod {
		return ref ? { type: 'Password', password_ref: ref } : { type: 'Password', password: typed || undefined };
	}

	/**
	 * A user name from the picked entry, for a field still empty (or still the
	 * default a new session starts with). One the user typed is never replaced.
	 */
	function nameFromEntry(current: string, entry: EntrySummary): string {
		const picked = entry.username?.trim();
		if (!picked) return current;
		const untouched = current.trim() === '' || (!isEditing && current === 'root');
		return untouched ? picked : current;
	}

	async function browseKey(setter: (path: string) => void): Promise<void> {
		try {
			const selected = await openDialog({
				multiple: false,
				directory: false,
				title: t('session.select_key_file'),
				filters: [
					{ name: t('session.ssh_private_key_filter'), extensions: ['pem', 'key', 'ppk', 'rsa', 'ed25519', 'ecdsa', 'dsa'] },
					{ name: 'All Files', extensions: ['*'] },
				],
			});
			if (typeof selected === 'string') setter(selected);
		} catch {
			// User cancelled the dialog
		}
	}

	async function browseFolder(): Promise<void> {
		try {
			const selected = await openDialog({ multiple: false, directory: true, title: t('session.rdp_share_folder') });
			if (typeof selected === 'string') sharePath = selected;
		} catch {
			// User cancelled the dialog
		}
	}

	function addHop(): void {
		jumpHops = [...jumpHops, { host: '', port: '22', username: 'root', authType: 'Password', password: '', keyPath: '', keyPassphrase: '', passwordRef: null, keyPassphraseRef: null, credentialId: '', read: null }];
	}

	function addField(): void {
		customFields = [...customFields, { name: '', value: '', hidden: false, shown: true }];
	}

	function removeField(index: number): void {
		customFields = customFields.filter((_, i) => i !== index);
	}

	function removeHop(index: number): void {
		jumpHops = jumpHops.filter((_, i) => i !== index);
	}

	function handleClose(): void {
		if (!saving) {
			open = false;
		}
	}
</script>

<svelte:window bind:innerWidth={windowWidth} />

<Modal {open} onclose={handleClose} title={isEditing ? t('session.edit_session') : cloneSession ? t('session.clone_session') : t('session.new')} maxWidth={wide ? WIDE_DIALOG : undefined} height={isMobile() ? undefined : DIALOG_HEIGHT} fullOnPhone>
	<form class="form session-form" onsubmit={(e) => { e.preventDefault(); handleSave(); }}>
		<!-- Same control as the auth method below, but a grid that wraps into
		     rows where the window is narrow, so no protocol is cut to "S…". -->
		<div class="auth-section">
			<span class="auth-label">{t('session.protocol')}</span>
			<!-- First the kind of session, then the protocols of that kind. -->
			<div class="category-tabs" role="tablist" aria-label={t('session.protocol')}>
				{#each visibleGroups as g (g.id)}
					<button type="button" role="tab" class="category-tab" class:active={g.id === group} aria-selected={g.id === group} disabled={saving} onclick={() => pickGroup(g.id)}>
						{t(`session.category_${g.id}`)}
					</button>
				{/each}
			</div>
			<div class="auth-toggle protocol-toggle">
				{#each groupKinds as k (k)}
					<button type="button" class="auth-btn" class:active={kind === k} disabled={saving} onclick={() => setKind(k)} title={k === 'rdp' ? t('session.protocol_rdp') : undefined}>
						{k === 'rdp' ? t('session.protocol_rdp_short') : t(`session.protocol_${k}`)}
					</button>
				{/each}
			</div>
		</div>

		<!-- On a wide window two columns; otherwise the columns vanish
		     (display: contents) and it is one list as before. -->
		<div class="columns" class:wide>
		<div class="column">
		{#if wide}<span class="column-title">{t('session.column_connection')}</span>{/if}
		<Input label={t('session.name')} bind:value={name} placeholder={kind === 'rdp' ? 'Office PC' : kind === 'telnet' ? 'Core switch' : kind === 'serial' ? 'Switch console' : kind === 'winbox' ? 'Router' : kind === 'web' ? 'iDRAC rack 3' : 'My Server'} disabled={saving} />

		{#if kind === 'wsl'}
			<div class="shell-field">
				<label class="via-label" for="wsl-distro">{t('session.wsl_distro')}</label>
				{#if availableDistros.length > 0}
					<select id="wsl-distro" class="via-select" bind:value={wslDistro} disabled={saving}>
						{#each availableDistros as d}
							<option value={d}>{d}</option>
						{/each}
					</select>
				{:else}
					<Input
						label={t('session.wsl_distro')}
						bind:value={wslDistro}
						placeholder={t('session.wsl_distro_placeholder')}
						disabled={saving}
					/>
				{/if}
			</div>
		{:else if kind === 'serial'}
			<div class="shell-field">
				<label class="via-label" for="serial-port">{t('serial.port')}</label>
				<div class="key-path-row">
					<div class="key-path-input">
						{#if serialPortTyped}
							<Input bind:value={host} placeholder={isWindows() ? 'COM3' : '/dev/ttyUSB0'} disabled={saving} />
						{:else}
							<select id="serial-port" class="via-select" bind:value={host} disabled={saving}>
								{#each serialPorts as p (p.name)}
									<option value={p.name}>{p.description ? `${p.name} · ${p.description}` : p.name}</option>
								{/each}
							</select>
						{/if}
					</div>
					<Button variant="secondary" size="sm" onclick={loadSerialPorts} disabled={saving || serialPortsLoading}>
						{t('serial.refresh')}
					</Button>
				</div>
				<p class="shell-hint">
					{#if serialPorts.length === 0 && !serialPortsLoading}
						{t('serial.no_ports')}
					{/if}
					{#if serialPorts.length > 0}
						<button type="button" class="link-btn" onclick={() => (serialPortTyped = !serialPortTyped)} disabled={saving}>
							{serialPortTyped ? t('serial.pick_port') : t('serial.type_port')}
						</button>
					{/if}
				</p>
			</div>
		{:else}
			<div class="row">
				<div class="field-host">
					<Input label={kind === 'web' ? t('web.address') : t('session.host')} bind:value={host} placeholder={kind === 'web' ? 'https://192.168.1.10:8006' : '192.168.1.1'} disabled={saving} />
				</div>
				<div class="field-port">
					<Input label={t('session.port')} bind:value={portStr} type="number" placeholder={DEFAULT_PORT[kind]} disabled={saving} />
				</div>
			</div>
			{#if kind === 'ssh'}
				<SessionAddresses bind:mainLabel={hostLabel} bind:rows={addressRows} bind:mode={addressMode} bind:check={addressCheck} checkByDefault={getSettings().checkAddresses} jumpSessions={sshSessions.filter((s) => s.id !== editSession?.id).map((s) => ({ id: s.id, name: s.name }))} routed={jumpEnabled || proxyEnabled} port={portStr} disabled={saving} />
			{/if}
		{/if}

		<!-- The passwords this session used before (editing only: a new or
		     cloned session has none). Use puts one back as the typed password. -->
		{#snippet pastPasswords()}
			{#if editSession && !passwordRef}
				<PasswordHistory owner={{ session: editSession.id }} disabled={saving} onrestore={(p) => { password = p; passwordRef = null; if (authType === 'Manager') authType = 'Password'; }} />
			{/if}
		{/snippet}

		{#if kind === 'ssh' || kind === 'rdp' || kind === 'telnet' || kind === 'winbox' || kind === 'web'}
			<div class="shell-field">
				<label class="via-label" for="session-login">{t('credential.login')}</label>
				<select id="session-login" class="via-select" value={credentialId} onchange={(e) => pickCredential(e.currentTarget.value)} disabled={saving}>
					{#if folderCredential}
						<option value="">{folderCredential.credential ? t('credential.from_folder', { name: folderCredential.credential.name, folder: folderCredential.folder.name }) : t('credential.from_folder_missing', { folder: folderCredential.folder.name })}</option>
						<option value={OWN_LOGIN}>{t('credential.own_login')}</option>
					{:else}
						<option value="">{t('credential.own_login')}</option>
					{/if}
					{#each credentialGroups as g (g.label ?? '')}
						{#if g.label === null}
							{#each g.credentials as c (c.id)}
								<option value={c.id}>{c.name} ({c.username})</option>
							{/each}
						{:else}
							<optgroup label={g.label}>
								{#each g.credentials as c (c.id)}
									<option value={c.id}>{c.name} ({c.username})</option>
								{/each}
							</optgroup>
						{/if}
					{/each}
					{#if credentialId && credentialId !== OWN_LOGIN && !credentials.some((c) => c.id === credentialId)}
						<option value={credentialId}>{t('credential.missing')}</option>
					{/if}
				</select>
				{#if usedCredential}
					<p class="shell-hint">{t('credential.login_hint', { username: usedCredential.username })}</p>
				{:else if credentialId && credentialId !== OWN_LOGIN}
					<p class="shell-hint">{t('credential.missing_hint')}</p>
				{/if}
			</div>
		{/if}

		{#if kind !== 'vnc' && kind !== 'spice' && kind !== 'wsl' && kind !== 'serial' && !usedCredential}
			<Input label={kind === 'telnet' ? t('session.telnet_username') : t('session.username')} bind:value={username} placeholder={kind === 'rdp' ? 'Administrator' : kind === 'telnet' || kind === 'winbox' || kind === 'web' ? 'admin' : 'root'} disabled={saving} />
		{/if}

		{#if kind === 'rdp'}
			{#if !isMobile()}
				<div class="shell-field">
					<label class="via-label" for="rdp-client">{t('session.rdp_client')}</label>
					<select id="rdp-client" class="via-select" bind:value={rdpClient} disabled={saving}>
						<option value="ironrdp">{t('session.rdp_client_ironrdp')}</option>
						<option value="freerdp">{t('session.rdp_client_freerdp')}</option>
					</select>
					{#if rdpClient === 'freerdp'}<p class="shell-hint">{t('session.rdp_client_freerdp_hint')}</p>{/if}
				</div>
			{/if}
			{#if !usedCredential}
			<div class="shell-field">
				<SecretRefField approvable label={t('session.password_optional')} bind:value={password} bind:secretRef={passwordRef} placeholder={t('session.stored_encrypted')} disabled={saving} onpick={(e) => (username = nameFromEntry(username, e))} />
				{@render pastPasswords()}
				<p class="shell-hint">{t('session.rdp_password_hint')}</p>
			</div>
			<Input label={t('session.rdp_domain')} bind:value={domain} placeholder="CORP" disabled={saving} />
			{/if}
			<div class="auth-section">
				<span class="auth-label">{t('session.os')}</span>
				<div class="auth-toggle">
					<button type="button" class="auth-btn" class:active={os === 'windows'} disabled={saving} onclick={() => (os = 'windows')}>{t('session.os_windows')}</button>
					<button type="button" class="auth-btn" class:active={os === 'linux'} disabled={saving} onclick={() => (os = 'linux')}>{t('session.os_linux')}</button>
					<button type="button" class="auth-btn" class:active={os === 'gnome'} disabled={saving} onclick={() => (os = 'gnome')}>{t('session.os_gnome')}</button>
				</div>
			</div>
			{#if onIos}
			<div class="shell-field">
				<label class="jump-toggle">
					<input type="checkbox" checked={sharePath === APP_FILES_SHARE} disabled={saving} onchange={(e) => (sharePath = e.currentTarget.checked ? APP_FILES_SHARE : '')} />
					<span class="jump-toggle-text">{t('session.rdp_share_app_files')}</span>
				</label>
				<p class="shell-hint">{t('session.rdp_share_app_files_hint')}</p>
			</div>
			{:else}
			<div class="shell-field">
				<div class="key-path-row">
					<div class="key-path-input">
						<Input label={t('session.rdp_share_folder')} bind:value={sharePath} placeholder="C:\Users\me\Documents" disabled={saving} />
					</div>
					<Button variant="secondary" size="sm" onclick={browseFolder} disabled={saving}>
						{t('session.rdp_share_browse')}
					</Button>
				</div>
				<p class="shell-hint">{t('session.rdp_share_hint')}</p>
			</div>
			{/if}
			<div class="shell-field">
				<label class="via-label" for="rdp-clipboard">{t('session.rdp_clipboard')}</label>
				<select id="rdp-clipboard" class="via-select" bind:value={rdpClipboard} disabled={saving}>
					<option value="">{t('session.rdp_clipboard_all')}</option>
					<option value="text">{t('session.rdp_clipboard_text')}</option>
					<option value="off">{t('session.rdp_clipboard_off')}</option>
				</select>
				<p class="shell-hint">{t('session.rdp_clipboard_hint')}</p>
			</div>
			{#if rdpClient === 'ironrdp' || isMobile()}
				<div class="shell-field">
					<label class="jump-toggle">
						<input type="checkbox" bind:checked={rdpForwardLockKeys} disabled={saving} aria-describedby="rdp-forward-lock-keys-hint" />
						<span class="jump-toggle-text">{t('session.rdp_forward_lock_keys')}</span>
					</label>
					<p id="rdp-forward-lock-keys-hint" class="shell-hint">{t('session.rdp_forward_lock_keys_hint')}</p>
				</div>
			{/if}
		{:else if kind === 'vnc'}
			<div class="shell-field">
				<Input label={t('session.vnc_mac_username')} bind:value={username} placeholder={t('session.vnc_mac_username_placeholder')} disabled={saving} />
				<p class="shell-hint">{t('session.vnc_mac_username_hint')}</p>
			</div>
			<div class="shell-field">
				<SecretRefField approvable label={t('session.password_optional')} bind:value={password} bind:secretRef={passwordRef} placeholder={t('session.stored_encrypted')} disabled={saving} />
				{@render pastPasswords()}
				<p class="shell-hint">{username.trim() ? t('session.vnc_mac_password_hint') : t('session.vnc_password_hint')}</p>
			</div>
			<div class="shell-field">
				<label class="via-label" for="vnc-via">{t('session.vnc_via')}</label>
				<select id="vnc-via" class="via-select" bind:value={viaSessionId} disabled={saving}>
					<option value="">{t('session.vnc_via_none')}</option>
					{#each sshSessions as s (s.id)}
						<option value={s.id}>{s.name} ({s.username}@{s.host})</option>
					{/each}
				</select>
				<p class="shell-hint">{t('session.vnc_via_hint')}</p>
			</div>
		{:else if kind === 'spice'}
			<div class="shell-field">
				<SecretRefField approvable label={t('session.password_optional')} bind:value={password} bind:secretRef={passwordRef} placeholder={t('session.stored_encrypted')} disabled={saving} />
				{@render pastPasswords()}
				<p class="shell-hint">{t('spice.password_hint')}</p>
			</div>
			<div class="shell-field">
				<Input label={t('spice.tls_port')} bind:value={spiceTlsPort} type="number" placeholder={t('spice.tls_port_placeholder')} disabled={saving} />
				<p class="shell-hint">{t('spice.tls_port_hint')}</p>
			</div>
			{#if spiceTlsPort.trim()}
				<div class="shell-field">
					<label class="via-label" for="spice-ca">{t('spice.ca')}</label>
					<textarea id="spice-ca" class="spice-ca" rows="4" bind:value={spiceCa} placeholder="-----BEGIN CERTIFICATE-----" spellcheck="false" disabled={saving}></textarea>
					<p class="shell-hint">{t('spice.ca_hint')}</p>
				</div>
				<div class="shell-field">
					<Input label={t('spice.host_subject')} bind:value={spiceSubject} placeholder="O=Example,CN=host.example.com" disabled={saving} />
					<p class="shell-hint">{t('spice.host_subject_hint')}</p>
				</div>
			{/if}
			<div class="shell-field">
				<label class="via-label" for="spice-via">{t('session.vnc_via')}</label>
				<select id="spice-via" class="via-select" bind:value={viaSessionId} disabled={saving}>
					<option value="">{t('session.vnc_via_none')}</option>
					{#each sshSessions as s (s.id)}
						<option value={s.id}>{s.name} ({s.username}@{s.host})</option>
					{/each}
				</select>
				<p class="shell-hint">{t('spice.via_hint')}</p>
			</div>
		{:else if kind === 'telnet'}
			<!-- Telnet has no login of its own: what is saved here is typed in
			     when the device prompts for it. No keys, jump hosts or proxies. -->
			{#if !usedCredential}
				<div class="shell-field">
					<SecretRefField approvable label={t('session.password_optional')} bind:value={password} bind:secretRef={passwordRef} placeholder={t('session.stored_encrypted')} disabled={saving} onpick={(e) => (username = nameFromEntry(username, e))} />
					{@render pastPasswords()}
					<p class="shell-hint">{t('session.telnet_login_hint')}</p>
				</div>
			{/if}
			<div class="shell-field">
				<label class="via-label" for="telnet-mode">{t('session.telnet_mode')}</label>
				<select id="telnet-mode" class="via-select" bind:value={telnetOptions.mode} disabled={saving}>
					<option value="active">{t('session.telnet_mode_active')}</option>
					<option value="passive">{t('session.telnet_mode_passive')}</option>
					<option value="raw">{t('session.telnet_mode_raw')}</option>
				</select>
				<p class="shell-hint">{t('session.telnet_mode_hint')}</p>
			</div>
			<div class="row">
				<div class="shell-field telnet-half">
					<label class="via-label" for="telnet-newline">{t('session.telnet_enter')}</label>
					<select id="telnet-newline" class="via-select" bind:value={telnetOptions.newline} disabled={saving}>
						<option value="crlf">CR LF</option>
						<option value="cr">CR</option>
						<option value="lf">LF</option>
					</select>
				</div>
				<div class="shell-field telnet-half">
					<label class="via-label" for="telnet-backspace">{t('session.telnet_backspace')}</label>
					<select id="telnet-backspace" class="via-select" bind:value={telnetOptions.backspace} disabled={saving}>
						<option value="del">DEL (^?)</option>
						<option value="ctrlh">Ctrl+H (^H)</option>
					</select>
				</div>
			</div>
			<p class="shell-hint">{t('session.telnet_keys_hint')}</p>
		{:else if kind === 'web'}
			<!-- The page runs in a window of its own that cannot reach the app;
			     the login is typed into it from the backend. See src-tauri/src/web. -->
			{#if !usedCredential}
				<div class="shell-field">
					<SecretRefField approvable label={t('session.password_optional')} bind:value={password} bind:secretRef={passwordRef} placeholder={t('session.stored_encrypted')} disabled={saving} onpick={(e) => (username = nameFromEntry(username, e))} />
				</div>
			{/if}
			<div class="row">
				<div class="shell-field telnet-half">
					<label class="via-label" for="web-scheme">{t('web.scheme')}</label>
					<select id="web-scheme" class="via-select" bind:value={webScheme} disabled={saving} onchange={() => { if (portStr === '443' || portStr === '80' || !portStr.trim()) portStr = webScheme === 'http' ? '80' : '443'; }}>
						<option value="https">HTTPS</option>
						<option value="http">HTTP</option>
					</select>
				</div>
				<div class="shell-field telnet-half">
					<Input label={t('web.path')} bind:value={webPath} placeholder="/" disabled={saving} />
				</div>
			</div>
			<div class="shell-field">
				<label class="via-label" for="web-login">{t('web.login')}</label>
				<select id="web-login" class="via-select" bind:value={webLogin} disabled={saving}>
					<option value="auto">{t('web.login_auto')}</option>
					<option value="idrac">Dell iDRAC</option>
					<option value="ilo">HPE iLO</option>
					<option value="proxmox">Proxmox VE / PBS</option>
					<option value="unifi">UniFi</option>
					<option value="generic">{t('web.login_generic')}</option>
					<option value="custom">{t('web.login_custom')}</option>
					<option value="off">{t('web.login_off')}</option>
				</select>
				<p class="shell-hint">{webLogin === 'off' ? t('web.login_off_hint') : t('web.login_hint')}</p>
			</div>
			{#if webLogin === 'custom'}
				<Input label={t('web.sel_username')} bind:value={webUserSel} placeholder="#username" disabled={saving} />
				<Input label={t('web.sel_password')} bind:value={webPassSel} placeholder="input[type=password]" disabled={saving} />
				<Input label={t('web.sel_submit')} bind:value={webSubmitSel} placeholder="button[type=submit]" disabled={saving} />
			{/if}
			{#if webLogin !== 'off'}
				<label class="jump-toggle">
					<input type="checkbox" bind:checked={webSubmit} disabled={saving} />
					<span class="jump-toggle-text">{t('web.submit')}</span>
				</label>
			{/if}
			<div class="shell-field">
				<label class="via-label" for="web-open">{t('web.open_in')}</label>
				<select id="web-open" class="via-select" bind:value={webOpenIn} disabled={saving}>
					<option value="">{isMobile() ? t('web.open_browser') : t('web.open_window')}</option>
					{#if !isMobile()}<option value="browser">{t('web.open_browser')}</option>{/if}
				</select>
				<p class="shell-hint">{webOpenIn === 'browser' || isMobile() ? t('web.open_browser_hint') : t('web.open_window_hint')}</p>
			</div>
			{#if webOpenIn !== 'browser' && !isMobile()}
				<div class="shell-field">
					<label class="via-label" for="web-storage">{t('web.storage')}</label>
					<select id="web-storage" class="via-select" bind:value={webStorage} disabled={saving}>
						<option value="">{t('web.storage_session')}</option>
						<option value="profile">{t('web.storage_profile')}</option>
						<option value="temporary">{t('web.storage_temporary')}</option>
					</select>
					{#if webStorage === 'profile'}
						<Input label={t('web.profile')} bind:value={webProfile} placeholder="pve-cluster" disabled={saving} />
					{/if}
					<p class="shell-hint">{webStorage === 'profile' ? t('web.storage_profile_hint') : webStorage === 'temporary' ? t('web.storage_temporary_hint') : t('web.storage_session_hint')}</p>
					{#if editSession && webStorage === ''}
						<div class="key-path-row">
							<Button variant="secondary" size="sm" onclick={clearWebData} disabled={saving || webDataCleared}>
								{webDataCleared ? t('web.data_cleared') : t('web.clear_data')}
							</Button>
						</div>
					{/if}
				</div>
				{#if editSession}
					<div class="shell-field">
						<span class="via-label">{t('web.certs')}</span>
						{#if webCerts.length === 0}
							<p class="shell-hint">{t('web.certs_none')}</p>
						{:else}
							{#each webCerts as c (c.sha256)}
								<div class="key-path-row">
									<code class="web-fp" title={c.sha256}>{c.host ? `${c.host} · ` : ''}{c.sha256}</code>
									<Button variant="secondary" size="sm" onclick={() => removeWebCert(c.sha256)} disabled={saving}>{t('web.cert_remove')}</Button>
								</div>
							{/each}
						{/if}
						<div class="key-path-row">
							<div class="key-path-input">
								<Input bind:value={webCertTyped} placeholder={t('web.cert_add_placeholder')} disabled={saving} />
							</div>
							<Button variant="secondary" size="sm" onclick={addWebCert} disabled={saving || !webCertTyped.trim()}>{t('web.cert_add')}</Button>
						</div>
						{#if webCertError}<p class="shell-hint web-error">{webCertError}</p>{/if}
						<p class="shell-hint">{t('web.certs_hint')}</p>
					</div>
				{/if}
			{/if}
		{:else if kind === 'serial'}
			<!-- A serial line has no login of its own; the device's console may
			     ask for one, typed in the terminal. -->
			<div class="row">
				<div class="shell-field telnet-half">
					<label class="via-label" for="serial-baud">{t('serial.speed')}</label>
					<select id="serial-baud" class="via-select" bind:value={serialOptions.baud_rate} disabled={saving}>
						{#each baudRates as b (b)}
							<option value={b}>{b}</option>
						{/each}
					</select>
				</div>
				<div class="shell-field telnet-half">
					<label class="via-label" for="serial-data-bits">{t('serial.data_bits')}</label>
					<select id="serial-data-bits" class="via-select" bind:value={serialOptions.data_bits} disabled={saving}>
						{#each [8, 7, 6, 5] as b (b)}
							<option value={b}>{b}</option>
						{/each}
					</select>
				</div>
			</div>
			<div class="row">
				<div class="shell-field telnet-half">
					<label class="via-label" for="serial-parity">{t('serial.parity')}</label>
					<select id="serial-parity" class="via-select" bind:value={serialOptions.parity} disabled={saving}>
						<option value="none">{t('serial.parity_none')}</option>
						<option value="odd">{t('serial.parity_odd')}</option>
						<option value="even">{t('serial.parity_even')}</option>
					</select>
				</div>
				<div class="shell-field telnet-half">
					<label class="via-label" for="serial-stop-bits">{t('serial.stop_bits')}</label>
					<select id="serial-stop-bits" class="via-select" bind:value={serialOptions.stop_bits} disabled={saving}>
						<option value={1}>1</option>
						<option value={2}>2</option>
					</select>
				</div>
			</div>
			<div class="shell-field">
				<label class="via-label" for="serial-flow">{t('serial.flow_control')}</label>
				<select id="serial-flow" class="via-select" bind:value={serialOptions.flow_control} disabled={saving}>
					<option value="none">{t('serial.flow_none')}</option>
					<option value="software">{t('serial.flow_software')}</option>
					<option value="hardware">{t('serial.flow_hardware')}</option>
				</select>
			</div>
			<div class="row">
				<div class="shell-field telnet-half">
					<label class="via-label" for="serial-newline">{t('session.telnet_enter')}</label>
					<select id="serial-newline" class="via-select" bind:value={serialOptions.newline} disabled={saving}>
						<option value="cr">CR</option>
						<option value="crlf">CR LF</option>
						<option value="lf">LF</option>
					</select>
				</div>
				<div class="shell-field telnet-half">
					<label class="via-label" for="serial-backspace">{t('session.telnet_backspace')}</label>
					<select id="serial-backspace" class="via-select" bind:value={serialOptions.backspace} disabled={saving}>
						<option value="del">DEL (^?)</option>
						<option value="ctrlh">Ctrl+H (^H)</option>
					</select>
				</div>
			</div>
			<div class="shell-field">
				<label class="jump-toggle">
					<input type="checkbox" bind:checked={serialOptions.local_echo} disabled={saving} />
					<span class="jump-toggle-text">{t('serial.local_echo')}</span>
				</label>
				<p class="shell-hint">{t('serial.hint')}</p>
			</div>
		{:else if kind === 'winbox'}
			<!-- WinBox logs in by itself with what is saved here; how the
			     password reaches it is Settings' choice (Sessions). -->
			{#if !usedCredential}
				<div class="shell-field">
					<SecretRefField approvable label={t('session.password_optional')} bind:value={password} bind:secretRef={passwordRef} placeholder={t('session.stored_encrypted')} disabled={saving} onpick={(e) => (username = nameFromEntry(username, e))} />
					{@render pastPasswords()}
				</div>
			{/if}
			<p class="shell-hint">{t('winbox.editor_hint')}</p>
		{:else if kind === 'wsl'}
			<!-- wsl.exe signs in by itself: no password, key, jump host, proxy or
			     ssh_config here. Only which Linux user, if not the distro's default. -->
			<div class="shell-field">
				<Input label={t('session.wsl_user')} bind:value={username} placeholder={t('session.wsl_user_placeholder')} disabled={saving} />
				<p class="shell-hint">{t('session.wsl_user_hint')}</p>
			</div>
		{:else}
		{#if !usedCredential}
		<div class="auth-section">
			<span class="auth-label">{t('session.auth_method')}</span>
			<div class="auth-toggle">
				<button
					type="button"
					class="auth-btn"
					class:active={authType === 'Password'}
					disabled={saving}
					onclick={() => (authType = 'Password')}
				>
					{t('session.auth_password')}
				</button>
				<button
					type="button"
					class="auth-btn"
					class:active={authType === 'Key'}
					disabled={saving}
					onclick={() => (authType = 'Key')}
				>
					{t('session.auth_key')}
				</button>
				<button
					type="button"
					class="auth-btn"
					class:active={authType === 'Agent'}
					disabled={saving}
					onclick={() => (authType = 'Agent')}
				>
					{t('session.auth_agent')}
				</button>
				<button
					type="button"
					class="auth-btn"
					class:active={authType === 'Manager'}
					disabled={saving}
					onclick={() => (authType = 'Manager')}
				>
					{t('session.auth_manager')}
				</button>
			</div>
		</div>

		{#if authType === 'Password'}
			<Input label={t('session.password_optional')} type="password" bind:value={password} placeholder={t('session.stored_encrypted')} disabled={saving} />
			{@render pastPasswords()}
		{:else if authType === 'Manager'}
			<SecretRefField approvable refOnly label={t('session.password_from_manager')} bind:value={password} bind:secretRef={passwordRef} disabled={saving} onpick={(e) => (username = nameFromEntry(username, e))} />
		{:else if authType === 'Key'}
			<KeyPicker bind:path={keyPath} bind:keyId disabled={saving} />
			{#if !keyId}
				<SecretRefField approvable label={t('session.passphrase_optional')} bind:value={keyPassphrase} bind:secretRef={keyPassphraseRef} placeholder={t('session.stored_encrypted')} disabled={saving} />
			{/if}
			<div class="shell-field">
				<label class="jump-toggle">
					<input type="checkbox" bind:checked={tryAgentKeys} disabled={saving} />
					<span class="jump-toggle-text">{t('session.try_agent_keys')}</span>
				</label>
				<p class="shell-hint">{t('session.try_agent_keys_hint')}</p>
			</div>
		{/if}
		{/if}

		<div class="jump-section">
			<label class="jump-toggle">
				<input type="checkbox" bind:checked={jumpEnabled} disabled={saving} />
				<span class="jump-toggle-text">{t('session.jump_host_enable')}</span>
			</label>

			{#if jumpEnabled}
				<p class="jump-hint">{t('session.jump_host_hint')}</p>

				{#each jumpHops as hop, i (i)}
					<div class="jump-hop">
						<div class="jump-hop-header">
							<span class="jump-hop-label">{t('session.jump_hop', { n: String(i + 1) })}</span>
							<button type="button" class="jump-hop-remove" onclick={() => removeHop(i)} disabled={saving}>
								{t('session.jump_remove_hop')}
							</button>
						</div>
						<div class="row">
							<div class="field-host">
								<Input label={t('session.host')} bind:value={hop.host} placeholder="bastion.example.com" disabled={saving} />
							</div>
							<div class="field-port">
								<Input label={t('session.port')} bind:value={hop.port} type="number" placeholder="22" disabled={saving} />
							</div>
						</div>
						{#if credentials.length > 0 || hop.credentialId}
							<div class="shell-field">
								<label class="via-label" for={`hop-login-${i}`}>{t('credential.login')}</label>
								<select id={`hop-login-${i}`} class="via-select" value={hop.credentialId} onchange={(e) => pickHopCredential(hop, e.currentTarget.value)} disabled={saving}>
									<option value="">{t('credential.own_login_hop')}</option>
									{#each credentials as c (c.id)}
										<option value={c.id}>{c.name} ({c.username})</option>
									{/each}
									{#if hop.credentialId && !hopCredential(hop)}
										<option value={hop.credentialId}>{t('credential.missing')}</option>
									{/if}
								</select>
								{#if hopCredential(hop)}
									<p class="shell-hint">{t('credential.login_hint', { username: hopCredential(hop)?.username ?? '' })}</p>
								{:else if hop.credentialId}
									<p class="shell-hint">{t('credential.missing_hint')}</p>
								{/if}
							</div>
						{/if}
						{#if !hopCredential(hop)}
						<Input label={t('session.username')} bind:value={hop.username} placeholder="root" disabled={saving} />

						<div class="auth-section">
							<span class="auth-label">{t('session.auth_method')}</span>
							<div class="auth-toggle">
								<button type="button" class="auth-btn" class:active={hop.authType === 'Password'} disabled={saving} onclick={() => (hop.authType = 'Password')}>
									{t('session.auth_password')}
								</button>
								<button type="button" class="auth-btn" class:active={hop.authType === 'Key'} disabled={saving} onclick={() => (hop.authType = 'Key')}>
									{t('session.auth_key')}
								</button>
								<button type="button" class="auth-btn" class:active={hop.authType === 'Agent'} disabled={saving} onclick={() => (hop.authType = 'Agent')}>
									{t('session.auth_agent')}
								</button>
								<button type="button" class="auth-btn" class:active={hop.authType === 'Manager'} disabled={saving} onclick={() => (hop.authType = 'Manager')}>
									{t('session.auth_manager')}
								</button>
							</div>
						</div>

						{#if hop.authType === 'Password'}
							<Input label={t('session.password_optional')} type="password" bind:value={hop.password} disabled={saving} />
						{:else if hop.authType === 'Manager'}
							<SecretRefField approvable refOnly label={t('session.password_from_manager')} bind:value={hop.password} bind:secretRef={hop.passwordRef} disabled={saving} onpick={(e) => (hop.username = nameFromEntry(hop.username, e))} />
						{:else if hop.authType === 'Key'}
							<div class="key-path-row">
								<div class="key-path-input">
									<Input label={t('session.key_path')} bind:value={hop.keyPath} placeholder="~/.ssh/id_rsa" disabled={saving} />
								</div>
								<Button variant="secondary" size="sm" onclick={() => browseKey((p) => (hop.keyPath = p))} disabled={saving}>
									{t('session.browse_key')}
								</Button>
							</div>
							<SecretRefField approvable label={t('session.passphrase_optional')} bind:value={hop.keyPassphrase} bind:secretRef={hop.keyPassphraseRef} disabled={saving} />
						{/if}
						{/if}
					</div>
				{/each}

				<button type="button" class="jump-add-btn" onclick={addHop} disabled={saving}>
					+ {t('session.jump_add_hop')}
				</button>
			{/if}
		</div>

		<div class="proxy-section">
			<label class="proxy-toggle">
				<input type="checkbox" bind:checked={proxyEnabled} disabled={saving} />
				<span class="proxy-toggle-text">{t('proxy.connect_via')}</span>
			</label>

			{#if proxyEnabled}
				<div class="proxy-fields">
					<div class="proxy-type-row">
						<button type="button" class="proxy-type-btn" class:active={proxyType === 'socks5'} onclick={() => (proxyType = 'socks5')} disabled={saving}>SOCKS5</button>
						<button type="button" class="proxy-type-btn" class:active={proxyType === 'socks4'} onclick={() => (proxyType = 'socks4')} disabled={saving}>SOCKS4</button>
						<button type="button" class="proxy-type-btn" class:active={proxyType === 'http'} onclick={() => (proxyType = 'http')} disabled={saving}>HTTP</button>
					</div>
					<div class="row">
						<div class="field-host">
							<Input label={t('proxy.host')} bind:value={proxyHost} placeholder="127.0.0.1" disabled={saving} />
						</div>
						<div class="field-port">
							<Input label={t('session.port')} bind:value={proxyPort} type="number" placeholder="9050" disabled={saving} />
						</div>
					</div>
					<div class="row">
						<div class="field-host">
							<Input label={t('proxy.username_optional')} bind:value={proxyUsername} placeholder="" disabled={saving} />
						</div>
						<div class="field-host">
							<SecretRefField approvable label={t('session.password_optional')} bind:value={proxyPassword} bind:secretRef={proxyPasswordRef} disabled={saving} onpick={(e) => { if (!proxyUsername.trim() && e.username) proxyUsername = e.username; }} />
						</div>
					</div>
					<p class="proxy-hint">
						{#if proxyType === 'socks5'}
							{t('proxy.hint_socks5')}
						{:else if proxyType === 'http'}
							{t('proxy.hint_http')}
						{:else}
							{t('proxy.hint_socks4')}
						{/if}
					</p>
				</div>
			{/if}
		</div>

		{#key source?.id ?? 'new'}
			<SshOptionsSection
				bind:this={sshSection}
				bind:options={sshOptions}
				{host}
				port={parseInt(portStr, 10) || 22}
				{username}
				disabled={saving}
			/>
		{/key}
		{#if kind === 'ssh' && editSession}
			{#key editSession.id}
				<SessionHostKeys sessionId={editSession.id} initial={editSession.host_keys} disabled={saving} />
			{/key}
		{/if}
		{/if}

		</div>
		<div class="column">
		{#if wide}<span class="column-title">{opensTerminal ? t('session.column_terminal') : t('session.column_organising')}</span>{/if}
		<Input label={t('session.tags')} bind:value={tagsStr} placeholder={kind === 'rdp' ? 'office, windows' : 'production, web, linux'} disabled={saving} />

		{#if kind === 'ssh' || kind === 'telnet' || kind === 'vnc' || kind === 'spice' || kind === 'winbox'}
			<div class="folder-section">
				<span class="folder-label">{t('session.os_icon')}</span>
				<div class="os-row">
					<OsIconSelect options={osIconOptions} bind:value={osChoice} disabled={saving} />
					{#if kind === 'ssh' && !osChoice && autoOs}
						<Button variant="secondary" size="sm" onclick={() => (osCleared = true)} disabled={saving}>
							{t('session.os_clear')}
						</Button>
					{/if}
				</div>
				{#if kind === 'ssh' && !osChoice && osCleared}
					<span class="os-cleared">{t('session.os_cleared')}</span>
				{/if}
			</div>
		{/if}

		{#if vaultFolders.length > 0}
			<div class="folder-section">
				<span class="folder-label">{t('session.folder')}</span>
				<select class="folder-select" bind:value={folderIdStr} disabled={saving}>
					<option value="">{t('session.no_folder')}</option>
					{#each vaultFolders as folder (folder.id)}
						<option value={folder.id}>{folderPath(folder.id, folders)}</option>
					{/each}
				</select>
			</div>
		{/if}

		{#if kind !== 'rdp' && kind !== 'vnc' && kind !== 'spice' && kind !== 'winbox'}
			<Disclosure title={t('termtheme.session_title')} summary={colorsSummary} bind:open={colorsOpen}>
				<TerminalThemeList
					selected={terminalTheme}
					onselect={(id) => (terminalTheme = id)}
					noneLabel={t('termtheme.session_default')}
					nonePalette={terminalPalette(null)}
					disabled={saving}
				/>
				<p class="colors-hint">{t('termtheme.session_hint')}</p>
			</Disclosure>
			<div class="folder-section">
				<span class="folder-label">{t('cursor.session_label')}</span>
				<div class="cursor-choices">
					<select class="folder-select" bind:value={cursorChoice} disabled={saving} aria-label={t('cursor.session_label')}>
						<option value="">{t('cursor.session_default', { style: t(`cursor.style_${getSettings().terminalCursorStyle}`) })}</option>
						<option value="block">{t('cursor.style_block')}</option>
						<option value="underline">{t('cursor.style_underline')}</option>
						<option value="bar">{t('cursor.style_bar')}</option>
					</select>
					<select class="folder-select" bind:value={blinkChoice} disabled={saving} aria-label={t('cursor.blink')}>
						<option value="">{getSettings().terminalCursorBlink ? t('cursor.blink_default_on') : t('cursor.blink_default_off')}</option>
						<option value="on">{t('cursor.blink_on')}</option>
						<option value="off">{t('cursor.blink_off')}</option>
					</select>
				</div>
			</div>
			<div class="folder-section">
				<span class="folder-label">{t('history.session_label')}</span>
				<select class="folder-select" bind:value={historyChoice} disabled={saving}>
					<option value="">{t('history.session_default')}</option>
					<option value="off">{t('history.mode_off')}</option>
					<option value="commands">{t('history.mode_commands')}</option>
					<option value="output">{t('history.mode_output')}</option>
				</select>
			</div>
		{/if}

		{#if kind === 'ssh'}
			<div class="folder-section">
				<label class="folder-label" for="session-term-type">{t('session.term_type')}</label>
				<input id="session-term-type" class="field-input" list="session-term-types" bind:value={termType} placeholder="xterm-256color" spellcheck="false" autocapitalize="off" autocomplete="off" disabled={saving} />
				<datalist id="session-term-types">
					{#each TERMINAL_TYPES as tt (tt)}<option value={tt}></option>{/each}
				</datalist>
				<p class="shell-hint">{t('session.term_type_hint')}</p>
			</div>
			<div class="folder-section">
				<span class="folder-label">{t('session.telnet_backspace')}</span>
				<select class="folder-select" bind:value={backspaceChoice} disabled={saving}>
					<option value="">DEL (^?, ASCII 127)</option>
					<option value="ctrlh">Ctrl+H (^H, ASCII 8)</option>
				</select>
			</div>
			<div class="folder-section">
				<span class="folder-label">{t('session.encoding')}</span>
				<select class="folder-select" bind:value={encodingChoice} disabled={saving}>
					<option value="">UTF-8</option>
					{#each TERMINAL_ENCODINGS.filter((e) => e !== 'UTF-8') as enc (enc)}
						<option value={enc}>{enc}</option>
					{/each}
					{#if encodingChoice && !TERMINAL_ENCODINGS.includes(encodingChoice)}
						<option value={encodingChoice}>{encodingChoice}</option>
					{/if}
				</select>
				<p class="shell-hint">{t('session.encoding_hint')}</p>
			</div>
		{/if}

		{#if kind === 'ssh' || kind === 'wsl'}
			<div class="folder-section">
				<span class="folder-label">{t('session.monitoring')}</span>
				<select class="folder-select" bind:value={monitoringChoice} disabled={saving}>
					<option value="">{getSettings().serverMonitoring ? t('session.monitoring_default_on') : t('session.monitoring_default_off')}</option>
					<option value="on">{t('session.monitoring_on')}</option>
					<option value="off">{t('session.monitoring_off')}</option>
				</select>
			</div>
		{/if}

		{#if kind === 'ssh' && (!isMobile() || x11Desktop)}
			<div class="folder-section">
				<span class="folder-label">{t('x11desktop.label')}</span>
				<select class="folder-select" bind:value={x11Desktop} disabled={saving}>
					<option value="">{t('x11desktop.off')}</option>
					{#each X11_DESKTOPS as d (d)}
						<option value={d}>{t(`x11desktop.${d}`)}</option>
					{/each}
				</select>
			</div>
			{#if x11Desktop === 'custom'}
				<div class="shell-field">
					<Input label={t('x11desktop.command')} bind:value={x11Command} placeholder="i3" disabled={saving} />
				</div>
			{/if}
			{#if x11Desktop}
				<p class="shell-hint">{t('x11desktop.hint')}</p>
			{/if}
		{/if}

		{#if kind === 'ssh' && !x11Desktop}
			<div class="shell-field">
				<Input label={t('session.login_shell_optional')} bind:value={shell} placeholder="fish -l" disabled={saving} />
				<p class="shell-hint">{t('session.login_shell_hint')}</p>
			</div>
		{/if}

		{#if tmuxKind}
			<div class="folder-section">
				<span class="folder-label">{kind === 'wsl' ? t('tmux.session_label_wsl') : t('tmux.session_label')}</span>
				<select class="folder-select" bind:value={tmuxChoice} disabled={saving}>
					<option value="">{tmuxFolderOn ? t('tmux.session_folder_on') : t('tmux.session_folder_off')}</option>
					<option value="attach">{t('tmux.mode_attach')}</option>
					{#if kind === 'ssh'}<option value="ask">{t('tmux.mode_ask')}</option>{/if}
					<option value="off">{t('tmux.mode_off')}</option>
				</select>
			</div>
			{#if tmuxOn}
				<div class="shell-field">
					<label class="via-label" for="session-tmux-name">{t('tmux.name_label')}</label>
					<input id="session-tmux-name" class="field-input" bind:value={tmuxName} placeholder={defaultTmuxName(name.trim())} spellcheck="false" autocomplete="off" disabled={saving} />
					<p class="shell-hint">{tmuxChoice === 'ask' ? t('tmux.ask_hint') : t('tmux.session_hint')}</p>
				</div>
				<div class="shell-field">
					<label class="jump-toggle">
						<input type="checkbox" bind:checked={tmuxMouse} disabled={saving} />
						<span class="jump-toggle-text">{t('tmux.mouse')}</span>
					</label>
					<p class="shell-hint">{t('tmux.mouse_hint')}</p>
				</div>
			{/if}
		{/if}

		<Disclosure title={t('session.docs')} summary={docsSummary} bind:open={docsOpen}>
			<div class="docs">
				<div class="shell-field">
					<label class="via-label" for="session-notes">{t('session.notes')}</label>
					<textarea id="session-notes" class="notes-input" rows="5" bind:value={notes} placeholder={t('session.notes_placeholder')} disabled={saving}></textarea>
				</div>
				<div class="shell-field">
					<span class="via-label">{t('session.custom_fields')}</span>
					{#each customFields as field, i (i)}
						<div class="field-row">
							<input class="field-input field-name" bind:value={field.name} placeholder={t('session.field_name')} aria-label={t('session.field_name')} disabled={saving} />
							<input class="field-input field-value" type={field.hidden && !field.shown ? 'password' : 'text'} bind:value={field.value} placeholder={t('session.field_value')} aria-label={t('session.field_value')} disabled={saving} />
							{#if field.hidden}
								<button type="button" class="field-btn" onclick={() => (field.shown = !field.shown)} title={field.shown ? t('session.field_conceal') : t('session.field_reveal')} aria-label={field.shown ? t('session.field_conceal') : t('session.field_reveal')} disabled={saving}>
									<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
										{#if field.shown}
											<path d="M17.94 17.94A10 10 0 0 1 12 20C5 20 1 12 1 12a18 18 0 0 1 5.06-5.94M9.9 4.24A9 9 0 0 1 12 4c7 0 11 8 11 8a18 18 0 0 1-2.16 3.19M1 1l22 22"/>
										{:else}
											<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8S1 12 1 12z"/><circle cx="12" cy="12" r="3"/>
										{/if}
									</svg>
								</button>
							{/if}
							<label class="field-hidden" title={t('session.field_hidden_hint')}>
								<input type="checkbox" bind:checked={field.hidden} onchange={() => (field.shown = !field.hidden)} disabled={saving} />
								{t('session.field_hidden')}
							</label>
							<button type="button" class="field-btn" onclick={() => removeField(i)} title={t('session.field_remove')} aria-label={t('session.field_remove')} disabled={saving}>×</button>
						</div>
					{/each}
					<button type="button" class="jump-add-btn" onclick={addField} disabled={saving}>
						+ {t('session.field_add')}
					</button>
				</div>
			</div>
		</Disclosure>
		</div>
		</div>

		{#if error}
			<div class="error-message">{error}</div>
		{/if}
	</form>

	{#snippet actions()}
		<Button variant="secondary" onclick={handleClose} disabled={saving}>
			{t('common.cancel')}
		</Button>
		<Button variant="primary" onclick={handleSave} disabled={!canSave}>
			{#if saving}
				<span class="spinner"></span>
				{t('session.saving')}
			{:else}
				{isEditing ? t('session.update_session') : t('session.save_session')}
			{/if}
		</Button>
	{/snippet}
</Modal>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.row {
		display: flex;
		gap: 10px;
		align-items: flex-start;
	}

	.telnet-half {
		flex: 1;
		min-width: 0;
	}

	.link-btn {
		background: none;
		border: none;
		padding: 0;
		color: var(--color-accent);
		font: inherit;
		cursor: pointer;
	}

	.link-btn:hover {
		text-decoration: underline;
	}

	.field-host {
		flex: 1;
		min-width: 0;
	}

	.field-port {
		width: 80px;
		flex-shrink: 0;
	}

	.key-path-row {
		display: flex;
		align-items: flex-end;
		gap: 8px;
	}

	.key-path-input {
		flex: 1;
		min-width: 0;
	}

	.folder-section {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.os-row {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.os-cleared {
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.folder-label {
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--color-text-secondary);
	}

	.folder-select {
		padding: 6px 10px;
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		color: var(--color-text-primary);
		font-family: var(--font-sans);
		font-size: 0.75rem;
		outline: none;
	}

	.cursor-choices {
		display: flex;
		gap: 6px;
	}

	.cursor-choices .folder-select {
		flex: 1;
		min-width: 0;
	}

	.folder-select:focus {
		border-color: var(--color-accent);
	}

	.auth-section {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.auth-label {
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--color-text-secondary);
	}

	.shell-field {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.docs {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.colors-hint {
		margin: 6px 0 0;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.notes-input,
	.field-input {
		padding: 6px 10px;
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		color: var(--color-text-primary);
		font-family: var(--font-sans);
		font-size: 0.75rem;
		outline: none;
	}

	.notes-input {
		resize: vertical;
		min-height: 80px;
		line-height: 1.4;
	}

	.notes-input:focus,
	.field-input:focus {
		border-color: var(--color-accent);
	}

	.field-row {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.field-name {
		width: 30%;
		min-width: 0;
	}

	.field-value {
		flex: 1;
		min-width: 0;
	}

	.field-hidden {
		display: flex;
		align-items: center;
		gap: 4px;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
		white-space: nowrap;
		cursor: pointer;
	}

	.field-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		flex-shrink: 0;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		font-size: 1rem;
	}

	.field-btn:hover:not(:disabled) {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.shell-hint {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	/* A segmented control: a recessed track, the choice raised out of it.
	   A solid accent fill on two of these stacked was the loudest thing in
	   the dialog, louder than Save. */
	.auth-toggle {
		display: flex;
		gap: 2px;
		padding: 3px;
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
	}

	/* The protocols: as many to a row as fit, then the next row. A grid,
	   so the rows line up and every button is as wide as its neighbours. */
	/* A scrollbar that comes and goes would shift the columns sideways. */
	:global(.modal:has(.session-form) .modal-body) {
		scrollbar-gutter: stable;
	}

	.category-tabs {
		display: flex;
		gap: 4px;
		border-bottom: 1px solid var(--color-border);
		margin-bottom: 4px;
	}

	.category-tab {
		padding: 6px 12px 8px;
		margin-bottom: -1px;
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		font-weight: 600;
		white-space: nowrap;
		border: none;
		border-bottom: 2px solid transparent;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.category-tab:hover:not(:disabled):not(.active) {
		color: var(--color-text-primary);
	}

	.category-tab.active {
		color: var(--color-accent);
		border-bottom-color: var(--color-accent);
	}

	.protocol-toggle {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(84px, 1fr));
	}

	.columns,
	.column {
		display: contents;
	}

	.columns.wide {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: 28px;
		align-items: start;
		margin-top: 4px;
	}

	.columns.wide .column {
		display: flex;
		flex-direction: column;
		gap: 12px;
		min-width: 0;
	}

	.columns.wide .column + .column {
		border-left: 1px solid var(--color-border);
		padding-left: 28px;
	}

	.column-title {
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--color-text-secondary);
		padding-bottom: 6px;
		border-bottom: 1px solid var(--color-border);
	}

	/* Sized to its label first, so a long one ("Remote desktop (RDP)") is
	   whole where there is room, then cut only where there is not. */
	.auth-btn {
		flex: 1 1 auto;
		min-width: 0;
		padding: 6px 10px;
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition:
			background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default);
	}

	.auth-btn:hover:not(:disabled):not(.active) {
		color: var(--color-text-primary);
	}

	.auth-btn.active {
		background-color: var(--color-bg-elevated);
		color: var(--color-accent);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25), inset 0 0 0 1px var(--color-border);
	}

	.auth-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.error-message {
		padding: 8px 12px;
		font-size: 0.8125rem;
		color: var(--color-danger);
		background-color: rgba(255, 69, 58, 0.08);
		border: 1px solid rgba(255, 69, 58, 0.2);
		border-radius: var(--radius-btn);
	}

	.spinner {
		display: inline-block;
		width: 14px;
		height: 14px;
		border: 2px solid var(--color-border);
		border-top-color: #fff;
		border-radius: 50%;
		animation: spin 0.6s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	.proxy-section {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding-top: 4px;
	}

	.proxy-toggle {
		display: flex;
		align-items: center;
		gap: 8px;
		cursor: pointer;
	}

	.proxy-toggle input[type="checkbox"] {
		width: 14px;
		height: 14px;
		accent-color: var(--color-accent);
		cursor: pointer;
	}

	.proxy-toggle-text {
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-text-primary);
	}

	.proxy-fields {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 10px;
		background: var(--color-surface-hover);
		border: 1px solid var(--color-border);
		border-radius: 8px;
	}

	.proxy-type-row {
		display: flex;
		gap: 0;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		overflow: hidden;
	}

	.proxy-type-btn {
		flex: 1;
		padding: 5px 8px;
		font-family: var(--font-sans);
		font-size: 0.6875rem;
		font-weight: 500;
		border: none;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default), color var(--duration-default) var(--ease-default);
	}

	.proxy-type-btn.active {
		background-color: var(--color-accent-fill);
		color: #fff;
	}

	.proxy-type-btn:not(.active):hover {
		background-color: var(--color-surface-hover);
	}

	.proxy-hint {
		margin: 0;
		font-size: 0.625rem;
		color: var(--color-text-secondary);
		opacity: 0.7;
	}

	.via-label {
		display: block;
		margin-bottom: 4px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	/* Dressed as the text fields around it. */
	.spice-ca {
		width: 100%;
		padding: 10px 12px;
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--color-text-primary);
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		outline: none;
		box-sizing: border-box;
		resize: vertical;
	}

	.via-select {
		width: 100%;
		padding: 12px;
		font-family: var(--font-sans);
		font-size: 0.875rem;
		color: var(--color-text-primary);
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		outline: none;
		box-sizing: border-box;
	}

	.via-select:focus {
		border-color: var(--color-accent);
	}

	.jump-section {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding-top: 4px;
	}

	.jump-toggle {
		display: flex;
		align-items: center;
		gap: 8px;
		cursor: pointer;
	}

	.jump-toggle input {
		width: 14px;
		height: 14px;
		accent-color: var(--color-accent);
	}

	.jump-toggle-text {
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-text-primary);
	}


	.jump-hint {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.jump-hop {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 10px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		background-color: var(--color-surface-hover);
	}

	.jump-hop-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
	}

	.jump-hop-label {
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--color-text-secondary);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	.jump-hop-remove {
		padding: 2px 8px;
		font-family: var(--font-sans);
		font-size: 0.6875rem;
		color: var(--color-danger);
		background: transparent;
		border: 1px solid rgba(255, 69, 58, 0.3);
		border-radius: 4px;
		cursor: pointer;
	}

	.jump-hop-remove:hover:not(:disabled) {
		background-color: rgba(255, 69, 58, 0.08);
	}

	.jump-hop-remove:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.jump-add-btn {
		padding: 6px 12px;
		font-family: var(--font-sans);
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-accent);
		background: transparent;
		border: 1px dashed var(--color-accent);
		border-radius: var(--radius-btn);
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.jump-add-btn:hover:not(:disabled) {
		background-color: rgba(0, 122, 255, 0.08);
	}

	.jump-add-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.web-fp {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 0.72rem;
		color: var(--color-text-secondary);
	}

	.web-error {
		color: var(--color-danger, #e5484d);
	}
</style>
