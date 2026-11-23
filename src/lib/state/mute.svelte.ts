import { invoke } from '@tauri-apps/api/core';
import { getSettings, updateSetting } from '#lib/state/settings.svelte.js';

/**
 * The top bar's mute switch. The page's own sounds (the notification chime,
 * the terminal bell) read the `muted` setting where they play; a remote
 * desktop's audio is played by the backend, which is told here.
 */
export function isMuted(): boolean {
	return getSettings().muted;
}

export function toggleMute(): void {
	setMuted(!getSettings().muted);
}

export function setMuted(muted: boolean): void {
	updateSetting('muted', muted);
	syncMuteToBackend();
}

/** Tell the backend the current state: at start, and after every change. */
export function syncMuteToBackend(): void {
	invoke('set_audio_muted', { muted: getSettings().muted }).catch(() => {});
}
