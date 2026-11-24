import { invoke } from '@tauri-apps/api/core';
import type { SecretRef } from './pwmanager';
import { ensureSessionWireguard } from '#lib/state/wireguard.svelte.js';

/** How a saved password reaches WinBox: on its command line (it logs in by
 *  itself), on the clipboard for 45 seconds, or not at all. */
export type WinboxPasswordMode = 'argument' | 'clipboard' | 'none';

/** WinBox's own port. */
export const WINBOX_PORT = 8291;

export interface WinboxLaunchParams {
  host: string;
  port: number;
  username?: string | null;
  password?: string | null;
  passwordRef?: SecretRef | null;
  passwordMode: WinboxPasswordMode;
  /** Settings' path; empty to look for WinBox. */
  program?: string | null;
}

export interface WinboxLaunched {
  program: string;
  /** The password was put on the clipboard. */
  copied: boolean;
}

/** Where WinBox is (Settings' path if usable, else found), or null. */
export async function winboxFind(program?: string | null): Promise<string | null> {
  return invoke<string | null>('winbox_find', { program: program || null });
}

/** Start WinBox on a saved session; desktop only. */
export async function winboxLaunch(params: WinboxLaunchParams, sessionId: string | null): Promise<WinboxLaunched> {
  await ensureSessionWireguard(sessionId, params.host, params.port);
  return invoke<WinboxLaunched>('winbox_launch', { params, sessionId });
}
