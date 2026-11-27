import { invoke } from '@tauri-apps/api/core';
import type { SecretRef } from './pwmanager';
import type { TelnetOptions } from './telnet';
import type { SerialOptions } from './serial';
import type { SpiceOptions } from './spice';
import type { RdpOptions } from './rdp';
import type { ProxmoxOptions } from './proxmox';
import type { HistoryMode } from './history';
import type { CursorStyle } from '#lib/terminal/cursor.js';
import { t } from '#lib/state/i18n.svelte.js';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

export interface AuthMethod {
  type: 'Password' | 'Key' | 'Agent';
  password?: string; // for Password type - stored encrypted in vault
  path?: string; // for Key type - a key file on this machine
  passphrase?: string; // for Key type - stored encrypted in vault
  /**
   * For Key type - a key imported into the vault, used instead of `path`.
   * Set when the user picked an imported key, so the session also works on a
   * machine that has no copy of the key file.
   */
  key_id?: string;
  /** For Password type - an entry in a password manager; wins over `password`. */
  password_ref?: SecretRef | null;
  /** For Key type - an entry in a password manager; wins over `passphrase`. */
  passphrase_ref?: SecretRef | null;
}

export interface JumpHostConfig {
  host: string;
  port: number;
  username: string;
  auth_method: AuthMethod;
  /** A saved credential this hop logs in with, from the session's vault. */
  credential_id?: string | null;
  /** Read only: the saved credential the hop's login was filled in from. */
  credential?: CredentialUse | null;
}

export interface SessionConfig {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth_method: AuthMethod;
  folder_id: string | null;
  tags: string[];
  detected_os?: string | null;
  /** detected_os was picked by hand, so connecting never replaces it. */
  os_manual?: boolean;
  vault_id?: string | null; // Which vault this session belongs to
  jump_chain?: JumpHostConfig[] | null; // ProxyJump chain
  proxy?: ProxySessionConfig | null; // Proxy config (SOCKS5/Tor, HTTP)
  /** A WireGuard profile whose tunnel carries the connection to the main
   *  address (or the first jump host). SSH and Telnet. */
  wireguard_id?: string | null;
  shell?: string | null; // Optional per-session login shell (e.g. "fish -l")
  /** What the session connects with. Absent means SSH: older saves have no kind. */
  kind?: SessionKind;
  /** Windows logon domain, RDP only. */
  domain?: string | null;
  /** A local folder shown inside the remote desktop as a drive, RDP only. */
  share_path?: string | null;
  /** RDP only: what the clipboard carries; absent for text, pictures and files. */
  rdp_clipboard?: 'text' | 'off' | null;
  rdp_use_freerdp?: boolean;
  rdp_forward_lock_keys?: boolean | null;
  /** RDP only: display, keyboard, sound, performance and program settings. */
  rdp?: RdpOptions | null;
  /** VNC and SPICE only: a saved SSH session to reach the server through. */
  via_session_id?: string | null;
  /** Key sessions: also offer the SSH agent's keys if this key is refused. Off by default. */
  try_agent_keys?: boolean | null;
  /** ssh_config settings: imported files, lines set in Reach, approvals. */
  ssh_options?: SshOptions | null;
  /** WSL only: the distribution name, e.g. "Ubuntu" */
  wsl_distro?: string | null;
  /** Telnet only: how it talks to the device. Absent: the defaults. */
  telnet?: TelnetOptions | null;
  /** Serial only: speed, framing, flow control and keys. The port is `host`. */
  serial?: SerialOptions | null;
  /** SPICE only: TLS port and certificate checks. */
  spice?: SpiceOptions | null;
  /** VNC only: encryption and the certificates it trusts. */
  vnc?: VncOptions | null;
  /** A saved credential this session logs in with (see savedCredentials.ts).
   *  Unset: its folder's, if one applies. */
  credential_id?: string | null;
  /** Read only: the saved credential the login was filled in from. */
  credential?: CredentialUse | null;
  /** On the start screen's Favorites. Stored on the session, so it syncs. */
  favorite?: boolean;
  /** Its place among the sessions of its folder (or those outside any),
   *  lowest first; unset: after those with one. Stored on the session, so
   *  it syncs. Changed only by sessionSetOrder. */
  order?: number | null;
  /** Free text about the server (RDM's Description and Notes). */
  notes?: string | null;
  /** Named values kept with the session, in the user's order. */
  custom_fields?: CustomField[] | null;
  /** The terminal colours its tabs use, by terminal theme id; unset: the
   *  default from Settings (see #lib/state/terminal-themes). */
  terminal_theme?: string | null;
  /** The terminal cursor's shape in its tabs; unset: the default from
   *  Settings (see #lib/terminal/cursor). */
  cursor_style?: CursorStyle | null;
  /** Whether that cursor blinks; unset: the default from Settings. */
  cursor_blink?: boolean | null;
  /** Server monitoring for its tabs, on or off; unset: the default from
   *  Settings. */
  monitoring?: boolean | null;
  /** What its tabs record into the session history; unset: the default
   *  from Settings. */
  history?: HistoryMode | null;
  /** Start its terminals in tmux on the server; unset: as its folder says
   *  (see #lib/terminal/tmux). */
  tmux?: TmuxSetting | null;
  /** SSH only: start this desktop over X11 forwarding, in a window of its
   *  own, instead of a shell. */
  x11_desktop?: X11Desktop | null;
  /** SSH only: terminal type, Backspace and encoding; unset: xterm-256color,
   *  DEL and UTF-8. */
  terminal?: TerminalOptions | null;
  /** The server keys it trusts, one per server it goes through. Read
   *  only: saved by connecting (Accept and save) and changed with
   *  sessionHostKeyRemove / sessionHostKeyReplace; a save keeps them. */
  host_keys?: SavedHostKey[] | null;
  /** A name for the main address, shown when there are others ("Office"). */
  host_label?: string | null;
  /** Other addresses of the same server, by priority after the main one
   *  (see #lib/sessions/addresses). SSH only for now. */
  alt_addresses?: SessionAddress[] | null;
  /** How a session with other addresses picks one; unset: `auto`. */
  connect_mode?: ConnectMode | null;
  /** Check which addresses answer before connecting; unset: as Settings says. */
  address_check?: boolean | null;
  /** Web only: address, storage, sign-in, trusted certificates. */
  web?: WebOptions | null;
  /** Wake-on-LAN for the machine (Wake in its menu, Network tools). */
  wol?: WakeOnLan | null;
  /** SSH only: open in the full-pane file manager over SFTP or SCP
   *  instead of a terminal; unset: a terminal. */
  files?: FilesProtocol | null;
  /** FTP only: encryption, and the certificates it trusts. */
  ftp?: FtpOptions | null;
  /** Server management (BMC) only: trusted certificates (read only here)
   *  and its controller's VNC server. */
  bmc?: BmcOptions | null;
  /** Proxmox only: realm, API token and trusted certificates. */
  proxmox?: ProxmoxOptions | null;
}

/** How to wake a session's machine over the network. */
export interface WakeOnLan {
  /** Its network card's MAC address, as typed. */
  mac: string;
  /** A broadcast address (10.0.0.255 for another subnet) or name; unset:
   *  255.255.255.255. */
  broadcast?: string | null;
  /** UDP port; unset: 9. */
  port?: number | null;
}

/** How an SSH session that opens as files reaches them. */
export type FilesProtocol = 'sftp' | 'scp';

/** An FTP session's settings (see src-tauri/src/ftp/mod.rs). */
/** A VNC session's encryption. */
export interface VncOptions {
  /** `vencrypt`: TLS negotiated inside VNC (QEMU/libvirt, TigerVNC, wayvnc);
   *  `tls`: TLS from the first byte (an iDRAC with SSL encryption on);
   *  unset: none. */
  encryption?: 'vencrypt' | 'tls' | null;
  /** SHA-256 fingerprints of certificates this session trusts although the
   *  system does not. Read only here: trusted from the desktop's tab, kept
   *  by a save. */
  certs?: string[];
}

export interface FtpOptions {
  /** Unset: explicit (AUTH TLS, required). */
  tls?: 'implicit' | 'none' | null;
  /** Fingerprints trusted by answering the prompt; read only here. */
  certs?: string[];
}

/** A web session's settings (see src-tauri/src/web/mod.rs). */
export interface WebOptions {
  /** Unset: https. */
  scheme?: 'http' | null;
  /** Path and query after the address; unset: `/`. */
  path?: string | null;
  /** Unset: its own window; `browser`: the system's browser. */
  open_in?: 'browser' | null;
  /** Unset: kept for this session alone. */
  storage?: 'profile' | 'temporary' | null;
  profile?: string | null;
  /** Unset: `auto`. */
  login?: WebLogin | null;
  username_selector?: string | null;
  password_selector?: string | null;
  submit_selector?: string | null;
  /** Press the login button once filled; unset: yes. */
  submit?: boolean | null;
  /** Let the page open windows (a virtual console) as more windows of
   *  this session; unset: yes. */
  popups?: boolean | null;
  /** Read only: certificates trusted by answering the prompt; changed
   *  with webSessionCertAdd / webSessionCertRemove, a save keeps them. */
  certs?: WebCert[];
}

export type WebLogin = 'auto' | 'off' | 'idrac' | 'ilo' | 'proxmox' | 'unifi' | 'generic' | 'custom';

export interface WebCert {
  /** SHA-256 as AB:CD:… */
  sha256: string;
  host: string;
  saved_at?: number | null;
}

/** What `web-cert-ask` carries: a certificate a web window met that the
 *  system does not trust and the session does not either. */
export interface WebCertAsk {
  label: string;
  sessionId: string;
  sessionName: string;
  host: string;
  fingerprint: string;
  /** The session trusted other certificates before. */
  changed: boolean;
}

/** The address a web session opens, as the backend builds it. */
export function webSessionUrl(s: Pick<SessionConfig, 'host' | 'port' | 'web'>): string {
  const scheme = s.web?.scheme === 'http' ? 'http' : 'https';
  const host = s.host.trim().replace(/^\[|\]$/g, '');
  const hostPart = host.includes(':') ? `[${host}]` : host;
  const def = scheme === 'http' ? 80 : 443;
  const port = s.port && s.port !== def ? `:${s.port}` : '';
  const path = s.web?.path ? (s.web.path.startsWith('/') ? s.web.path : `/${s.web.path}`) : '/';
  return `${scheme}://${hostPart}${port}${path}`;
}

/** Open a web session in its own window (desktop); resolves with its label. */
export async function webSessionOpen(sessionId: string): Promise<string> {
  await ensureSessionWireguard(sessionId);
  return invoke<string>('web_session_open', { sessionId });
}

export async function webSessionCertAnswer(label: string, fingerprint: string, trust: boolean): Promise<void> {
  return invoke('web_session_cert_answer', { label, fingerprint, trust });
}

export async function webSessionCertAdd(sessionId: string, fingerprint: string, host?: string): Promise<WebCert[]> {
  return invoke<WebCert[]>('web_session_cert_add', { sessionId, fingerprint, host: host ?? null });
}

export async function webSessionCertRemove(sessionId: string, fingerprint?: string): Promise<WebCert[]> {
  return invoke<WebCert[]>('web_session_cert_remove', { sessionId, fingerprint: fingerprint ?? null });
}

export async function webSessionClearData(sessionId: string): Promise<void> {
  return invoke('web_session_clear_data', { sessionId });
}

/** `auto`: by priority, skipping what does not answer; `ask`: a picker on
 *  every connect; `main`: the main address only. */
export type ConnectMode = 'auto' | 'ask' | 'main';

/** One more address of a session's server. */
export interface SessionAddress {
  label?: string;
  host: string;
  /** Unset: the session's port. */
  port?: number | null;
  /** Reached straight, without the session's jump hosts or proxy. */
  direct?: boolean;
  /** Reached through another saved SSH session instead (its jump hosts,
   *  then its server); wins over `direct`. */
  via_session_id?: string | null;
  /** Reached through this WireGuard profile's tunnel, straight; wins over
   *  the others. */
  via_wireguard?: string | null;
}

/** A server key a session trusts. */
export interface SavedHostKey {
  /** The server as reached, `host:port` (a jump host's own). */
  host: string;
  /** Empty or absent when typed in by hand. */
  key_type?: string;
  /** `SHA256:` and base64. */
  fingerprint: string;
  /** Seconds since 1970. */
  saved_at?: number | null;
}

/** The desktops an X11 Desktop session can start. */
export const X11_DESKTOPS = ['gnome', 'kde', 'xfce', 'mate', 'cinnamon', 'lxqt', 'openbox', 'custom'] as const;
export type X11DesktopKind = (typeof X11_DESKTOPS)[number];

/** An SSH session's terminal settings (see `ssh::terminal`). Unset fields
 *  are the defaults. */
export interface TerminalOptions {
  /** TERM announced to the server, e.g. vt100. */
  term?: string | null;
  /** What Backspace sends. */
  backspace?: 'del' | 'ctrlh' | null;
  /** The server's character encoding (WHATWG name), e.g. ISO-8859-2. */
  encoding?: string | null;
}

/** Terminal types offered in the session editor; any other can be typed. */
export const TERMINAL_TYPES = ['xterm-256color', 'xterm', 'vt100', 'vt220', 'linux', 'screen-256color', 'tmux-256color', 'ansi', 'dumb'];

/** Encodings offered in the session editor (ssh/terminal.rs `ENCODINGS`). */
export const TERMINAL_ENCODINGS = [
  'UTF-8', 'ISO-8859-2', 'ISO-8859-7', 'ISO-8859-15', 'windows-1250', 'windows-1251', 'windows-1252', 'windows-1253', 'KOI8-R', 'KOI8-U', 'IBM866',
  'Shift_JIS', 'EUC-JP', 'EUC-KR', 'GBK', 'gb18030', 'Big5'
];

/** A session's X11 desktop (see `ssh::x11_desktop`). */
export interface X11Desktop {
  desktop: X11DesktopKind;
  /** The start command, for `custom`. */
  command?: string | null;
}

/** A session's own tmux choice. */
export interface TmuxSetting {
  /** `attach`: attach to the named tmux session, or create it; `ask`: pick
   *  from the server's tmux sessions at connect; `inherit`: as the folder
   *  says (kept to hold a name). */
  mode: 'off' | 'attach' | 'ask' | 'inherit';
  /** The tmux session's name; unset: made from the session's name. */
  name?: string | null;
  /** tmux's mouse mode for this session: the wheel scrolls tmux's history. */
  mouse?: boolean;
  /** The program: tmux (unset), Byobu or Zellij. */
  tool?: 'tmux' | 'byobu' | 'zellij' | null;
}

/** One named value of a session's documentation. */
export interface CustomField {
  name: string;
  value: string;
  /** Shown masked until revealed. */
  hidden?: boolean;
}

/** Which saved credential a session's login came from, as read. */
export interface CredentialUse {
  id: string;
  /** Empty when the credential is missing. */
  name: string;
  /** The folder it is inherited from; absent when the session links it. */
  folder_id?: string | null;
  /** Not in the session's vault (deleted, or not synced yet): the session's
   *  own stored login is used. */
  missing?: boolean;
  /** Set when the session or its folder asks for each person's own login
   *  (`credential_id` = PERSONAL_LOGIN): the session or folder id the
   *  reader's choice is kept under. `missing` then means none is chosen. */
  personal_target?: string | null;
  /** With `personal_target`: the session's own user name, which the
   *  reader's choice may have replaced. The editor shows and keeps it. */
  own_username?: string | null;
}

/** A config file as it was read at import. */
export interface SshConfigFile {
  path: string;
  text: string;
  role: 'user' | 'system' | 'included';
}

/** A session's ssh_config settings (see src-tauri/src/ssh/sshconf). */
export interface SshOptions {
  /** What the session was imported from; resolved again at every connect. */
  imported?: { alias: string; files: SshConfigFile[]; at: number } | null;
  /** Lines set in Reach ("MACs +hmac-sha1"); they win over the files. */
  lines?: string[];
  /** Local commands the user allowed for this session, exactly as written. */
  approved_commands?: string[];
  /** Weakening settings the user has seen and kept ("Keyword value"). */
  accepted_weakenings?: string[];
  /** This person's approvals in plain words: filled from the signed ones when
   *  a session is read, signed by the backend when it is saved. */
  my_approved_commands?: string[] | null;
  my_accepted_weakenings?: string[] | null;
}

/** `winbox`: MikroTik's WinBox 4, started as a program of its own (desktop). */
/** A server management session's own settings. */
export interface BmcOptions {
  certs?: string[];
  vnc?: BmcVnc | null;
}

/** How to reach a controller's own VNC server (an iDRAC's). */
export interface BmcVnc {
  /** Unset: 5901. */
  port?: number | null;
  /** The whole connection in TLS (an iDRAC's SSL encryption). */
  tls?: boolean;
  /** Unset: asked for at each connect. */
  password?: string | null;
}

export type SessionKind = 'ssh' | 'rdp' | 'vnc' | 'spice' | 'wsl' | 'telnet' | 'serial' | 'winbox' | 'web' | 'ftp' | 'bmc' | 'proxmox';

export function sessionKind(session: Pick<SessionConfig, 'kind'>): SessionKind {
  return session.kind ?? 'ssh';
}

export interface ProxySessionConfig {
  proxy_type: string;
  host: string;
  port: number;
  username?: string | null;
  password?: string | null;
  /** An entry in a password manager; wins over `password`. */
  password_ref?: SecretRef | null;
}

export interface Folder {
  id: string;
  name: string;
  parent_id: string | null;
  vault_id: string | null;
  /** A saved credential the sessions inside log in with. */
  credential_id?: string | null;
  /** Start the sessions inside in tmux, unless they or a nearer folder say otherwise. */
  tmux?: boolean | null;
  /** The icon shown for it: `fa:<symbol>` or `os:<logo>`; none: the plain folder. */
  icon?: string | null;
  /** That icon's colour, by name (see data/folder-icons.ts). */
  color?: string | null;
}

export async function sessionList(): Promise<SessionConfig[]> {
  return invoke<SessionConfig[]>('session_list');
}

export async function sessionGet(sessionId: string): Promise<SessionConfig> {
  return invoke<SessionConfig>('session_get', { sessionId });
}

export async function sessionCreate(params: {
  name: string;
  host: string;
  port: number;
  username: string;
  authMethod: AuthMethod;
  folderId: string | null;
  tags: string[];
  vaultId?: string | null;
  jumpChain?: JumpHostConfig[] | null;
  proxy?: ProxySessionConfig | null;
  wireguardId?: string | null;
  shell?: string | null;
  kind?: SessionKind;
  domain?: string | null;
  /** For RDP: what the machine is, since the protocol cannot say. */
  detectedOs?: string | null;
  osManual?: boolean;
  sharePath?: string | null;
  rdpClipboard?: 'text' | 'off' | null;
  rdpUseFreerdp?: boolean;
  rdpForwardLockKeys?: boolean | null;
  rdp?: RdpOptions | null;
  viaSessionId?: string | null;
  tryAgentKeys?: boolean | null;
  sshOptions?: SshOptions | null;
  wslDistro?: string | null;
  credentialId?: string | null;
  telnet?: TelnetOptions | null;
  serial?: SerialOptions | null;
  spice?: SpiceOptions | null;
  vnc?: VncOptions | null;
  notes?: string | null;
  customFields?: CustomField[] | null;
  terminalTheme?: string | null;
  cursorStyle?: CursorStyle | null;
  cursorBlink?: boolean | null;
  monitoring?: boolean | null;
  history?: HistoryMode | null;
  tmux?: TmuxSetting | null;
  x11Desktop?: X11Desktop | null;
  terminal?: TerminalOptions | null;
  hostLabel?: string | null;
  altAddresses?: SessionAddress[] | null;
  connectMode?: ConnectMode | null;
  addressCheck?: boolean | null;
  web?: WebOptions | null;
  wol?: WakeOnLan | null;
  files?: FilesProtocol | null;
  ftp?: FtpOptions | null;
  proxmox?: ProxmoxOptions | null;
  bmc?: BmcOptions | null;
}): Promise<SessionConfig> {
  return invoke<SessionConfig>('session_create', {
    name: params.name,
    host: params.host,
    port: params.port,
    username: params.username,
    authMethod: params.authMethod,
    folderId: params.folderId,
    tags: params.tags,
    vaultId: params.vaultId ?? null,
    jumpChain: params.jumpChain ?? null,
    proxy: params.proxy ?? null,
    wireguardId: params.wireguardId || null,
    shell: params.shell?.trim() ? params.shell.trim() : null,
    kind: params.kind ?? 'ssh',
    domain: params.domain?.trim() ? params.domain.trim() : null,
    detectedOs: params.detectedOs ?? null,
    osManual: params.osManual || null,
    sharePath: params.sharePath?.trim() ? params.sharePath.trim() : null,
    rdpClipboard: params.kind === 'rdp' ? (params.rdpClipboard ?? null) : null,
    rdpUseFreerdp: params.kind === 'rdp' && params.rdpUseFreerdp === true,
    rdpForwardLockKeys: params.kind === 'rdp' ? (params.rdpForwardLockKeys ?? null) : null,
    rdp: params.kind === 'rdp' ? (params.rdp ?? null) : null,
    viaSessionId: params.viaSessionId || null,
    tryAgentKeys: params.tryAgentKeys || null,
    sshOptions: params.sshOptions ?? null,
    wslDistro: params.wslDistro || null,
    credentialId: params.credentialId || null,
    telnet: params.kind === 'telnet' ? (params.telnet ?? null) : null,
    serial: params.kind === 'serial' ? (params.serial ?? null) : null,
    spice: params.kind === 'spice' ? (params.spice ?? null) : null,
    vnc: params.kind === 'vnc' ? (params.vnc ?? null) : null,
    notes: params.notes?.trim() ? params.notes : null,
    customFields: params.customFields?.length ? params.customFields : null,
    terminalTheme: params.terminalTheme || null,
    cursorStyle: params.cursorStyle || null,
    cursorBlink: params.cursorBlink ?? null,
    monitoring: params.monitoring ?? null,
    history: params.history || null,
    tmux: params.tmux ?? null,
    x11Desktop: params.kind === 'ssh' || !params.kind ? (params.x11Desktop ?? null) : null,
    terminal: params.kind === 'ssh' || !params.kind ? (params.terminal ?? null) : null,
    hostLabel: params.hostLabel?.trim() || null,
    altAddresses: params.altAddresses?.length ? params.altAddresses : null,
    connectMode: params.altAddresses?.length ? (params.connectMode ?? null) : null,
    addressCheck: params.altAddresses?.length ? (params.addressCheck ?? null) : null,
    web: params.kind === 'web' ? (params.web ?? {}) : null,
    wol: params.wol?.mac.trim() ? params.wol : null,
    files: (params.kind ?? 'ssh') === 'ssh' ? (params.files ?? null) : null,
    ftp: params.kind === 'ftp' ? (params.ftp ?? {}) : null,
    proxmox: params.kind === 'proxmox' ? (params.proxmox ?? {}) : null,
    bmc: params.kind === 'bmc' ? (params.bmc ?? null) : null,
  });
}

export async function sessionUpdate(session: SessionConfig): Promise<SessionConfig> {
  return invoke<SessionConfig>('session_update', { session });
}

/** One password manager entry of a session waiting for the user's approval. */
export interface RefSlot {
  /** `password`, `passphrase`, `jump:<n>:password`, `jump:<n>:passphrase` or `proxy`. */
  slot: string;
  provider: string;
  entry_id: string;
}

/**
 * A session's password manager entries the current user has not approved,
 * with where they would go, read from the stored session in one go. The
 * confirmation dialog shows this and sends `context` and `refs` back.
 */
export interface UntrustedRefs {
  session_id: string;
  session_name: string;
  context: string;
  host: string;
  port: number;
  username: string;
  /** `user@host:port` for each jump host. */
  jumps: string[];
  /** `type user@host:port` of the proxy, if one is used. */
  proxy?: string | null;
  /** The session's own ssh_config lines that change where it connects (HostName, ProxyJump, ProxyCommand). */
  route_lines: string[];
  /** The SSH session a VNC session goes through, as stored now. */
  via?: ViaRoute | null;
  refs: RefSlot[];
}

/** Where the SSH session a VNC session goes through connects. */
export interface ViaRoute {
  session_id: string;
  session_name: string;
  host: string;
  port: number;
  username: string;
  jumps: string[];
  proxy?: string | null;
  route_lines: string[];
}

/** What a connect error starts with when the session has unapproved entries; JSON follows. */
export const UNTRUSTED_REFS_PREFIX = 'PWMANAGER_UNTRUSTED:';

/** The unapproved entries a connect was refused for, or null for any other error. */
export function untrustedRefsFromError(err: unknown): UntrustedRefs | null {
  const text = String(err);
  const at = text.indexOf(UNTRUSTED_REFS_PREFIX);
  if (at < 0) return null;
  try {
    return JSON.parse(text.slice(at + UNTRUSTED_REFS_PREFIX.length)) as UntrustedRefs;
  } catch {
    return null;
  }
}

/**
 * For a caller that cannot ask (a database, container host or cluster that
 * goes through a saved session): the unapproved-entries error as a sentence
 * pointing at the session list. Other errors pass through unchanged.
 */
export function explainUntrustedRefs<T>(call: Promise<T>): Promise<T> {
  return call.catch((err: unknown) => {
    throw untrustedRefsFromError(err) ? t('pwmanager.untrusted_reopen') : err;
  });
}

/** The session's unapproved password manager entries a connect would use, or null. */
export async function sessionUntrustedSecretRefs(sessionId: string): Promise<UntrustedRefs | null> {
  return invoke<UntrustedRefs | null>('session_untrusted_secret_refs', { sessionId });
}

/**
 * Approve entries, as the user just did in the confirmation dialog. Refused
 * if the session changed since `asked` was read.
 */
export async function sessionApproveSecretRefs(asked: UntrustedRefs): Promise<SessionConfig> {
  return invoke<SessionConfig>('session_approve_secret_refs', {
    sessionId: asked.session_id,
    context: asked.context,
    refs: asked.refs,
  });
}

export async function sessionDelete(sessionId: string): Promise<void> {
  return invoke('session_delete', { sessionId });
}

/** Move sessions into a folder (`null`: out of any folder) in one call.
 *  Only the folder changes; returns how many moved. */
export async function sessionMoveToFolder(sessionIds: string[], folderId: string | null): Promise<number> {
  return invoke<number>('session_move_to_folder', { sessionIds, folderId });
}

/** Remove a session's key for one server (`host:port`), or all of them
 *  when `host` is null; the next connect asks again. Returns the keys left. */
export async function sessionHostKeyRemove(sessionId: string, host: string | null): Promise<SavedHostKey[]> {
  return invoke<SavedHostKey[]>('session_host_key_remove', { sessionId, host });
}

/** Trust this fingerprint (as `ssh-keygen -lf` prints it) for one server
 *  of a session instead of the one saved. Returns the keys now kept. */
export async function sessionHostKeyReplace(sessionId: string, host: string, fingerprint: string): Promise<SavedHostKey[]> {
  return invoke<SavedHostKey[]>('session_host_key_replace', { sessionId, host, fingerprint });
}

/** What hostKeysRemoveAll did. */
export interface HostKeysCleared {
  sessions: number;
  failed: number;
  device: number;
}

/** Remove every saved host key: from all sessions (shared vaults too) and
 *  from this device's own list. */
export async function hostKeysRemoveAll(): Promise<HostKeysCleared> {
  return invoke<HostKeysCleared>('host_keys_remove_all');
}

/** Mark sessions as favorites (or not) in one call. Only that flag
 *  changes; returns how many changed. */
export async function sessionSetFavorite(sessionIds: string[], favorite: boolean): Promise<number> {
  return invoke<number>('session_set_favorite', { sessionIds, favorite });
}

/** Put sessions in this order: each gets its index as its place. Give
 *  all the sessions of one folder (or those outside any). Only that place
 *  changes; returns how many changed. */
export async function sessionSetOrder(sessionIds: string[]): Promise<number> {
  return invoke<number>('session_set_order', { sessionIds });
}

export async function sessionListFolders(): Promise<Folder[]> {
  return invoke<Folder[]>('session_list_folders');
}

export async function sessionCreateFolder(name: string, parentId: string | null, vaultId?: string | null): Promise<Folder> {
  return invoke<Folder>('session_create_folder', { name, parentId, vaultId: vaultId ?? null });
}

export async function sessionUpdateFolderParent(folderId: string, parentId: string | null): Promise<Folder> {
  return invoke<Folder>('session_update_folder_parent', { folderId, parentId });
}

/** Turn tmux on or off for a folder's sessions; null: as the folder above says. */
export async function sessionSetFolderTmux(folderId: string, tmux: boolean | null): Promise<Folder> {
  return invoke<Folder>('session_set_folder_tmux', { folderId, tmux });
}

/** Set or clear the saved credential a folder's sessions log in with. */
export async function sessionSetFolderCredential(folderId: string, credentialId: string | null): Promise<Folder> {
  return invoke<Folder>('session_set_folder_credential', { folderId, credentialId });
}

/** Set or clear a folder's icon and colour (null: the plain folder, the theme's colour). */
export async function sessionSetFolderIcon(folderId: string, icon: string | null, color: string | null): Promise<Folder> {
  return invoke<Folder>('session_set_folder_icon', { folderId, icon, color });
}

/** Rename a folder; what is inside stays put. */
export async function sessionRenameFolder(folderId: string, name: string): Promise<Folder> {
  return invoke<Folder>('session_rename_folder', { folderId, name });
}

export async function sessionDeleteFolder(folderId: string): Promise<void> {
  return invoke('session_delete_folder', { folderId });
}

export interface ShareResult {
  shareId: string;
  shareUrl: string;
}

/** Share a session with another user via X25519 key re-wrap. */
export async function sessionShare(
  sessionId: string,
  recipientUuid: string,
  recipientPublicKey: string,
  expiresInHours?: number
): Promise<ShareResult> {
  return invoke<ShareResult>('session_share', {
    session_id: sessionId,
    recipient_uuid: recipientUuid,
    recipient_public_key: recipientPublicKey,
    expires_in_hours: expiresInHours
  });
}

/** Fetch installed WSL distributions on Windows */
export async function wslListDistros(): Promise<string[]> {
  return invoke<string[]>('wsl_list_distros');
}

/** One field of a bulk edit: a value for every session, or none. */
export type BulkChange<T> = { set: T } | 'clear';

/** What a bulk edit changes; a field left out stays as each session has it. */
export interface SessionPatch {
  username?: string;
  /** Only sessions that log in with a password take it. */
  password?: BulkChange<string>;
  /** `{ set: id }` a saved credential, `{ set: 'own' }` the session's own
   *  login, `'clear'` the folder's credential, if any. */
  credential?: BulkChange<string>;
  port?: number;
  domain?: BulkChange<string>;
  tags_add?: string[];
  tags_remove?: string[];
  notes?: { op: 'replace' | 'append'; text: string } | { op: 'clear' };
  terminal_theme?: BulkChange<string>;
  history?: BulkChange<HistoryMode>;
  cursor_style?: BulkChange<CursorStyle>;
  cursor_blink?: BulkChange<boolean>;
  /** SSH: log in with a key, the agent, or a password (set by `password`). */
  auth?: AuthMethod;
  /** SSH key sessions: also offer the agent's keys. */
  try_agent_keys?: boolean;
  /** SSH: the whole jump host chain, or none. */
  jump_chain?: BulkChange<JumpHostConfig[]>;
  /** SSH: the proxy, or none. */
  proxy?: BulkChange<ProxySessionConfig>;
  /** SSH: the login shell, or the server's default. */
  shell?: BulkChange<string>;
  /** Custom fields set by name (added or overwritten). */
  fields_set?: CustomField[];
  /** Custom fields removed by name. */
  fields_remove?: string[];
  /** Telnet: the parts of its options to change. */
  telnet?: Partial<TelnetOptions>;
  /** SSH and WSL: server monitoring on or off, or the default. */
  monitoring?: BulkChange<boolean>;
  /** SSH and WSL: tmux mode (`inherit`: as the folder says) and mouse. */
  tmux?: { mode?: 'off' | 'attach' | 'ask' | 'inherit'; mouse?: boolean };
  /** VNC: the saved SSH session it goes through, or directly. */
  via_session?: BulkChange<string>;
  /** A WireGuard profile the connection goes through, or none. */
  wireguard?: BulkChange<string>;
  /** Sessions with other addresses: how one is picked. */
  connect_mode?: ConnectMode;
  /** Sessions with other addresses: check them first; `'clear'`: as Settings says. */
  address_check?: BulkChange<boolean>;
  /** SSH: open in the file manager; `'clear'`: a terminal. */
  files?: BulkChange<FilesProtocol>;
  /** SSH: terminal type, Backspace and encoding; `'clear'`: the default. */
  terminal?: { term?: BulkChange<string>; backspace?: BulkChange<'del' | 'ctrlh'>; encoding?: BulkChange<string> };
  /** RDP: the settings to change; `'clear'`: the default. */
  rdp?: RdpPatch;
  /** Sessions with a Wake-on-LAN MAC: where the packet goes. */
  wol?: { broadcast?: BulkChange<string>; port?: BulkChange<number> };
  favorite?: boolean;
  /** The list icon by OS id (`unknown`: none), kept on connect; `'clear'`:
   *  found on connect again (SSH), else none. */
  os_icon?: BulkChange<string>;
}

/** An RD Gateway set on many sessions at once. */
export interface RdpGatewayPatch {
  host: string;
  detect: boolean;
  rpc: boolean;
  user?: string | null;
  password?: string | null;
}

/** The RDP settings a bulk edit changes (see src-tauri/src/ipc/session_bulk.rs). */
export interface RdpPatch {
  client?: 'ironrdp' | 'freerdp';
  clipboard?: BulkChange<'text' | 'off'>;
  forward_lock_keys?: boolean;
  resolution?: BulkChange<string>;
  scale?: BulkChange<number>;
  color_depth?: BulkChange<number>;
  keyboard_layout?: BulkChange<number>;
  sound?: BulkChange<'off'>;
  audio_quality?: BulkChange<'dynamic' | 'medium' | 'high'>;
  performance?: BulkChange<'fast' | 'best'>;
  compression?: boolean;
  keepalive?: BulkChange<number>;
  jiggler?: BulkChange<number>;
  admin?: boolean;
  all_drives?: boolean;
  auth?: BulkChange<'kerberos'>;
  kdc?: BulkChange<string>;
  gateway?: BulkChange<RdpGatewayPatch>;
  files_port?: BulkChange<number>;
}

export interface BulkOutcome {
  id: string;
  name: string;
  status: 'changed' | 'unchanged' | 'failed';
  /** Patch fields this session does not take, e.g. `domain` on SSH. */
  skipped: string[];
  error?: string;
}

export interface BulkResult {
  results: BulkOutcome[];
  /** For `sessionBulkUndo`; absent when nothing changed. */
  undo?: string;
}

/** Change the same fields on many sessions. Each is saved as the session
 *  editor would save it; one that fails is reported and the rest stay saved. */
export async function sessionBulkUpdate(sessionIds: string[], patch: SessionPatch): Promise<BulkResult> {
  return invoke<BulkResult>('session_bulk_update', { sessionIds, patch });
}

/** Put the sessions of the last bulk edit back as they were, except those
 *  changed since (`kept`, by name). */
export async function sessionBulkUndo(token: string): Promise<{ restored: number; kept: string[] }> {
  return invoke<{ restored: number; kept: string[] }>('session_bulk_undo', { token });
}
