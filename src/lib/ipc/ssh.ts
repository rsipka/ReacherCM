import { ensureWireguard } from '#lib/state/wireguard.svelte.js';
import { invoke } from '@tauri-apps/api/core';
import type { SshOptions, TerminalOptions, X11Desktop } from './sessions';
import type { SecretRef } from './pwmanager';

/** What a connect asks of tmux on the server (see #lib/terminal/tmux). */
export interface TmuxStart {
  /** The tmux session to attach to, or create (a new one, when asking). */
  name: string;
  /** Ask first, from the server's tmux sessions (see sshTmuxChosen). */
  ask?: boolean;
  /** tmux's mouse mode on for this session. */
  mouse?: boolean;
  /** The tab's own view of the session, `{name}-{view}`: a session grouped
   *  with it, so the tab shows a window of its own. */
  view?: string;
  /** The window the view opens on: 'new', or a window id ('@3'); unset:
   *  the one with the latest activity. */
  window?: string;
  /** tmux's status line off in the view. */
  hideStatus?: boolean;
  /** The name earlier versions gave the same session: attached to instead
   *  while only it is on the server (see sshTmuxChosen). */
  legacy?: string;
}

/** One window of a tab's tmux session. */
export interface TmuxWindow {
  /** tmux's id for it ('@3'). */
  id: string;
  index: number;
  name: string;
  /** The program in its active pane. */
  command: string;
  panes: number;
  /** The one the tab shows. */
  active: boolean;
}

/** What a tab's tmux windows dialog does to its view. */
export type TmuxWindowOp =
  | { kind: 'select'; window: string }
  | { kind: 'rename'; window: string; name: string }
  | { kind: 'close'; window: string }
  | { kind: 'new' };

/** One of the server's tmux sessions. */
export interface TmuxListed {
  name: string;
  windows: number;
  attached: number;
  /** Last activity, Unix seconds. */
  activity: number;
}

export interface JumpHostConnectParams {
  host: string;
  port: number;
  username: string;
  authMethod: string;
  password?: string;
  keyPath?: string;
  keyPassphrase?: string;
  /** An imported key, used instead of keyPath. See ipc/sshkeys. */
  keyId?: string;
  /** Password-manager entries, resolved by the backend at connect; they win over the plaintext. */
  passwordRef?: SecretRef;
  keyPassphraseRef?: SecretRef;
}

export interface ProxyConfig {
  proxy_type: string;
  host: string;
  port: number;
  username?: string;
  password?: string;
  /** A password-manager entry for the proxy password. */
  password_ref?: SecretRef;
}

export interface SshConnectParams {
  id: string;
  host: string;
  port: number;
  username: string;
  authMethod: string;
  password?: string;
  keyPath?: string;
  keyPassphrase?: string;
  /** An imported key, used instead of keyPath. See ipc/sshkeys. */
  keyId?: string;
  /** Password-manager entries, resolved by the backend at connect; they win over the plaintext. */
  passwordRef?: SecretRef;
  keyPassphraseRef?: SecretRef;
  cols: number;
  rows: number;
  jumpChain?: JumpHostConnectParams[];
  proxy?: ProxyConfig;
  /** Optional per-session login shell (e.g. "fish" or "fish -l"). Empty = remote default. */
  shell?: string;
  /** Type the shell colour/prompt setup after login (off unless asked for). */
  injectColors?: boolean;
  /** Write the session to a text file; null or absent logs nothing. */
  sessionLog?: SessionLogConfig | null;
  /** Keep the server's login message (MOTD) on screen (default true). */
  showLoginMessage?: boolean;
  /** Key sessions: also offer the SSH agent's keys if the session's key is refused. Off unless the session says so. */
  tryAgentKeys?: boolean;
  /** The session's ssh_config settings, applied by the backend at connect. */
  sshOptions?: SshOptions | null;
  /**
   * The saved session this connect is for. Required with any password-manager
   * entry: the backend takes the entries from the stored session, and only
   * those the user approved for it.
   */
  sessionId?: string | null;
  /** Start inside this tmux session on the server (attach or create);
   *  absent: no tmux. See #lib/terminal/tmux. */
  tmux?: TmuxStart | null;
  /** Start this desktop over X11 forwarding, in a window of its own,
   *  instead of a shell (desktop apps only). */
  x11Desktop?: X11Desktop | null;
  /** Terminal type, Backspace and encoding; absent: the defaults. */
  terminal?: TerminalOptions | null;
  /** A line typed once the server has gone quiet after login (a server
   *  management controller's serial console command). Kept on the tab
   *  only; not sent to the backend. */
  afterLogin?: string;
  /** A WireGuard profile whose tunnel carries the connection; brought up
   *  first (asking the person) when it is off. */
  wireguard?: string | null;
  /** The session's other addresses, for a reconnect to fall back on when
   *  the one it used no longer answers (a laptop that left the office).
   *  Kept on the tab only; not sent to the backend. */
  fallback?: AddressFallback;
}

/** See `SshConnectParams.fallback`. */
export interface AddressFallback {
  addresses: { host: string; port: number; routed: boolean; label: string; index: number; viaSessionId?: string; wireguardId?: string; tunnelOnly?: boolean }[];
  /** The hops of addresses that go through another saved session, by
   *  that session's id. */
  viaChains?: Record<string, JumpHostConnectParams[]>;
  /** The session's route, for addresses that go through it. */
  jumpChain?: JumpHostConnectParams[];
  proxy?: ProxyConfig;
  sessionPort: number;
  /** Check which answer before trying them; off: silent, in order. */
  check?: boolean;
}

export interface ConnectionInfo {
  id: string;
  host: string;
  port: number;
  username: string;
}

/** Settings → Appearance → Session logging, in the shape the backend reads. */
export interface SessionLogConfig {
  mode: 'printable' | 'all';
  /** Empty means the default folder (see sessionLogDefaultDir). */
  folder: string;
  /** PuTTY placeholders: &H &P &Y &M &D &T, plus &S for the session name. */
  name: string;
  /** The saved session's name for &S; empty for a quick connect. */
  session: string;
  append: boolean;
  header: boolean;
}

export async function sessionLogDefaultDir(): Promise<string> {
  return invoke<string>('session_log_default_dir');
}

/** A terminal's file log, while it is writing or paused. */
export interface SessionLogStatus {
  path: string;
  paused: boolean;
}

export function sessionLogStatus(stream: string): Promise<SessionLogStatus | null> {
  return invoke<SessionLogStatus | null>('session_log_status', { stream });
}

export function sessionLogPause(stream: string, paused: boolean): Promise<SessionLogStatus | null> {
  return invoke<SessionLogStatus | null>('session_log_pause', { stream, paused });
}

/** Stops the log and closes its file; false when there was none. */
export function sessionLogStop(stream: string): Promise<boolean> {
  return invoke<boolean>('session_log_stop', { stream });
}

export async function sshConnect(params: SshConnectParams): Promise<string> {
  // Without the tunnel when the person chose to connect straight.
  const wireguard = params.wireguard && (await ensureWireguard(params.wireguard)) ? params.wireguard : null;
  return invoke<string>('ssh_connect', {
    id: params.id,
    host: params.host,
    port: params.port,
    username: params.username,
    authMethod: params.authMethod,
    password: params.password,
    keyPath: params.keyPath,
    keyPassphrase: params.keyPassphrase,
    keyId: params.keyId ?? null,
    passwordRef: params.passwordRef ?? null,
    keyPassphraseRef: params.keyPassphraseRef ?? null,
    cols: params.cols,
    rows: params.rows,
    jumpChain: params.jumpChain ?? null,
    proxy: params.proxy ?? null,
    shell: params.shell?.trim() ? params.shell.trim() : null,
    injectColors: params.injectColors ?? null,
    showLoginMessage: params.showLoginMessage ?? null,
    sessionLog: params.sessionLog ?? null,
    tryAgentKeys: params.tryAgentKeys ?? null,
    sshOptions: params.sshOptions ?? null,
    sessionId: params.sessionId ?? null,
    tmux: params.tmux ?? null,
    x11Desktop: params.x11Desktop ?? null,
    terminal: params.terminal ?? null,
    wireguard,
  });
}

/** Which addresses answer from here: a plain TCP connect to each, all at
 *  once. Milliseconds per address, or null when it did not answer. */
export async function sshProbeAddresses(targets: { host: string; port: number }[], timeoutMs = 1500): Promise<(number | null)[]> {
  return invoke<(number | null)[]>('ssh_probe_addresses', { targets, timeoutMs });
}

/** What the tmux picker chose while connection `id` opened: a name, null
 *  for no tmux; undefined when nothing was asked. */
export async function sshTmuxChosen(id: string): Promise<string | null | undefined> {
  const r = await invoke<{ name: string | null } | null>('ssh_tmux_chosen', { connectionId: id });
  return r ? r.name : undefined;
}

/** The server's tmux sessions, on a tab's connection. */
export async function sshTmuxList(connectionId: string): Promise<TmuxListed[]> {
  return invoke<TmuxListed[]>('ssh_tmux_list', { connectionId });
}

/** The tmux start script, for a WSL tab to run with `wsl.exe -e sh -c`. */
export async function tmuxStartScript(start: TmuxStart): Promise<string> {
  return invoke<string>('tmux_start_script', { start });
}

/** The windows of a tab's tmux session, as its view sees them. */
export async function sshTmuxWindows(connectionId: string, start: TmuxStart): Promise<TmuxWindow[]> {
  return invoke<TmuxWindow[]>('ssh_tmux_windows', { connectionId, name: start.name, view: start.view ?? '' });
}

/** Switch, rename, close or open a window in a tab's tmux view; a new
 *  window runs `shell`, the session's login shell, as the first one does. */
export async function sshTmuxWindow(connectionId: string, start: TmuxStart, op: TmuxWindowOp, shell?: string | null): Promise<void> {
  return invoke('ssh_tmux_window', { connectionId, name: start.name, view: start.view ?? '', op, shell: shell ?? null });
}

/** End a tab's tmux session for good, with the ones other tabs joined it as. */
export async function sshTmuxKill(connectionId: string, name: string): Promise<void> {
  return invoke('ssh_tmux_kill', { connectionId, name });
}

export async function sshSend(connectionId: string, data: number[]): Promise<void> {
  return invoke('ssh_send', { connectionId, data });
}

/** Signal the backend that this terminal's data listener is attached, so it
 *  flushes buffered output (motd/banner) emitted before mount. */
export async function sshReady(connectionId: string): Promise<void> {
  return invoke('ssh_ready', { connectionId });
}

/** Host-key verification request emitted by the backend (`ssh-hostkey-prompt`). */
export interface HostKeyPrompt {
  promptId: string;
  host: string;
  port: number;
  fingerprint: string;
  keyType: string;
  /** true = the stored key changed (possible MITM); false = unknown host (TOFU). */
  changed: boolean;
  oldFingerprint?: string | null;
  /** VisualHostKey: the key's randomart. */
  randomart?: string | null;
  /** VerifyHostKeyDNS: whether a matching SSHFP record was found. */
  dnsMatch?: boolean | null;
  /** Where Accept and save keeps the key: with the saved session (synced)
   *  or in this device's own list. */
  saveTo?: 'session' | 'device';
}

/** Report the user's answer to a host-key prompt: Don't connect
 *  (`accept` false), Accept once (`save` false) or Accept and save. */
export async function sshHostkeyResponse(promptId: string, accept: boolean, save = true): Promise<void> {
  return invoke('ssh_hostkey_response', { promptId, accept, save });
}

export async function sshResize(connectionId: string, cols: number, rows: number): Promise<void> {
  return invoke('ssh_resize', { connectionId, cols, rows });
}

export async function sshDisconnect(connectionId: string): Promise<void> {
  return invoke('ssh_disconnect', { connectionId });
}

export async function sshListConnections(): Promise<ConnectionInfo[]> {
  return invoke<ConnectionInfo[]>('ssh_list_connections');
}

/** What a connection's key exchange settled on. The MACs are empty for a
 *  cipher that carries its own (GCM, ChaCha20-Poly1305). */
export interface NegotiatedAlgorithms {
  kex: string;
  host_key: string;
  cipher: string;
  mac_out: string;
  mac_in: string;
  compression_out: string;
  compression_in: string;
}

/** `null` before the first key exchange, or on a shared (ControlMaster) connection. */
export async function sshConnectionAlgorithms(connectionId: string): Promise<NegotiatedAlgorithms | null> {
  return invoke<NegotiatedAlgorithms | null>('ssh_connection_algorithms', { connectionId });
}

export async function sshDetectOs(connectionId: string): Promise<string> {
  return invoke<string>('ssh_detect_os', { connectionId });
}

export type KeyFileKind = 'private_key' | 'public_key' | 'not_a_key' | 'not_found';

export interface KeyCandidate {
  path: string;
  name: string;
  algo?: string | null;
}

export interface KeyFileInfo {
  path: string;
  kind: KeyFileKind;
  algo?: string | null;
  comment?: string | null;
  encrypted: boolean;
  suggestedPrivateKey?: KeyCandidate | null;
  siblingPrivateKeys: KeyCandidate[];
}

/** Inspect a key-file path to detect wrong-file selections (e.g. a public key)
 *  and surface private-key suggestions from the same folder. */
export async function inspectKeyFile(path: string): Promise<KeyFileInfo> {
  return invoke<KeyFileInfo>('inspect_key_file', { path });
}
