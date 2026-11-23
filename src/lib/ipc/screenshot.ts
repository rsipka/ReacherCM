import { invoke } from '@tauri-apps/api/core';

/** Write a PNG (base64) to a file the save dialog chose. Desktop only. */
export function screenshotSave(path: string, pngBase64: string): Promise<void> {
	return invoke('screenshot_save', { path, pngBase64 });
}

/** Put a PNG (base64) on the clipboard as a picture. Desktop only. */
export function screenshotCopy(pngBase64: string): Promise<void> {
	return invoke('screenshot_copy', { pngBase64 });
}
