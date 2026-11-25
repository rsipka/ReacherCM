<script lang="ts">
	import PasswordField from '#lib/components/shared/PasswordField.svelte';
	// Editing many sessions at once. Every field starts as "keep": it shows
	// the value the sessions share, or "Mixed", and nothing is sent for it.
	// Typing or picking makes it a change (an accent bar and a revert
	// button); optional fields can also be cleared on every session. Apply
	// shows what will change before anything is written.
	import Modal from '#lib/components/shared/Modal.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import TerminalThemeList from '#lib/components/terminal/TerminalThemeList.svelte';
	import KeyPicker from './KeyPicker.svelte';
	import BulkSelectRow from './BulkSelectRow.svelte';
	import OsIconSelect from './OsIconSelect.svelte';
	import { wireguardProfiles } from '#lib/state/wireguard.svelte.js';
	import { osChoices } from '#lib/data/distro-icons.js';
	import { RDP_INTERVALS, RDP_KEYBOARDS, RDP_RESOLUTIONS, RDP_SCALES, rdpIntervalLabel } from '#lib/sessions/rdp-choices.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { findTerminalTheme, terminalPalette } from '#lib/state/terminal-themes.svelte.js';
	import {
		sessionBulkUpdate,
		type AuthMethod,
		type BulkResult,
		type JumpHostConfig,
		type RdpPatch,
		type SessionConfig,
		type SessionPatch,
		TERMINAL_ENCODINGS,
		TERMINAL_TYPES
	} from '#lib/ipc/sessions.js';
	import { savedCredentialCreate, credentialsFor, type SavedCredential } from '#lib/ipc/savedCredentials.js';
	import type { HistoryMode } from '#lib/ipc/history.js';
	import type { CursorStyle } from '#lib/terminal/cursor.js';
	import {
		OWN_LOGIN,
		appliesTo,
		changedFields,
		countApplying,
		describeCounts,
		hasOwnPassword,
		parseTags,
		summarize,
		vaultsOf,
		type BulkField
	} from '#lib/sessions/bulk-edit.js';

	interface Props {
		open: boolean;
		sessions: SessionConfig[];
		/** Every saved session, for jump hosts and a VNC session's SSH tunnel. */
		allSessions?: SessionConfig[];
		credentials: SavedCredential[];
		onclose: () => void;
		/** After a write: the backend's report, to reload and offer Undo. */
		onapplied: (result: BulkResult) => void;
	}

	let { open, sessions, allSessions = [], credentials, onclose, onapplied }: Props = $props();

	const KEEP = '__keep__';
	const FOLDER = '__folder__';

	// ---- What the user changed (null / KEEP: leave as is) ------------------
	let username = $state<string | null>(null);
	let password = $state<string | null>(null);
	let passwordClear = $state(false);
	let saveAsCredential = $state(true);
	let credentialName = $state('');
	let credential = $state(KEEP);
	let port = $state<string | null>(null);
	let domain = $state<string | null>(null);
	let domainClear = $state(false);
	let tagsAdd = $state('');
	let tagsRemove = $state('');
	let notesOp = $state<'keep' | 'replace' | 'append' | 'clear'>('keep');
	let notesText = $state('');
	let theme = $state<string | null>(null);
	let history = $state(KEEP);
	let cursor = $state(KEEP);
	let blink = $state(KEEP);
	// Phase 2: login method, connection, Telnet, server side, custom fields.
	let authType = $state(KEEP);
	let keyPath = $state('');
	let keyId = $state('');
	let keyPassphrase = $state('');
	let agentKeys = $state(KEEP);
	/** One jump host as typed here; `login` is 'agent', 'password' or a credential id. */
	interface Hop {
		host: string;
		port: string;
		username: string;
		login: string;
		password: string;
	}
	let jumpMode = $state(KEEP);
	let hops = $state<Hop[]>([]);
	let proxyMode = $state(KEEP);
	let proxyType = $state<'socks5' | 'socks4' | 'http'>('socks5');
	let proxyHost = $state('');
	let proxyPort = $state('9050');
	let proxyUser = $state('');
	let proxyPass = $state('');
	let shell = $state<string | null>(null);
	let shellClear = $state(false);
	let fieldsSet = $state<{ name: string; value: string; hidden: boolean }[]>([]);
	let fieldsRemove = $state('');
	let telnetMode = $state(KEEP);
	let telnetNewline = $state(KEEP);
	let telnetBackspace = $state(KEEP);
	let monitoring = $state(KEEP);
	let tmuxMode = $state(KEEP);
	let tmuxMouse = $state(KEEP);
	let via = $state(KEEP);
	// Phase 3: settings added to sessions since.
	let wireguard = $state(KEEP);
	let connectMode = $state(KEEP);
	let addressCheck = $state(KEEP);
	let filesMode = $state(KEEP);
	let termType = $state<string | null>(null);
	let termTypeClear = $state(false);
	let backspace = $state(KEEP);
	let encoding = $state(KEEP);
	/** RDP choices by row key (see `rdpRows`): KEEP, or an option's value. */
	let rdpSel = $state<Record<string, string>>({});
	let rdpKdc = $state<string | null>(null);
	let rdpKdcClear = $state(false);
	let gatewayMode = $state(KEEP);
	let gatewayHost = $state('');
	let gatewayDetect = $state(false);
	let gatewayRpc = $state(false);
	let gatewayOwnLogin = $state(false);
	let gatewayUser = $state('');
	let gatewayPass = $state('');
	let filesPort = $state<string | null>(null);
	let filesPortClear = $state(false);
	let wolBroadcast = $state<string | null>(null);
	let wolBroadcastClear = $state(false);
	let wolPort = $state<string | null>(null);
	let wolPortClear = $state(false);
	let favorite = $state(KEEP);
	let osIcon = $state(KEEP);

	let phase = $state<'edit' | 'confirm' | 'saving' | 'report'>('edit');
	let error = $state('');
	let report = $state<BulkResult | null>(null);
	let themesOpen = $state(false);

	function reset(): void {
		username = password = port = domain = theme = null;
		passwordClear = domainClear = false;
		saveAsCredential = true;
		credentialName = '';
		credential = history = cursor = blink = KEEP;
		authType = agentKeys = jumpMode = proxyMode = KEEP;
		telnetMode = telnetNewline = telnetBackspace = monitoring = tmuxMode = tmuxMouse = via = KEEP;
		keyPath = keyId = keyPassphrase = '';
		hops = [newHop()];
		proxyType = 'socks5';
		proxyHost = proxyUser = proxyPass = '';
		proxyPort = '9050';
		shell = null;
		shellClear = false;
		fieldsSet = [];
		fieldsRemove = '';
		wireguard = connectMode = addressCheck = filesMode = backspace = encoding = gatewayMode = favorite = osIcon = KEEP;
		termType = rdpKdc = filesPort = wolBroadcast = wolPort = null;
		termTypeClear = rdpKdcClear = filesPortClear = wolBroadcastClear = wolPortClear = false;
		rdpSel = Object.fromEntries(rdpRows.map((r) => [r.key, KEEP]));
		gatewayHost = gatewayUser = gatewayPass = '';
		gatewayDetect = gatewayRpc = gatewayOwnLogin = false;
		tagsAdd = tagsRemove = notesText = '';
		notesOp = 'keep';
		phase = 'edit';
		error = '';
		report = null;
		themesOpen = false;
	}

	// A fresh form each time the dialog opens.
	$effect(() => {
		if (open) reset();
	});

	// ---- The selection -------------------------------------------------------
	let total = $derived(sessions.length);
	let vaults = $derived(vaultsOf(sessions));
	let oneVault = $derived(vaults.length === 1);
	let vaultCredentials = $derived(oneVault ? credentialsFor(credentials, vaults[0]) : []);
	let folderCount = $derived(new Set(sessions.map((s) => s.folder_id ?? '')).size);

	/** A typed password becomes a new saved credential, linked to them all. */
	let canCreateCredential = $derived(oneVault && countApplying('credential', sessions) > 0);
	let creatingCredential = $derived(!!password && !passwordClear && saveAsCredential && canCreateCredential);
	/** The credential choice as the patch will carry it ('__new__': the one
	 *  created on apply). */
	let credentialChange = $derived.by((): SessionPatch['credential'] => {
		if (creatingCredential) return { set: '__new__' };
		if (credential === KEEP) return undefined;
		if (credential === FOLDER) return 'clear';
		return { set: credential };
	});

	/** The login method the patch switches SSH sessions to, if any. */
	let authChange = $derived(authType === KEEP ? undefined : (authType as AuthMethod['type']));
	const count = (field: BulkField) => countApplying(field, sessions, credentialChange, authChange);
	const appliesText = (field: BulkField): string | undefined => {
		const n = count(field);
		return n < total ? t('bulk.applies', { count: String(n), total: String(total) }) : undefined;
	};
	const sshSessions = $derived(allSessions.filter((s) => (s.kind ?? 'ssh') === 'ssh'));

	function newHop(): Hop {
		return { host: '', port: '22', username: '', login: 'agent', password: '' };
	}

	/** Fill a hop from a saved SSH session's address and login. */
	function copyHop(hop: Hop, id: string): void {
		const from = allSessions.find((s) => s.id === id);
		if (!from) return;
		hop.host = from.host;
		hop.port = String(from.port || 22);
		hop.username = from.username;
		const cred = from.credential_id && from.credential_id !== OWN_LOGIN ? from.credential_id : '';
		hop.login = cred && vaultCredentials.some((c) => c.id === cred) ? cred : 'agent';
	}

	function hopConfig(h: Hop): JumpHostConfig {
		const cred = h.login !== 'agent' && h.login !== 'password' ? h.login : null;
		return {
			host: h.host.trim(),
			port: parseInt(h.port, 10) || 22,
			username: h.username.trim(),
			auth_method: h.login === 'password' ? { type: 'Password', password: h.password || undefined } : { type: 'Agent' },
			credential_id: cred
		};
	}

	let hopsValid = $derived(hops.length > 0 && hops.every((h) => h.host.trim() && parseInt(h.port, 10) > 0 && parseInt(h.port, 10) < 65536));
	let proxyValid = $derived(!!proxyHost.trim() && parseInt(proxyPort, 10) > 0 && parseInt(proxyPort, 10) < 65536);
	let keyMissing = $derived(authType === 'Key' && !keyId && !keyPath.trim());
	const shows = (field: BulkField) => sessions.some((s) => appliesTo(field, s));

	// ---- Shared values, for placeholders -------------------------------------
	let usernames = $derived(summarize(sessions.filter((s) => appliesTo('username', s)), (s) => s.username));
	let ports = $derived(summarize(sessions.filter((s) => appliesTo('port', s)), (s) => s.port));
	let domains = $derived(summarize(sessions.filter((s) => appliesTo('domain', s)), (s) => s.domain));
	let creds = $derived(
		summarize(sessions.filter((s) => appliesTo('credential', s)), (s) =>
			s.credential_id === OWN_LOGIN ? t('credential.own_login') : s.credential && !s.credential.missing ? s.credential.name : s.credential_id ? t('credential.missing') : ''
		)
	);
	let themes = $derived(
		summarize(sessions.filter((s) => appliesTo('terminal_theme', s)), (s) => (s.terminal_theme ? (findTerminalTheme(s.terminal_theme)?.name ?? t('termtheme.missing')) : ''))
	);
	let histories = $derived(summarize(sessions.filter((s) => appliesTo('history', s)), (s) => s.history ?? ''));
	let cursors = $derived(summarize(sessions.filter((s) => appliesTo('cursor_style', s)), (s) => s.cursor_style ?? ''));
	let blinks = $derived(
		summarize(sessions.filter((s) => appliesTo('cursor_blink', s)), (s) => (s.cursor_blink == null ? '' : s.cursor_blink ? 'on' : 'off'))
	);
	const cursorName = (v: string): string => (v ? t(`cursor.style_${v}`) : t('history.session_default'));
	const blinkName = (v: string): string => (v ? t(`cursor.blink_${v}`) : t('history.session_default'));
	const methodName = (m: string): string =>
		m === 'Key' ? t('session.auth_key') : m === 'Agent' ? t('session.auth_agent') : t('session.auth_password');
	let methods = $derived(summarize(sessions.filter((s) => appliesTo('auth', s)), (s) => methodName(s.auth_method?.type ?? 'Password')));
	let agentKeysNow = $derived(summarize(sessions.filter((s) => appliesTo('try_agent_keys', s)), (s) => (s.try_agent_keys ? 'on' : 'off')));
	let jumps = $derived(
		summarize(sessions.filter((s) => appliesTo('jump_chain', s)), (s) => (s.jump_chain ?? []).map((j) => j.host).join(' → '))
	);
	let proxies = $derived(
		summarize(sessions.filter((s) => appliesTo('proxy', s)), (s) => (s.proxy ? `${s.proxy.proxy_type.toUpperCase()} ${s.proxy.host}:${s.proxy.port}` : ''))
	);
	let shells = $derived(summarize(sessions.filter((s) => appliesTo('shell', s)), (s) => s.shell ?? ''));
	let vias = $derived(
		summarize(sessions.filter((s) => appliesTo('via_session', s)), (s) =>
			s.via_session_id ? (allSessions.find((x) => x.id === s.via_session_id)?.name ?? s.via_session_id) : ''
		)
	);
	const telnets = $derived(sessions.filter((s) => appliesTo('telnet', s)));
	const telnetLabel = (v: string): string =>
		({
			active: t('session.telnet_mode_active'),
			passive: t('session.telnet_mode_passive'),
			raw: t('session.telnet_mode_raw'),
			crlf: 'CR LF',
			cr: 'CR',
			lf: 'LF',
			del: 'DEL (^?)',
			ctrlh: 'Ctrl+H (^H)'
		})[v] ?? v;
	let telnetModes = $derived(summarize(telnets, (s) => telnetLabel(s.telnet?.mode ?? 'active')));
	let telnetNewlines = $derived(summarize(telnets, (s) => telnetLabel(s.telnet?.newline ?? 'crlf')));
	let telnetBackspaces = $derived(summarize(telnets, (s) => telnetLabel(s.telnet?.backspace ?? 'del')));
	let monitorings = $derived(
		summarize(sessions.filter((s) => appliesTo('monitoring', s)), (s) => (s.monitoring == null ? '' : s.monitoring ? 'on' : 'off'))
	);
	let tmuxModes = $derived(summarize(sessions.filter((s) => appliesTo('tmux', s)), (s) => s.tmux?.mode ?? 'inherit'));
	let tmuxMice = $derived(summarize(sessions.filter((s) => appliesTo('tmux', s)), (s) => (s.tmux?.mouse ? 'on' : 'off')));
	let fieldNames = $derived(summarize(sessions.flatMap((s) => (s.custom_fields ?? []).map((f) => ({ name: f.name }))), (x) => x.name));
	let withPassword = $derived(sessions.filter((s) => appliesTo('password', s) && hasOwnPassword(s)).length);
	let withNotes = $derived(sessions.filter((s) => s.notes?.trim()).length);
	let allTags = $derived(summarize(sessions.flatMap((s) => s.tags.map((tag) => ({ tag }))), (x) => x.tag));

	// ---- Phase 3: summaries and the RDP rows ----------------------------------
	/** What the sessions that take `field` have now, as `name` shows it. */
	const nowOf = (field: BulkField, name: (s: SessionConfig) => string) => summarize(sessions.filter((s) => appliesTo(field, s)), name);
	/** The Keep option's text: the shared value or Mixed. */
	const currentOf = (summary: ReturnType<typeof summarize>): string =>
		summary.counts.length === 0 ? '' : summary.same === null ? t('bulk.mixed') : summary.same;
	const onOffDefault = (v: boolean | null | undefined): string =>
		v == null ? t('history.session_default') : v ? t('session.monitoring_on') : t('session.monitoring_off');
	const wgProfiles = $derived(wireguardProfiles().map((p) => ({ id: p.id, name: p.name })));
	const wgName = (id: string | null | undefined): string =>
		!id ? t('wireguard.none') : (wgProfiles.find((p) => p.id === id)?.name ?? t('wireguard.missing'));
	let wireguards = $derived(nowOf('wireguard', (s) => wgName(s.wireguard_id)));
	const connectModeName = (m: string | null | undefined): string => t(`session.connect_mode_${m || 'auto'}`);
	let connectModes = $derived(nowOf('connect_mode', (s) => connectModeName(s.connect_mode)));
	let addressChecks = $derived(nowOf('address_check', (s) => onOffDefault(s.address_check)));
	const filesName = (f: string | null | undefined): string => (f ? t(`bulk.opens_${f}`) : t('bulk.opens_terminal'));
	let filesNow = $derived(nowOf('files', (s) => filesName(s.files)));
	let termTypes = $derived(nowOf('terminal', (s) => s.terminal?.term ?? ''));
	let backspaces = $derived(nowOf('terminal', (s) => (s.terminal?.backspace === 'ctrlh' ? 'Ctrl+H (^H)' : 'DEL (^?)')));
	let encodings = $derived(nowOf('terminal', (s) => s.terminal?.encoding ?? 'UTF-8'));
	let wolBroadcasts = $derived(nowOf('wol', (s) => s.wol?.broadcast ?? ''));
	let wolPorts = $derived(nowOf('wol', (s) => (s.wol?.port ? String(s.wol.port) : '')));
	let favorites = $derived(nowOf('favorite', (s) => (s.favorite ? t('bulk.favorite_on') : t('bulk.favorite_off'))));
	const osOptions = osChoices();
	const osTitle = (id: string): string => (id === 'unknown' ? t('session.os_none') : (osOptions.find((o) => o.id === id)?.title ?? id));
	/** Whether SSH sessions are among them: they can go back to Automatic. */
	let osAuto = $derived(sessions.some((s) => appliesTo('os_icon', s) && (s.kind ?? 'ssh') === 'ssh'));
	let osIcons = $derived(
		nowOf('os_icon', (s) => (s.os_manual ? osTitle(s.detected_os ?? 'unknown') : (s.kind ?? 'ssh') === 'ssh' ? t('session.os_auto') : s.detected_os ? osTitle(s.detected_os) : t('session.os_none')))
	);
	let osIconOptions = $derived([
		{ value: KEEP, label: t('bulk.keep_value', { value: currentOf(osIcons) }), osId: osIcons.same !== null ? (osOptions.find((o) => o.title === osIcons.same)?.id ?? null) : null },
		{ value: '', label: osAuto ? t('session.os_auto') : t('session.os_none'), osId: null },
		...(osAuto ? [{ value: 'unknown', label: t('session.os_none'), osId: null }] : []),
		...osOptions.map((o) => ({ value: o.id, label: o.title, osId: o.id }))
	]);

	interface RdpRow {
		key: keyof RdpPatch;
		label: string;
		options: { value: string; label: string }[];
		/** The session's value as one of `options`' labels. */
		now: (s: SessionConfig) => string;
		hint?: string;
	}
	const ON_OFF = () => [
		{ value: 'on', label: t('session.monitoring_on') },
		{ value: 'off', label: t('session.monitoring_off') }
	];
	const optLabel = (options: { value: string; label: string }[], v: string): string => options.find((o) => o.value === v)?.label ?? v;
	const intervalOptions = () => [{ value: '', label: t('session.rdp_interval_off') }, ...RDP_INTERVALS.map((v) => ({ value: v, label: rdpIntervalLabel(v) }))];
	/** The RDP settings that make sense on many desktops at once. '' is the
	 *  default, sent as a clear. */
	let rdpRows = $derived.by((): RdpRow[] => {
		const rows: (Omit<RdpRow, 'now'> & { get: (s: SessionConfig) => string })[] = [];
		const add = (key: keyof RdpPatch, label: string, options: { value: string; label: string }[], get: (s: SessionConfig) => string, hint?: string) =>
			rows.push({ key, label, options, get, hint });
		add('client', t('session.rdp_client'), [{ value: 'ironrdp', label: t('session.rdp_client_ironrdp') }, { value: 'freerdp', label: t('session.rdp_client_freerdp') }], (s) => (s.rdp_use_freerdp ? 'freerdp' : 'ironrdp'));
		add('clipboard', t('session.rdp_clipboard'), [{ value: '', label: t('session.rdp_clipboard_all') }, { value: 'text', label: t('session.rdp_clipboard_text') }, { value: 'off', label: t('session.rdp_clipboard_off') }], (s) => s.rdp_clipboard ?? '');
		add('forward_lock_keys', t('session.rdp_forward_lock_keys'), ON_OFF(), (s) => (s.rdp_forward_lock_keys === false ? 'off' : 'on'));
		add('resolution', t('session.rdp_resolution'), [{ value: '', label: t('session.rdp_resolution_fit') }, ...RDP_RESOLUTIONS.map((r) => ({ value: r, label: r.replace('x', ' × ') }))], (s) => s.rdp?.resolution ?? '');
		add('scale', t('session.rdp_scale'), [{ value: '', label: t('session.rdp_scale_auto') }, ...RDP_SCALES.map((v) => ({ value: v, label: `${v} %` }))], (s) => (s.rdp?.scale ? String(s.rdp.scale) : ''));
		add('color_depth', t('session.rdp_colors'), [{ value: '', label: t('session.rdp_colors_32') }, { value: '16', label: t('session.rdp_colors_16') }], (s) => (s.rdp?.color_depth === 16 ? '16' : ''));
		add('keyboard_layout', t('session.rdp_keyboard'), [{ value: '', label: t('session.rdp_keyboard_default') }, ...RDP_KEYBOARDS.map(([id, label]) => ({ value: id, label }))], (s) => (s.rdp?.keyboard_layout ? s.rdp.keyboard_layout.toString(16).toUpperCase().padStart(4, '0') : ''));
		add('sound', t('session.rdp_sound'), [{ value: '', label: t('session.rdp_sound_here') }, { value: 'off', label: t('session.rdp_sound_off') }], (s) => s.rdp?.sound ?? '');
		add('audio_quality', t('session.rdp_audio_quality'), [{ value: '', label: t('session.rdp_audio_high') }, { value: 'medium', label: t('session.rdp_audio_medium') }, { value: 'dynamic', label: t('session.rdp_audio_dynamic') }], (s) => (s.rdp?.audio_quality === 'high' ? '' : (s.rdp?.audio_quality ?? '')));
		add('performance', t('session.rdp_performance'), [{ value: '', label: t('session.rdp_performance_default') }, { value: 'fast', label: t('session.rdp_performance_fast') }, { value: 'best', label: t('session.rdp_performance_best') }], (s) => s.rdp?.performance ?? '');
		add('keepalive', t('session.rdp_keepalive'), intervalOptions(), (s) => (s.rdp?.keepalive ? String(s.rdp.keepalive) : ''));
		add('jiggler', t('session.rdp_jiggler'), intervalOptions(), (s) => (s.rdp?.jiggler ? String(s.rdp.jiggler) : ''));
		add('compression', t('session.rdp_compression'), ON_OFF(), (s) => (s.rdp?.compression === false ? 'off' : 'on'));
		add('admin', t('session.rdp_admin'), ON_OFF(), (s) => (s.rdp?.admin ? 'on' : 'off'));
		add('all_drives', t('session.rdp_share_all_drives'), ON_OFF(), (s) => (s.rdp?.all_drives ? 'on' : 'off'), t('session.rdp_share_all_drives_hint'));
		add('auth', t('session.rdp_auth'), [{ value: '', label: t('session.rdp_auth_ntlm') }, { value: 'kerberos', label: t('session.rdp_auth_kerberos') }], (s) => s.rdp?.auth ?? '');
		return rows.map((r) => ({ ...r, now: (s: SessionConfig) => optLabel(r.options, r.get(s)) }));
	});
	let rdpSessions = $derived(sessions.filter((s) => appliesTo('rdp', s)));
	const rdpNow = (row: RdpRow) => summarize(rdpSessions, row.now);
	let gateways = $derived(summarize(rdpSessions, (s) => s.rdp?.gateway ?? ''));
	let kdcs = $derived(summarize(rdpSessions, (s) => s.rdp?.kdc ?? ''));
	let filesPorts = $derived(summarize(rdpSessions, (s) => String(s.rdp?.files_port ?? 22)));
	let kerberosChosen = $derived(rdpSel.auth === 'kerberos' || (rdpSel.auth === KEEP && rdpSessions.some((s) => s.rdp?.auth === 'kerberos')));

	/** A row's choice as the patch carries it. */
	function rdpValue(key: keyof RdpPatch, v: string): RdpPatch[keyof RdpPatch] {
		switch (key) {
			case 'client':
				return v as 'ironrdp' | 'freerdp';
			case 'forward_lock_keys':
			case 'compression':
			case 'admin':
			case 'all_drives':
				return v === 'on';
			case 'scale':
			case 'color_depth':
			case 'keepalive':
			case 'jiggler':
				return v === '' ? 'clear' : { set: Number(v) };
			case 'keyboard_layout':
				return v === '' ? 'clear' : { set: parseInt(v, 16) };
			default:
				return (v === '' ? 'clear' : { set: v }) as RdpPatch[keyof RdpPatch];
		}
	}

	const validPort = (v: string | null): boolean => v !== null && /^\d+$/.test(v.trim()) && Number(v) > 0 && Number(v) < 65536;
	let gatewayValid = $derived(/^(https:\/\/)?[A-Za-z0-9.-]+(:\d{1,5})?\/?$/.test(gatewayHost.trim()));
	let rdpPortInvalid = $derived(!filesPortClear && !!filesPort?.trim() && !validPort(filesPort));
	let wolPortInvalid = $derived(!wolPortClear && !!wolPort?.trim() && !validPort(wolPort));
	let termTypeInvalid = $derived(!termTypeClear && !!termType?.trim() && /\s/.test(termType.trim()));

	function placeholderOf(summary: ReturnType<typeof summarize>, blank: string): string {
		if (summary.counts.length === 0) return '';
		return summary.same !== null ? summary.same || blank : t('bulk.mixed');
	}

	function tooltipOf(summary: ReturnType<typeof summarize>, blank: string): string {
		return summary.same === null ? describeCounts(summary, blank) : '';
	}

	// ---- The patch ------------------------------------------------------------
	let newUsername = $derived(username !== null ? username.trim() : null);
	let credentialUsername = $derived(newUsername || usernames.same || '');

	let patch = $derived.by((): SessionPatch => {
		const p: SessionPatch = {};
		if (credentialChange !== undefined) p.credential = credentialChange;
		if (newUsername !== null && newUsername !== '') p.username = newUsername;
		if (passwordClear) p.password = 'clear';
		else if (password) p.password = { set: password };
		const n = port === null ? NaN : parseInt(port, 10);
		if (n > 0 && n < 65536) p.port = n;
		if (domainClear) p.domain = 'clear';
		else if (domain !== null && domain.trim()) p.domain = { set: domain.trim() };
		const add = parseTags(tagsAdd);
		const remove = parseTags(tagsRemove);
		if (add.length) p.tags_add = add;
		if (remove.length) p.tags_remove = remove;
		if (notesOp === 'clear') p.notes = { op: 'clear' };
		else if (notesOp !== 'keep' && notesText.trim()) p.notes = { op: notesOp, text: notesText };
		if (theme !== null) p.terminal_theme = theme ? { set: theme } : 'clear';
		if (history !== KEEP) p.history = history ? { set: history as HistoryMode } : 'clear';
		if (cursor !== KEEP) p.cursor_style = cursor ? { set: cursor as CursorStyle } : 'clear';
		if (blink !== KEEP) p.cursor_blink = blink ? { set: blink === 'on' } : 'clear';
		if (authType === 'Key') {
			p.auth = {
				type: 'Key',
				key_id: keyId || undefined,
				path: keyId ? undefined : keyPath.trim() || undefined,
				passphrase: keyPassphrase || undefined
			};
		} else if (authType !== KEEP) {
			p.auth = { type: authType as 'Password' | 'Agent' };
		}
		if (agentKeys !== KEEP) p.try_agent_keys = agentKeys === 'on';
		if (jumpMode === 'clear') p.jump_chain = 'clear';
		else if (jumpMode === 'set' && hopsValid) p.jump_chain = { set: hops.map(hopConfig) };
		if (proxyMode === 'clear') p.proxy = 'clear';
		else if (proxyMode === 'set' && proxyValid) {
			p.proxy = {
				set: {
					proxy_type: proxyType,
					host: proxyHost.trim(),
					port: parseInt(proxyPort, 10),
					username: proxyType === 'socks4' ? null : proxyUser.trim() || null,
					password: proxyType === 'socks4' ? null : proxyPass || null
				}
			};
		}
		if (shellClear) p.shell = 'clear';
		else if (shell?.trim()) p.shell = { set: shell.trim() };
		const set = fieldsSet.filter((f) => f.name.trim()).map((f) => ({ name: f.name.trim(), value: f.value, hidden: f.hidden }));
		if (set.length) p.fields_set = set;
		const removeFields = fieldsRemove.split(',').map((x) => x.trim()).filter(Boolean);
		if (removeFields.length) p.fields_remove = removeFields;
		const telnet: NonNullable<SessionPatch['telnet']> = {};
		if (telnetMode !== KEEP) telnet.mode = telnetMode as 'active' | 'passive' | 'raw';
		if (telnetNewline !== KEEP) telnet.newline = telnetNewline as 'crlf' | 'cr' | 'lf';
		if (telnetBackspace !== KEEP) telnet.backspace = telnetBackspace as 'del' | 'ctrlh';
		if (Object.keys(telnet).length) p.telnet = telnet;
		if (monitoring !== KEEP) p.monitoring = monitoring ? { set: monitoring === 'on' } : 'clear';
		const tmux: NonNullable<SessionPatch['tmux']> = {};
		if (tmuxMode !== KEEP) tmux.mode = tmuxMode as 'off' | 'attach' | 'ask' | 'inherit';
		if (tmuxMouse !== KEEP) tmux.mouse = tmuxMouse === 'on';
		if (Object.keys(tmux).length) p.tmux = tmux;
		if (via !== KEEP) p.via_session = via ? { set: via } : 'clear';
		if (wireguard !== KEEP) p.wireguard = wireguard ? { set: wireguard } : 'clear';
		if (connectMode !== KEEP) p.connect_mode = connectMode as 'auto' | 'ask' | 'main';
		if (addressCheck !== KEEP) p.address_check = addressCheck ? { set: addressCheck === 'on' } : 'clear';
		if (filesMode !== KEEP) p.files = filesMode ? { set: filesMode as 'sftp' | 'scp' } : 'clear';
		const term: NonNullable<SessionPatch['terminal']> = {};
		if (termTypeClear) term.term = 'clear';
		else if (termType?.trim() && !termTypeInvalid) term.term = { set: termType.trim() };
		if (backspace !== KEEP) term.backspace = backspace === 'ctrlh' ? { set: 'ctrlh' } : 'clear';
		if (encoding !== KEEP) term.encoding = encoding && encoding !== 'UTF-8' ? { set: encoding } : 'clear';
		if (Object.keys(term).length) p.terminal = term;
		const rdp: RdpPatch = {};
		for (const row of rdpRows) {
			const v = rdpSel[row.key];
			if (v !== undefined && v !== KEEP) (rdp as Record<string, unknown>)[row.key] = rdpValue(row.key, v);
		}
		if (rdpKdcClear) rdp.kdc = 'clear';
		else if (rdpKdc?.trim()) rdp.kdc = { set: rdpKdc.trim() };
		if (gatewayMode === 'clear') rdp.gateway = 'clear';
		else if (gatewayMode === 'set' && gatewayValid) {
			rdp.gateway = {
				set: {
					host: gatewayHost.trim(),
					detect: gatewayDetect,
					rpc: gatewayRpc,
					user: gatewayOwnLogin ? gatewayUser.trim() || null : null,
					password: gatewayOwnLogin ? gatewayPass || null : null
				}
			};
		}
		if (filesPortClear) rdp.files_port = 'clear';
		else if (validPort(filesPort)) rdp.files_port = { set: Number(filesPort) };
		if (Object.keys(rdp).length) p.rdp = rdp;
		const wol: NonNullable<SessionPatch['wol']> = {};
		if (wolBroadcastClear) wol.broadcast = 'clear';
		else if (wolBroadcast?.trim()) wol.broadcast = { set: wolBroadcast.trim() };
		if (wolPortClear) wol.port = 'clear';
		else if (validPort(wolPort)) wol.port = { set: Number(wolPort) };
		if (Object.keys(wol).length) p.wol = wol;
		if (favorite !== KEEP) p.favorite = favorite === 'on';
		if (osIcon !== KEEP) p.os_icon = osIcon ? { set: osIcon } : 'clear';
		return p;
	});
	let fields = $derived(changedFields(patch));
	/** The confirm list: a username and password that only go into the new
	 *  credential are shown as part of it. */
	let shownFields = $derived(
		fields.filter((f) => !(creatingCredential && (f === 'username' || f === 'password') && count(f) === 0))
	);
	let portInvalid = $derived(port !== null && port.trim() !== '' && !(parseInt(port, 10) > 0 && parseInt(port, 10) < 65536));
	let credentialNeedsName = $derived(creatingCredential && (!credentialName.trim() || !credentialUsername));
	let incomplete = $derived(
		keyMissing ||
			(jumpMode === 'set' && !hopsValid) ||
			(proxyMode === 'set' && !proxyValid) ||
			(gatewayMode === 'set' && !gatewayValid) ||
			rdpPortInvalid ||
			wolPortInvalid ||
			termTypeInvalid
	);
	let canApply = $derived(fields.length > 0 && !portInvalid && !credentialNeedsName && !incomplete);

	// ---- Confirm --------------------------------------------------------------
	const fieldLabel = (f: BulkField): string =>
		({
			username: t('session.username'),
			password: t('session.password'),
			credential: t('credential.login'),
			port: t('session.port'),
			domain: t('bulk.domain'),
			tags: t('bulk.tags'),
			notes: t('session.notes'),
			terminal_theme: t('termtheme.session_title'),
			history: t('history.session_label'),
			cursor_style: t('cursor.session_label'),
			cursor_blink: t('cursor.blink'),
			auth: t('session.auth_method'),
			try_agent_keys: t('bulk.agent_keys'),
			jump_chain: t('bulk.jump_hosts'),
			proxy: t('bulk.proxy'),
			shell: t('bulk.shell'),
			custom_fields: t('session.custom_fields'),
			telnet: t('session.protocol_telnet'),
			monitoring: t('session.monitoring'),
			tmux: t('tmux.session_label'),
			via_session: t('session.vnc_via'),
			wireguard: t('wireguard.session_route'),
			connect_mode: t('session.connect_mode'),
			address_check: t('session.address_check'),
			files: t('bulk.opens'),
			terminal: t('bulk.ssh_terminal'),
			rdp: t('session.protocol_rdp'),
			wol: t('session.wol'),
			favorite: t('session.favorite'),
			os_icon: t('session.os_icon')
		})[f];

	const onOff = (on: boolean | undefined): string => (on ? t('session.monitoring_on') : t('session.monitoring_off'));
	const tmuxModeName = (m: string | undefined): string =>
		m === 'inherit' ? t('bulk.tmux_folder') : m ? t(`tmux.mode_${m}`) : '';

	function describeChange(f: BulkField): string {
		const p = patch;
		switch (f) {
			case 'username':
				return p.username ?? '';
			case 'password':
				return p.password === 'clear' ? t('bulk.cleared') : t('bulk.new_password');
			case 'credential':
				if (creatingCredential) return `${t('bulk.new_credential', { name: credentialName.trim() })} (${credentialUsername})`;
				if (p.credential === 'clear') return t('bulk.folder_credential');
				if (p.credential?.set === OWN_LOGIN) return t('credential.own_login');
				return vaultCredentials.find((c) => c.id === (p.credential as { set: string } | undefined)?.set)?.name ?? '';
			case 'port':
				return String(p.port);
			case 'domain':
				return p.domain === 'clear' ? t('bulk.cleared') : (p.domain?.set ?? '');
			case 'tags':
				return [...(p.tags_add ?? []).map((x) => `+${x}`), ...(p.tags_remove ?? []).map((x) => `−${x}`)].join(', ');
			case 'notes':
				return p.notes?.op === 'clear' ? t('bulk.cleared') : p.notes?.op === 'append' ? t('bulk.notes_appended') : t('bulk.notes_replaced');
			case 'terminal_theme':
				return p.terminal_theme === 'clear' ? t('termtheme.session_default') : (findTerminalTheme(p.terminal_theme?.set)?.name ?? '');
			case 'history':
				return p.history === 'clear' ? t('history.session_default') : t(`history.mode_${p.history?.set}`);
			case 'cursor_style':
				return p.cursor_style === 'clear' ? t('history.session_default') : cursorName(p.cursor_style?.set ?? '');
			case 'cursor_blink':
				return p.cursor_blink === 'clear' ? t('history.session_default') : blinkName(p.cursor_blink?.set ? 'on' : 'off');
			case 'auth':
				return p.auth?.type === 'Key' ? t('session.auth_key') : p.auth?.type === 'Agent' ? t('session.auth_agent') : t('session.auth_password');
			case 'try_agent_keys':
				return onOff(p.try_agent_keys);
			case 'jump_chain':
				return p.jump_chain === 'clear' ? t('bulk.jump_none') : (p.jump_chain?.set ?? []).map((j) => `${j.username ? j.username + '@' : ''}${j.host}`).join(' → ');
			case 'proxy':
				return p.proxy === 'clear' ? t('bulk.proxy_none') : `${p.proxy?.set.proxy_type.toUpperCase()} ${p.proxy?.set.host}:${p.proxy?.set.port}`;
			case 'shell':
				return p.shell === 'clear' ? t('bulk.shell_default') : (p.shell?.set ?? '');
			case 'custom_fields':
				return [...(p.fields_set ?? []).map((x) => `+${x.name}`), ...(p.fields_remove ?? []).map((x) => `−${x}`)].join(', ');
			case 'telnet':
				return Object.values(p.telnet ?? {}).map((v) => telnetLabel(String(v))).join(', ');
			case 'monitoring':
				return p.monitoring === 'clear' ? t('history.session_default') : onOff(p.monitoring?.set);
			case 'tmux':
				return [p.tmux?.mode ? tmuxModeName(p.tmux.mode) : '', p.tmux?.mouse !== undefined ? `${t('tmux.mouse')}: ${onOff(p.tmux.mouse)}` : '']
					.filter(Boolean)
					.join(', ');
			case 'via_session':
				return p.via_session === 'clear' ? t('session.vnc_via_none') : (allSessions.find((x) => p.via_session !== 'clear' && x.id === p.via_session?.set)?.name ?? '');
			case 'wireguard':
				return wgName(p.wireguard === 'clear' ? null : p.wireguard?.set);
			case 'connect_mode':
				return connectModeName(p.connect_mode);
			case 'address_check':
				return onOffDefault(p.address_check === 'clear' ? null : p.address_check?.set);
			case 'files':
				return filesName(p.files === 'clear' ? null : p.files?.set);
			case 'terminal': {
				const tp = p.terminal ?? {};
				return [
					tp.term ? `${t('session.term_type')}: ${tp.term === 'clear' ? 'xterm-256color' : tp.term.set}` : '',
					tp.backspace ? `${t('session.telnet_backspace')}: ${tp.backspace === 'clear' ? 'DEL' : 'Ctrl+H'}` : '',
					tp.encoding ? `${t('session.encoding')}: ${tp.encoding === 'clear' ? 'UTF-8' : tp.encoding.set}` : ''
				]
					.filter(Boolean)
					.join(', ');
			}
			case 'rdp': {
				const parts = rdpRows.filter((r) => rdpSel[r.key] !== undefined && rdpSel[r.key] !== KEEP).map((r) => `${r.label}: ${optLabel(r.options, rdpSel[r.key])}`);
				if (p.rdp?.kdc) parts.push(`${t('session.rdp_kdc')}: ${p.rdp.kdc === 'clear' ? t('bulk.cleared') : p.rdp.kdc.set}`);
				if (p.rdp?.gateway) parts.push(`${t('bulk.gateway')}: ${p.rdp.gateway === 'clear' ? t('bulk.gateway_none') : p.rdp.gateway.set.host}`);
				if (p.rdp?.files_port) parts.push(`${t('session.rdp_files_port')}: ${p.rdp.files_port === 'clear' ? '22' : p.rdp.files_port.set}`);
				return parts.join(', ');
			}
			case 'wol':
				return [
					p.wol?.broadcast ? `${t('net.broadcast')}: ${p.wol.broadcast === 'clear' ? '255.255.255.255' : p.wol.broadcast.set}` : '',
					p.wol?.port ? `${t('net.udp_port')}: ${p.wol.port === 'clear' ? '9' : p.wol.port.set}` : ''
				]
					.filter(Boolean)
					.join(', ');
			case 'favorite':
				return p.favorite ? t('bulk.favorite_on') : t('bulk.favorite_off');
			case 'os_icon':
				return p.os_icon === 'clear' ? (osAuto ? t('session.os_auto') : t('session.os_none')) : osTitle(p.os_icon?.set ?? '');
		}
	}

	async function apply(ids: string[] = sessions.map((s) => s.id)): Promise<void> {
		phase = 'saving';
		error = '';
		try {
			const send = { ...patch };
			if (creatingCredential) {
				const created = await savedCredentialCreate({
					name: credentialName.trim(),
					username: credentialUsername,
					domain: patch.domain && patch.domain !== 'clear' ? patch.domain.set : (domains.same ?? null),
					authMethod: { type: 'Password', password: password ?? '' },
					vaultId: vaults[0]
				});
				send.credential = { set: created.id };
				// Linked once; a retry uses the same credential.
				credential = created.id;
				saveAsCredential = false;
			}
			const result = await sessionBulkUpdate(ids, send);
			onapplied(result);
			const failed = result.results.filter((r) => r.status === 'failed');
			if (failed.length > 0) {
				report = result;
				phase = 'report';
			} else {
				onclose();
			}
		} catch (e) {
			error = String(e);
			phase = 'confirm';
		}
	}

	let failedIds = $derived(report?.results.filter((r) => r.status === 'failed').map((r) => r.id) ?? []);
</script>

<Modal {open} {onclose} title={t('bulk.title', { count: String(total) })} maxWidth="560px">
	{#if phase === 'edit'}
		<div class="bulk">
			<p class="intro">
				{folderCount > 1 ? t('bulk.intro_folders', { count: String(total), folders: String(folderCount) }) : t('bulk.intro', { count: String(total) })}
				{t('bulk.intro_keep')}
			</p>

			<h3 class="section">{t('credential.login')}</h3>

			{#if shows('credential')}
				<div class="row" class:changed={credential !== KEEP || creatingCredential}>
					<label class="label" for="bulk-credential">{t('credential.login')}</label>
					<div class="field">
						<select
							id="bulk-credential"
							bind:value={credential}
							disabled={!oneVault || creatingCredential}
							title={tooltipOf(creds, t('bulk.folder_credential'))}
						>
							<option value={KEEP}>{t('bulk.keep_value', { value: placeholderOf(creds, t('bulk.folder_credential')) })}</option>
							<option value={FOLDER}>{t('bulk.folder_credential')}</option>
							<option value={OWN_LOGIN}>{t('credential.own_login')}</option>
							{#each vaultCredentials as c (c.id)}
								<option value={c.id}>{c.name} ({c.username})</option>
							{/each}
						</select>
						{#if creatingCredential}
							<p class="hint">{t('bulk.new_credential', { name: credentialName.trim() || '…' })}</p>
						{:else if !oneVault}
							<p class="hint">{t('bulk.one_vault_only')}</p>
						{/if}
						<p class="applies">{t('bulk.applies', { count: String(count('credential')), total: String(total) })}</p>
					</div>
					{#if credential !== KEEP}
						<button class="revert" type="button" onclick={() => (credential = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('username')}
				<div class="row" class:changed={username !== null && username.trim() !== ''}>
					<label class="label" for="bulk-username">{t('session.username')}</label>
					<div class="field">
						<input
							id="bulk-username"
							type="text"
							autocomplete="off"
							spellcheck="false"
							value={username ?? ''}
							oninput={(e) => (username = e.currentTarget.value)}
							placeholder={placeholderOf(usernames, t('bulk.none'))}
							title={tooltipOf(usernames, t('bulk.none'))}
						/>
						{#if creatingCredential}
							<p class="applies">{t('bulk.into_credential', { count: String(countApplying('credential', sessions)), total: String(total) })}</p>
						{:else}
							<p class="applies">{t('bulk.applies', { count: String(count('username')), total: String(total) })}</p>
						{/if}
					</div>
					{#if username !== null && username !== ''}
						<button class="revert" type="button" onclick={() => (username = null)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('password')}
				<div class="row" class:changed={passwordClear || !!password}>
					<label class="label" for="bulk-password">{t('session.password')}</label>
					<div class="field">
						{#if passwordClear}
							<p class="cleared">{t('bulk.will_clear')}</p>
						{:else}
							<PasswordField>
								{#snippet children(type)}
									<input
										id="bulk-password"
										{type}
										autocomplete="new-password"
										value={password ?? ''}
										oninput={(e) => (password = e.currentTarget.value)}
										placeholder={t('bulk.password_set_on', { count: String(withPassword), total: String(count('password')) })}
									/>
								{/snippet}
							</PasswordField>
						{/if}
						{#if password && !passwordClear && canCreateCredential}
							<label class="check">
								<input type="checkbox" bind:checked={saveAsCredential} />
								<span>{t('bulk.save_as_credential')}</span>
							</label>
							{#if saveAsCredential}
								<input
									class="sub-input"
									type="text"
									value={credentialName}
									oninput={(e) => (credentialName = e.currentTarget.value)}
									placeholder={t('bulk.credential_name')}
									aria-label={t('bulk.credential_name')}
								/>
								{#if !credentialUsername}
									<p class="hint warn">{t('bulk.credential_needs_username')}</p>
								{:else}
									<p class="hint">{t('bulk.credential_hint', { username: credentialUsername })}</p>
								{/if}
							{/if}
						{/if}
						{#if !creatingCredential}
							<p class="applies">{t('bulk.applies', { count: String(count('password')), total: String(total) })}</p>
						{/if}
					</div>
					{#if passwordClear || password}
						<button class="revert" type="button" onclick={() => { password = null; passwordClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{:else}
						<button class="clear" type="button" onclick={() => (passwordClear = true)}>{t('bulk.clear')}</button>
					{/if}
				</div>
			{/if}

			{#if shows('auth')}
				<div class="row" class:changed={authType !== KEEP}>
					<label class="label" for="bulk-auth">{t('session.auth_method')}</label>
					<div class="field">
						<select id="bulk-auth" bind:value={authType} title={tooltipOf(methods, '')}>
							<option value={KEEP}>{t('bulk.keep_value', { value: placeholderOf(methods, '') })}</option>
							<option value="Password">{t('session.auth_password')}</option>
							<option value="Key">{t('session.auth_key')}</option>
							<option value="Agent">{t('session.auth_agent')}</option>
						</select>
						{#if authType === 'Key'}
							<KeyPicker bind:path={keyPath} bind:keyId />
							<PasswordField>
								{#snippet children(type)}
									<input
										class="sub-input"
										{type}
										autocomplete="new-password"
										bind:value={keyPassphrase}
										placeholder={t('session.passphrase_optional')}
										aria-label={t('session.passphrase_optional')}
									/>
								{/snippet}
							</PasswordField>
							{#if keyMissing}<p class="hint warn">{t('bulk.key_needed')}</p>{/if}
						{:else if authType === 'Password'}
							<p class="hint">{t('bulk.auth_password_hint')}</p>
						{/if}
						{#if appliesText('auth')}<p class="applies">{appliesText('auth')}</p>{/if}
					</div>
					{#if authType !== KEEP}
						<button class="revert" type="button" onclick={() => (authType = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if sessions.some((s) => appliesTo('try_agent_keys', s, undefined, authChange))}
				<BulkSelectRow
					id="bulk-agent-keys"
					label={t('bulk.agent_keys')}
					bind:value={agentKeys}
					keep={KEEP}
					current={agentKeysNow.same === null ? t('bulk.mixed') : onOff(agentKeysNow.same === 'on')}
					options={[{ value: 'on', label: t('session.monitoring_on') }, { value: 'off', label: t('session.monitoring_off') }]}
					hint={t('session.try_agent_keys')}
					applies={appliesText('try_agent_keys')}
				/>
			{/if}

			{#if shows('port')}
				<div class="row" class:changed={port !== null && port.trim() !== ''}>
					<label class="label" for="bulk-port">{t('session.port')}</label>
					<div class="field">
						<input
							id="bulk-port"
							type="text"
							inputmode="numeric"
							class:invalid={portInvalid}
							value={port ?? ''}
							oninput={(e) => (port = e.currentTarget.value)}
							placeholder={placeholderOf(ports, t('bulk.none'))}
							title={tooltipOf(ports, t('bulk.none'))}
						/>
						<p class="applies">{t('bulk.applies', { count: String(count('port')), total: String(total) })}</p>
					</div>
					{#if port}
						<button class="revert" type="button" onclick={() => (port = null)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('domain')}
				<div class="row" class:changed={domainClear || !!domain?.trim()}>
					<label class="label" for="bulk-domain">{t('bulk.domain')}</label>
					<div class="field">
						{#if domainClear}
							<p class="cleared">{t('bulk.will_clear')}</p>
						{:else}
							<input
								id="bulk-domain"
								type="text"
								autocomplete="off"
								value={domain ?? ''}
								oninput={(e) => (domain = e.currentTarget.value)}
								placeholder={placeholderOf(domains, t('bulk.none'))}
								title={tooltipOf(domains, t('bulk.none'))}
							/>
						{/if}
						<p class="applies">{t('bulk.applies', { count: String(count('domain')), total: String(total) })}</p>
					</div>
					{#if domainClear || domain}
						<button class="revert" type="button" onclick={() => { domain = null; domainClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{:else}
						<button class="clear" type="button" onclick={() => (domainClear = true)}>{t('bulk.clear')}</button>
					{/if}
				</div>
			{/if}

			{#if shows('jump_chain') || shows('via_session') || shows('wireguard')}
				<h3 class="section">{t('bulk.section_connection')}</h3>
			{/if}

			{#if shows('jump_chain')}
				<div class="row" class:changed={jumpMode !== KEEP}>
					<label class="label" for="bulk-jump">{t('bulk.jump_hosts')}</label>
					<div class="field">
						<select id="bulk-jump" bind:value={jumpMode} title={tooltipOf(jumps, t('bulk.jump_none'))}>
							<option value={KEEP}>{t('bulk.keep_value', { value: placeholderOf(jumps, t('bulk.jump_none')) })}</option>
							<option value="set">{t('bulk.jump_replace')}</option>
							<option value="clear">{t('bulk.jump_none')}</option>
						</select>
						{#if jumpMode === 'set'}
							{#each hops as hop, i (i)}
								<div class="hop">
									<div class="hop-head">
										<span>{t('session.jump_hop', { n: String(i + 1) })}</span>
										{#if hops.length > 1}
											<button class="link" type="button" onclick={() => (hops = hops.filter((_, j) => j !== i))}>{t('session.jump_remove_hop')}</button>
										{/if}
									</div>
									{#if sshSessions.length > 0}
										<select aria-label={t('bulk.hop_copy')} value="" onchange={(e) => { copyHop(hop, e.currentTarget.value); e.currentTarget.value = ''; }}>
											<option value="">{t('bulk.hop_copy')}</option>
											{#each sshSessions as s (s.id)}
												<option value={s.id}>{s.name} ({s.username ? s.username + '@' : ''}{s.host})</option>
											{/each}
										</select>
									{/if}
									<div class="hop-grid">
										<input type="text" bind:value={hop.host} placeholder={t('session.host')} aria-label={t('session.host')} spellcheck="false" />
										<input type="text" inputmode="numeric" bind:value={hop.port} placeholder="22" aria-label={t('session.port')} />
									</div>
									<input type="text" bind:value={hop.username} placeholder={t('session.username')} aria-label={t('session.username')} spellcheck="false" />
									<select bind:value={hop.login} aria-label={t('credential.login')}>
										<option value="agent">{t('session.auth_agent')}</option>
										<option value="password">{t('session.auth_password')}</option>
										{#each vaultCredentials as c (c.id)}
											<option value={c.id}>{c.name} ({c.username})</option>
										{/each}
									</select>
									{#if hop.login === 'password'}
										<PasswordField>
											{#snippet children(type)}
												<input {type} autocomplete="new-password" bind:value={hop.password} placeholder={t('session.password_optional')} aria-label={t('session.password')} />
											{/snippet}
										</PasswordField>
									{/if}
								</div>
							{/each}
							<button class="link" type="button" onclick={() => (hops = [...hops, newHop()])}>+ {t('session.jump_add_hop')}</button>
						{/if}
						{#if appliesText('jump_chain')}<p class="applies">{appliesText('jump_chain')}</p>{/if}
					</div>
					{#if jumpMode !== KEEP}
						<button class="revert" type="button" onclick={() => (jumpMode = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('proxy')}
				<div class="row" class:changed={proxyMode !== KEEP}>
					<label class="label" for="bulk-proxy">{t('bulk.proxy')}</label>
					<div class="field">
						<select id="bulk-proxy" bind:value={proxyMode} title={tooltipOf(proxies, t('bulk.proxy_none'))}>
							<option value={KEEP}>{t('bulk.keep_value', { value: placeholderOf(proxies, t('bulk.proxy_none')) })}</option>
							<option value="set">{t('bulk.proxy_set')}</option>
							<option value="clear">{t('bulk.proxy_none')}</option>
						</select>
						{#if proxyMode === 'set'}
							<select bind:value={proxyType} aria-label={t('bulk.proxy')}>
								<option value="socks5">SOCKS5</option>
								<option value="socks4">SOCKS4</option>
								<option value="http">HTTP CONNECT</option>
							</select>
							<div class="hop-grid">
								<input type="text" bind:value={proxyHost} placeholder={t('proxy.host')} aria-label={t('proxy.host')} spellcheck="false" />
								<input type="text" inputmode="numeric" bind:value={proxyPort} placeholder="9050" aria-label={t('session.port')} />
							</div>
							{#if proxyType !== 'socks4'}
								<input type="text" bind:value={proxyUser} placeholder={t('proxy.username_optional')} aria-label={t('proxy.username_optional')} spellcheck="false" />
								<PasswordField>
									{#snippet children(type)}
										<input {type} autocomplete="new-password" bind:value={proxyPass} placeholder={t('session.password_optional')} aria-label={t('session.password')} />
									{/snippet}
								</PasswordField>
							{/if}
						{/if}
						{#if appliesText('proxy')}<p class="applies">{appliesText('proxy')}</p>{/if}
					</div>
					{#if proxyMode !== KEEP}
						<button class="revert" type="button" onclick={() => (proxyMode = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('shell')}
				<div class="row" class:changed={shellClear || !!shell?.trim()}>
					<label class="label" for="bulk-shell">{t('bulk.shell')}</label>
					<div class="field">
						{#if shellClear}
							<p class="cleared">{t('bulk.shell_default')}</p>
						{:else}
							<input
								id="bulk-shell"
								type="text"
								autocomplete="off"
								spellcheck="false"
								value={shell ?? ''}
								oninput={(e) => (shell = e.currentTarget.value)}
								placeholder={placeholderOf(shells, t('bulk.shell_default'))}
								title={tooltipOf(shells, t('bulk.shell_default'))}
							/>
						{/if}
						{#if appliesText('shell')}<p class="applies">{appliesText('shell')}</p>{/if}
					</div>
					{#if shellClear || shell}
						<button class="revert" type="button" onclick={() => { shell = null; shellClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{:else}
						<button class="clear" type="button" onclick={() => (shellClear = true)}>{t('bulk.clear')}</button>
					{/if}
				</div>
			{/if}

			{#if shows('via_session')}
				<BulkSelectRow
					id="bulk-via"
					label={t('session.vnc_via')}
					bind:value={via}
					keep={KEEP}
					current={placeholderOf(vias, t('session.vnc_via_none'))}
					tooltip={tooltipOf(vias, t('session.vnc_via_none'))}
					options={[{ value: '', label: t('session.vnc_via_none') }, ...sshSessions.map((s) => ({ value: s.id, label: s.name }))]}
					applies={appliesText('via_session')}
				/>
			{/if}

			{#if shows('wireguard') && (wgProfiles.length > 0 || sessions.some((s) => s.wireguard_id))}
				<BulkSelectRow
					id="bulk-wireguard"
					label={t('wireguard.session_route')}
					bind:value={wireguard}
					keep={KEEP}
					current={currentOf(wireguards)}
					tooltip={tooltipOf(wireguards, '')}
					options={[{ value: '', label: t('wireguard.none') }, ...wgProfiles.map((w) => ({ value: w.id, label: w.name }))]}
					applies={appliesText('wireguard')}
				/>
			{/if}

			{#if shows('connect_mode')}
				<BulkSelectRow
					id="bulk-connect-mode"
					label={t('session.connect_mode')}
					bind:value={connectMode}
					keep={KEEP}
					current={currentOf(connectModes)}
					tooltip={tooltipOf(connectModes, '')}
					options={[
						{ value: 'auto', label: t('session.connect_mode_auto') },
						{ value: 'ask', label: t('session.connect_mode_ask') },
						{ value: 'main', label: t('session.connect_mode_main') }
					]}
					hint={t('bulk.addresses_only')}
					applies={appliesText('connect_mode')}
				/>
				<BulkSelectRow
					id="bulk-address-check"
					label={t('session.address_check')}
					bind:value={addressCheck}
					keep={KEEP}
					current={currentOf(addressChecks)}
					tooltip={tooltipOf(addressChecks, '')}
					options={[
						{ value: '', label: t('history.session_default') },
						{ value: 'on', label: t('session.address_check_on') },
						{ value: 'off', label: t('session.address_check_off') }
					]}
				/>
			{/if}

			{#if shows('files')}
				<BulkSelectRow
					id="bulk-files"
					label={t('bulk.opens')}
					bind:value={filesMode}
					keep={KEEP}
					current={currentOf(filesNow)}
					tooltip={tooltipOf(filesNow, '')}
					options={[
						{ value: '', label: t('bulk.opens_terminal') },
						{ value: 'sftp', label: t('bulk.opens_sftp') },
						{ value: 'scp', label: t('bulk.opens_scp') }
					]}
					applies={appliesText('files')}
				/>
			{/if}

			{#if shows('telnet')}
				<h3 class="section">{t('session.protocol_telnet')}</h3>
				<BulkSelectRow
					id="bulk-telnet-mode"
					label={t('session.telnet_mode')}
					bind:value={telnetMode}
					keep={KEEP}
					current={placeholderOf(telnetModes, '')}
					tooltip={tooltipOf(telnetModes, '')}
					options={[
						{ value: 'active', label: t('session.telnet_mode_active') },
						{ value: 'passive', label: t('session.telnet_mode_passive') },
						{ value: 'raw', label: t('session.telnet_mode_raw') }
					]}
					applies={appliesText('telnet')}
				/>
				<BulkSelectRow
					id="bulk-telnet-enter"
					label={t('session.telnet_enter')}
					bind:value={telnetNewline}
					keep={KEEP}
					current={placeholderOf(telnetNewlines, '')}
					tooltip={tooltipOf(telnetNewlines, '')}
					options={[{ value: 'crlf', label: 'CR LF' }, { value: 'cr', label: 'CR' }, { value: 'lf', label: 'LF' }]}
				/>
				<BulkSelectRow
					id="bulk-telnet-bs"
					label={t('session.telnet_backspace')}
					bind:value={telnetBackspace}
					keep={KEEP}
					current={placeholderOf(telnetBackspaces, '')}
					tooltip={tooltipOf(telnetBackspaces, '')}
					options={[{ value: 'del', label: 'DEL (^?)' }, { value: 'ctrlh', label: 'Ctrl+H (^H)' }]}
				/>
			{/if}

			{#if shows('rdp')}
				<h3 class="section">{t('session.protocol_rdp')}</h3>
				{#if appliesText('rdp')}<p class="applies section-applies">{appliesText('rdp')}</p>{/if}
				{#each rdpRows as row (row.key)}
					<BulkSelectRow
						id={`bulk-rdp-${row.key}`}
						label={row.label}
						bind:value={rdpSel[row.key]}
						keep={KEEP}
						current={currentOf(rdpNow(row))}
						tooltip={tooltipOf(rdpNow(row), '')}
						options={row.options}
						hint={rdpSel[row.key] !== KEEP ? row.hint : undefined}
					/>
				{/each}
				{#if kerberosChosen}
					<div class="row" class:changed={rdpKdcClear || !!rdpKdc?.trim()}>
						<label class="label" for="bulk-rdp-kdc">{t('session.rdp_kdc')}</label>
						<div class="field">
							{#if rdpKdcClear}
								<p class="cleared">{t('bulk.will_clear')}</p>
							{:else}
								<input
									id="bulk-rdp-kdc"
									type="text"
									autocomplete="off"
									spellcheck="false"
									value={rdpKdc ?? ''}
									oninput={(e) => (rdpKdc = e.currentTarget.value)}
									placeholder={placeholderOf(kdcs, t('bulk.none'))}
									title={tooltipOf(kdcs, t('bulk.none'))}
								/>
							{/if}
						</div>
						{#if rdpKdcClear || rdpKdc}
							<button class="revert" type="button" onclick={() => { rdpKdc = null; rdpKdcClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
						{:else}
							<button class="clear" type="button" onclick={() => (rdpKdcClear = true)}>{t('bulk.clear')}</button>
						{/if}
					</div>
				{/if}
				<div class="row" class:changed={gatewayMode !== KEEP}>
					<label class="label" for="bulk-gateway">{t('bulk.gateway')}</label>
					<div class="field">
						<select id="bulk-gateway" bind:value={gatewayMode} title={tooltipOf(gateways, t('bulk.gateway_none'))}>
							<option value={KEEP}>{t('bulk.keep_value', { value: placeholderOf(gateways, t('bulk.gateway_none')) })}</option>
							<option value="set">{t('bulk.gateway_set')}</option>
							<option value="clear">{t('bulk.gateway_none')}</option>
						</select>
						{#if gatewayMode === 'set'}
							<input
								type="text"
								class:invalid={!!gatewayHost.trim() && !gatewayValid}
								bind:value={gatewayHost}
								placeholder="gateway.example.com"
								aria-label={t('bulk.gateway')}
								spellcheck="false"
								autocomplete="off"
							/>
							<label class="check">
								<input type="checkbox" bind:checked={gatewayDetect} />
								<span>{t('session.rdp_gateway_detect')}</span>
							</label>
							<label class="check">
								<input type="checkbox" bind:checked={gatewayRpc} />
								<span>{t('session.rdp_gateway_rpc')}</span>
							</label>
							<label class="check">
								<input type="checkbox" bind:checked={gatewayOwnLogin} />
								<span>{t('session.rdp_gateway_own_login')}</span>
							</label>
							{#if gatewayOwnLogin}
								<input type="text" bind:value={gatewayUser} placeholder={t('session.rdp_gateway_user')} aria-label={t('session.rdp_gateway_user')} spellcheck="false" autocomplete="off" />
								<PasswordField>
									{#snippet children(type)}
										<input {type} autocomplete="new-password" bind:value={gatewayPass} placeholder={t('session.rdp_gateway_password')} aria-label={t('session.rdp_gateway_password')} />
									{/snippet}
								</PasswordField>
							{:else}
								<p class="hint">{t('session.rdp_gateway_same_login')}</p>
							{/if}
						{/if}
					</div>
					{#if gatewayMode !== KEEP}
						<button class="revert" type="button" onclick={() => (gatewayMode = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
				<div class="row" class:changed={filesPortClear || !!filesPort?.trim()}>
					<label class="label" for="bulk-rdp-files-port">{t('session.rdp_files_port')}</label>
					<div class="field">
						{#if filesPortClear}
							<p class="cleared">22</p>
						{:else}
							<input
								id="bulk-rdp-files-port"
								type="text"
								inputmode="numeric"
								class:invalid={rdpPortInvalid}
								value={filesPort ?? ''}
								oninput={(e) => (filesPort = e.currentTarget.value)}
								placeholder={placeholderOf(filesPorts, '22')}
								title={tooltipOf(filesPorts, '22')}
							/>
						{/if}
					</div>
					{#if filesPortClear || filesPort}
						<button class="revert" type="button" onclick={() => { filesPort = null; filesPortClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{:else}
						<button class="clear" type="button" onclick={() => (filesPortClear = true)}>{t('bulk.clear')}</button>
					{/if}
				</div>
			{/if}

			{#if shows('monitoring') || shows('tmux')}
				<h3 class="section">{t('bulk.section_server')}</h3>
				<BulkSelectRow
					id="bulk-monitoring"
					label={t('session.monitoring')}
					bind:value={monitoring}
					keep={KEEP}
					current={monitorings.same === null ? t('bulk.mixed') : monitorings.same ? onOff(monitorings.same === 'on') : t('history.session_default')}
					tooltip={tooltipOf(monitorings, t('history.session_default'))}
					options={[
						{ value: '', label: t('history.session_default') },
						{ value: 'on', label: t('session.monitoring_on') },
						{ value: 'off', label: t('session.monitoring_off') }
					]}
					applies={appliesText('monitoring')}
				/>
				<BulkSelectRow
					id="bulk-tmux"
					label={t('tmux.session_label')}
					bind:value={tmuxMode}
					keep={KEEP}
					current={tmuxModes.same === null ? t('bulk.mixed') : tmuxModeName(tmuxModes.same)}
					tooltip={tooltipOf(tmuxModes, '')}
					options={[
						{ value: 'inherit', label: t('bulk.tmux_folder') },
						{ value: 'attach', label: t('tmux.mode_attach') },
						{ value: 'ask', label: t('tmux.mode_ask') },
						{ value: 'off', label: t('tmux.mode_off') }
					]}
					applies={appliesText('tmux')}
				/>
				<BulkSelectRow
					id="bulk-tmux-mouse"
					label={t('tmux.mouse')}
					bind:value={tmuxMouse}
					keep={KEEP}
					current={tmuxMice.same === null ? t('bulk.mixed') : onOff(tmuxMice.same === 'on')}
					options={[{ value: 'on', label: t('session.monitoring_on') }, { value: 'off', label: t('session.monitoring_off') }]}
				/>
			{/if}

			{#if shows('terminal_theme') || shows('history') || shows('cursor_style') || shows('terminal')}
				<h3 class="section">{t('bulk.section_terminal')}</h3>
			{/if}

			{#if shows('terminal_theme')}
				<div class="row" class:changed={theme !== null}>
					<span class="label">{t('termtheme.session_title')}</span>
					<div class="field">
						<button class="picker" type="button" onclick={() => (themesOpen = !themesOpen)} title={tooltipOf(themes, t('termtheme.session_default'))}>
							{theme !== null
								? (theme ? (findTerminalTheme(theme)?.name ?? theme) : t('termtheme.session_default'))
								: t('bulk.keep_value', { value: placeholderOf(themes, t('termtheme.session_default')) })}
						</button>
						{#if themesOpen}
							<div class="themes">
								<TerminalThemeList
									selected={theme ?? ''}
									onselect={(id) => { theme = id; themesOpen = false; }}
									noneLabel={t('termtheme.session_default')}
									nonePalette={terminalPalette(null)}
								/>
							</div>
						{/if}
						<p class="applies">{t('bulk.applies', { count: String(count('terminal_theme')), total: String(total) })}</p>
					</div>
					{#if theme !== null}
						<button class="revert" type="button" onclick={() => (theme = null)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('history')}
				<div class="row" class:changed={history !== KEEP}>
					<label class="label" for="bulk-history">{t('history.session_label')}</label>
					<div class="field">
						<select id="bulk-history" bind:value={history} title={tooltipOf(histories, t('history.session_default'))}>
							<option value={KEEP}>{t('bulk.keep_value', { value: histories.same === null ? t('bulk.mixed') : histories.same ? t(`history.mode_${histories.same}`) : t('history.session_default') })}</option>
							<option value="">{t('history.session_default')}</option>
							<option value="off">{t('history.mode_off')}</option>
							<option value="commands">{t('history.mode_commands')}</option>
							<option value="output">{t('history.mode_output')}</option>
						</select>
						<p class="applies">{t('bulk.applies', { count: String(count('history')), total: String(total) })}</p>
					</div>
					{#if history !== KEEP}
						<button class="revert" type="button" onclick={() => (history = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('cursor_style')}
				<div class="row" class:changed={cursor !== KEEP}>
					<label class="label" for="bulk-cursor">{t('cursor.session_label')}</label>
					<div class="field">
						<select id="bulk-cursor" bind:value={cursor} title={tooltipOf(cursors, t('history.session_default'))}>
							<option value={KEEP}>{t('bulk.keep_value', { value: cursors.same === null ? t('bulk.mixed') : cursorName(cursors.same) })}</option>
							<option value="">{t('history.session_default')}</option>
							<option value="block">{t('cursor.style_block')}</option>
							<option value="underline">{t('cursor.style_underline')}</option>
							<option value="bar">{t('cursor.style_bar')}</option>
						</select>
						<p class="applies">{t('bulk.applies', { count: String(count('cursor_style')), total: String(total) })}</p>
					</div>
					{#if cursor !== KEEP}
						<button class="revert" type="button" onclick={() => (cursor = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('cursor_blink')}
				<div class="row" class:changed={blink !== KEEP}>
					<label class="label" for="bulk-blink">{t('cursor.blink')}</label>
					<div class="field">
						<select id="bulk-blink" bind:value={blink} title={tooltipOf(blinks, t('history.session_default'))}>
							<option value={KEEP}>{t('bulk.keep_value', { value: blinks.same === null ? t('bulk.mixed') : blinkName(blinks.same) })}</option>
							<option value="">{t('history.session_default')}</option>
							<option value="on">{t('cursor.blink_on')}</option>
							<option value="off">{t('cursor.blink_off')}</option>
						</select>
						<p class="applies">{t('bulk.applies', { count: String(count('cursor_blink')), total: String(total) })}</p>
					</div>
					{#if blink !== KEEP}
						<button class="revert" type="button" onclick={() => (blink = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('terminal')}
				<div class="row" class:changed={termTypeClear || !!termType?.trim()}>
					<label class="label" for="bulk-term-type">{t('session.term_type')}</label>
					<div class="field">
						{#if termTypeClear}
							<p class="cleared">xterm-256color</p>
						{:else}
							<input
								id="bulk-term-type"
								type="text"
								list="bulk-term-types"
								autocomplete="off"
								spellcheck="false"
								class:invalid={termTypeInvalid}
								value={termType ?? ''}
								oninput={(e) => (termType = e.currentTarget.value)}
								placeholder={placeholderOf(termTypes, 'xterm-256color')}
								title={tooltipOf(termTypes, 'xterm-256color')}
							/>
							<datalist id="bulk-term-types">
								{#each TERMINAL_TYPES as tt (tt)}<option value={tt}></option>{/each}
							</datalist>
						{/if}
						{#if appliesText('terminal')}<p class="applies">{appliesText('terminal')}</p>{/if}
					</div>
					{#if termTypeClear || termType}
						<button class="revert" type="button" onclick={() => { termType = null; termTypeClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{:else}
						<button class="clear" type="button" onclick={() => (termTypeClear = true)}>{t('bulk.clear')}</button>
					{/if}
				</div>
				<BulkSelectRow
					id="bulk-backspace"
					label={t('session.telnet_backspace')}
					bind:value={backspace}
					keep={KEEP}
					current={currentOf(backspaces)}
					tooltip={tooltipOf(backspaces, '')}
					options={[{ value: 'del', label: 'DEL (^?)' }, { value: 'ctrlh', label: 'Ctrl+H (^H)' }]}
				/>
				<BulkSelectRow
					id="bulk-encoding"
					label={t('session.encoding')}
					bind:value={encoding}
					keep={KEEP}
					current={currentOf(encodings)}
					tooltip={tooltipOf(encodings, '')}
					options={TERMINAL_ENCODINGS.map((e) => ({ value: e, label: e }))}
				/>
			{/if}

			<h3 class="section">{t('session.column_organising')}</h3>

			<BulkSelectRow
				id="bulk-favorite"
				label={t('session.favorite')}
				bind:value={favorite}
				keep={KEEP}
				current={currentOf(favorites)}
				tooltip={tooltipOf(favorites, '')}
				options={[{ value: 'on', label: t('bulk.favorite_on') }, { value: 'off', label: t('bulk.favorite_off') }]}
			/>

			{#if shows('os_icon')}
				<div class="row" class:changed={osIcon !== KEEP}>
					<span class="label">{t('session.os_icon')}</span>
					<div class="field">
						<OsIconSelect options={osIconOptions} bind:value={osIcon} />
						{#if osIcon === '' && osAuto}<p class="hint">{t('bulk.os_auto_hint')}</p>{/if}
						{#if appliesText('os_icon')}<p class="applies">{appliesText('os_icon')}</p>{/if}
					</div>
					{#if osIcon !== KEEP}
						<button class="revert" type="button" onclick={() => (osIcon = KEEP)} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{/if}
				</div>
			{/if}

			{#if shows('wol')}
				<h3 class="section">{t('session.wol')}</h3>
				<p class="applies section-applies">{t('bulk.wol_hint', { count: String(count('wol')), total: String(total) })}</p>
				<div class="row" class:changed={wolBroadcastClear || !!wolBroadcast?.trim()}>
					<label class="label" for="bulk-wol-broadcast">{t('net.broadcast')}</label>
					<div class="field">
						{#if wolBroadcastClear}
							<p class="cleared">255.255.255.255</p>
						{:else}
							<input
								id="bulk-wol-broadcast"
								type="text"
								autocomplete="off"
								spellcheck="false"
								value={wolBroadcast ?? ''}
								oninput={(e) => (wolBroadcast = e.currentTarget.value)}
								placeholder={placeholderOf(wolBroadcasts, '255.255.255.255')}
								title={tooltipOf(wolBroadcasts, '255.255.255.255')}
							/>
						{/if}
					</div>
					{#if wolBroadcastClear || wolBroadcast}
						<button class="revert" type="button" onclick={() => { wolBroadcast = null; wolBroadcastClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{:else}
						<button class="clear" type="button" onclick={() => (wolBroadcastClear = true)}>{t('bulk.clear')}</button>
					{/if}
				</div>
				<div class="row" class:changed={wolPortClear || !!wolPort?.trim()}>
					<label class="label" for="bulk-wol-port">{t('net.udp_port')}</label>
					<div class="field">
						{#if wolPortClear}
							<p class="cleared">9</p>
						{:else}
							<input
								id="bulk-wol-port"
								type="text"
								inputmode="numeric"
								class:invalid={wolPortInvalid}
								value={wolPort ?? ''}
								oninput={(e) => (wolPort = e.currentTarget.value)}
								placeholder={placeholderOf(wolPorts, '9')}
								title={tooltipOf(wolPorts, '9')}
							/>
						{/if}
					</div>
					{#if wolPortClear || wolPort}
						<button class="revert" type="button" onclick={() => { wolPort = null; wolPortClear = false; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
					{:else}
						<button class="clear" type="button" onclick={() => (wolPortClear = true)}>{t('bulk.clear')}</button>
					{/if}
				</div>
			{/if}

			<h3 class="section">{t('session.docs')}</h3>

			<div class="row" class:changed={!!tagsAdd.trim() || !!tagsRemove.trim()}>
				<span class="label">{t('bulk.tags')}</span>
				<div class="field">
					<input
						type="text"
						autocomplete="off"
						value={tagsAdd}
						oninput={(e) => (tagsAdd = e.currentTarget.value)}
						placeholder={t('bulk.tags_add')}
						aria-label={t('bulk.tags_add')}
					/>
					<input
						class="sub-input"
						type="text"
						autocomplete="off"
						value={tagsRemove}
						oninput={(e) => (tagsRemove = e.currentTarget.value)}
						placeholder={t('bulk.tags_remove')}
						aria-label={t('bulk.tags_remove')}
						title={describeCounts(allTags, '')}
					/>
				</div>
				{#if tagsAdd || tagsRemove}
					<button class="revert" type="button" onclick={() => { tagsAdd = ''; tagsRemove = ''; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
				{/if}
			</div>

			<div class="row" class:changed={notesOp === 'clear' || (notesOp !== 'keep' && !!notesText.trim())}>
				<label class="label" for="bulk-notes-op">{t('session.notes')}</label>
				<div class="field">
					<select id="bulk-notes-op" bind:value={notesOp}>
						<option value="keep">{t('bulk.keep_value', { value: t('bulk.notes_on', { count: String(withNotes), total: String(total) }) })}</option>
						<option value="replace">{t('bulk.notes_replace')}</option>
						<option value="append">{t('bulk.notes_append')}</option>
						<option value="clear">{t('bulk.clear')}</option>
					</select>
					{#if notesOp === 'replace' || notesOp === 'append'}
						<textarea class="sub-input" rows="3" bind:value={notesText} placeholder={t('session.notes_placeholder')}></textarea>
					{/if}
				</div>
				{#if notesOp !== 'keep'}
					<button class="revert" type="button" onclick={() => { notesOp = 'keep'; notesText = ''; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
				{/if}
			</div>
			<div class="row" class:changed={fieldsSet.some((f) => f.name.trim()) || !!fieldsRemove.trim()}>
				<span class="label">{t('session.custom_fields')}</span>
				<div class="field">
					{#each fieldsSet as f, i (i)}
						<div class="custom">
							<input type="text" bind:value={f.name} placeholder={t('session.field_name')} aria-label={t('session.field_name')} list="bulk-field-names" />
							<input type={f.hidden ? 'password' : 'text'} autocomplete="off" bind:value={f.value} placeholder={t('session.field_value')} aria-label={t('session.field_value')} />
							<label class="check" title={t('session.field_hidden_hint')}>
								<input type="checkbox" bind:checked={f.hidden} />
								<span>{t('session.field_hidden')}</span>
							</label>
							<button class="link" type="button" onclick={() => (fieldsSet = fieldsSet.filter((_, j) => j !== i))} aria-label={t('session.field_remove')}>✕</button>
						</div>
					{/each}
					<button class="link" type="button" onclick={() => (fieldsSet = [...fieldsSet, { name: '', value: '', hidden: false }])}>+ {t('bulk.fields_add')}</button>
					<input
						class="sub-input"
						type="text"
						autocomplete="off"
						bind:value={fieldsRemove}
						placeholder={t('bulk.fields_remove')}
						aria-label={t('bulk.fields_remove')}
						title={describeCounts(fieldNames, '')}
					/>
					<datalist id="bulk-field-names">
						{#each fieldNames.counts as [name] (name)}<option value={name}></option>{/each}
					</datalist>
				</div>
				{#if fieldsSet.length || fieldsRemove}
					<button class="revert" type="button" onclick={() => { fieldsSet = []; fieldsRemove = ''; }} title={t('bulk.revert')} aria-label={t('bulk.revert')}>↺</button>
				{/if}
			</div>
		</div>
	{:else if phase === 'confirm' || phase === 'saving'}
		<div class="bulk">
			<p class="intro">{t('bulk.confirm_intro', { fields: String(shownFields.length), count: String(total) })}</p>
			<ul class="changes">
				{#each shownFields as f (f)}
					{@const n = f === 'credential' && creatingCredential ? countApplying('credential', sessions) : count(f)}
					<li>
						<span class="change-field">{fieldLabel(f)}</span>
						<span class="change-value">{describeChange(f)}</span>
						{#if n < total}
							<span class="change-skip">{t('bulk.skipped_n', { count: String(total - n) })}</span>
						{/if}
					</li>
				{/each}
			</ul>
			<details class="names">
				<summary>{t('bulk.sessions_list', { count: String(total) })}</summary>
				<ul>
					{#each sessions as s (s.id)}
						<li>{s.name} <span class="host">{s.host}</span></li>
					{/each}
				</ul>
			</details>
			<p class="hint">{t('bulk.undo_hint')}</p>
			{#if error}
				<p class="error" role="alert">{error}</p>
			{/if}
		</div>
	{:else if phase === 'report' && report}
		<div class="bulk">
			<p class="intro">
				{t('bulk.report', {
					changed: String(report.results.filter((r) => r.status === 'changed').length),
					failed: String(failedIds.length)
				})}
			</p>
			<ul class="changes">
				{#each report.results.filter((r) => r.status === 'failed') as r (r.id)}
					<li><span class="change-field">{r.name || r.id}</span><span class="error">{r.error}</span></li>
				{/each}
			</ul>
		</div>
	{/if}

	{#snippet actions()}
		{#if phase === 'edit'}
			<Button variant="ghost" onclick={onclose}>{t('common.cancel')}</Button>
			<Button variant="primary" disabled={!canApply} onclick={() => (phase = 'confirm')}>{t('bulk.review')}</Button>
		{:else if phase === 'confirm' || phase === 'saving'}
			<Button variant="ghost" disabled={phase === 'saving'} onclick={() => (phase = 'edit')}>{t('common.back')}</Button>
			<Button variant="primary" disabled={phase === 'saving'} onclick={() => void apply()}>
				{t('bulk.apply_n', { count: String(total) })}
			</Button>
		{:else}
			<Button variant="ghost" onclick={onclose}>{t('common.close')}</Button>
			<Button variant="primary" onclick={() => void apply(failedIds)}>{t('common.retry')}</Button>
		{/if}
	{/snippet}
</Modal>

<style>
	.bulk {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.intro {
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}
	.section {
		margin: 8px 0 0;
		font-size: 0.75rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--color-text-tertiary);
	}
	.row {
		display: grid;
		grid-template-columns: 130px 1fr 52px;
		align-items: start;
		gap: 8px;
		padding: 4px 0 4px 8px;
		border-left: 3px solid transparent;
	}
	.row.changed {
		border-left-color: var(--color-accent);
	}
	.label {
		padding-top: 8px;
		font-size: 0.8125rem;
		color: var(--color-text-primary);
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}
	.field input[type='text'],
	.field input[type='password'],
	.field select,
	.field textarea,
	.picker {
		width: 100%;
		box-sizing: border-box;
		padding: 7px 10px;
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		outline: none;
		text-align: left;
	}
	.field textarea {
		resize: vertical;
	}
	.field input:focus,
	.field select:focus,
	.field textarea:focus {
		border-color: var(--color-accent);
	}
	.field input.invalid {
		border-color: var(--color-danger);
	}
	.picker {
		cursor: pointer;
	}
	.themes {
		max-height: 260px;
		overflow: auto;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		padding: 6px;
	}
	.hop {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 6px 8px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
	}
	.hop-head {
		display: flex;
		justify-content: space-between;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}
	.hop-grid {
		display: grid;
		grid-template-columns: 1fr 80px;
		gap: 4px;
	}
	.custom {
		display: grid;
		grid-template-columns: 1fr 1fr auto auto;
		gap: 4px;
		align-items: center;
	}
	.link {
		align-self: flex-start;
		padding: 2px 0;
		font-size: 0.75rem;
		color: var(--color-accent);
		background: none;
		border: none;
		cursor: pointer;
	}
	.applies,
	.hint {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-tertiary);
	}
	.hint.warn {
		color: var(--color-warning);
	}
	.section-applies {
		padding-left: 11px;
	}
	.cleared {
		margin: 0;
		padding: 7px 0;
		font-size: 0.8125rem;
		font-style: italic;
		color: var(--color-text-secondary);
	}
	.check {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}
	.revert,
	.clear {
		margin-top: 4px;
		padding: 3px 6px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		background: transparent;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		cursor: pointer;
	}
	.revert:hover,
	.clear:hover {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}
	.changes {
		margin: 0;
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.changes li {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		align-items: baseline;
		font-size: 0.8125rem;
	}
	.change-field {
		min-width: 130px;
		color: var(--color-text-secondary);
	}
	.change-value {
		color: var(--color-text-primary);
		font-weight: 500;
		overflow-wrap: anywhere;
	}
	.change-skip {
		font-size: 0.6875rem;
		color: var(--color-text-tertiary);
	}
	.names {
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}
	.names ul {
		max-height: 180px;
		overflow: auto;
		margin: 6px 0 0;
		padding-left: 18px;
	}
	.host {
		color: var(--color-text-tertiary);
	}
	.error {
		margin: 0;
		color: var(--color-danger);
		font-size: 0.8125rem;
		overflow-wrap: anywhere;
	}
	@media (max-width: 520px) {
		.row {
			grid-template-columns: 1fr auto;
		}
		.label {
			grid-column: 1 / -1;
			padding-top: 0;
		}
	}
</style>
