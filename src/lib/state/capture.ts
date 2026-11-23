/**
 * Screenshots of one session pane: the terminal's text (drawn from its
 * buffer) or a remote desktop's picture, saved as a PNG or copied to the
 * clipboard. Nothing else of the window is in it. Desktop only.
 *
 * Each pane registers how to draw itself under its tab's id, as the
 * terminal's find does; the terminal bar's camera button, the tab menu and
 * the two shortcuts all ask through here.
 */
import { save } from '@tauri-apps/plugin-dialog';
import { screenshotCopy, screenshotSave } from '#lib/ipc/screenshot.js';
import { screenshotName } from '#lib/terminal/snapshot.js';
import { isMobile } from '#lib/platform.js';
import { addToast } from './toasts.svelte';
import { t } from './i18n.svelte';

export type CaptureMode = 'copy' | 'save';

/** Draws the pane as it is now; null when there is nothing to show yet. */
export type Capturer = () => HTMLCanvasElement | null;

const capturers = new Map<string, Capturer>();

export function registerCapture(tabId: string, capturer: Capturer): () => void {
	if (isMobile()) return () => {};
	capturers.set(tabId, capturer);
	return () => {
		if (capturers.get(tabId) === capturer) capturers.delete(tabId);
	};
}

export function canCapture(tabId: string): boolean {
	return capturers.has(tabId);
}

function toBase64(canvas: HTMLCanvasElement): Promise<string> {
	return new Promise((resolve, reject) => {
		canvas.toBlob((blob) => {
			if (!blob) return reject(new Error(t('capture.failed_empty')));
			const reader = new FileReader();
			reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ''));
			reader.onerror = () => reject(reader.error);
			reader.readAsDataURL(blob);
		}, 'image/png');
	});
}

let busy = false;

/** Take the pane's picture and copy or save it, saying how it went. */
export async function captureTab(tabId: string, mode: CaptureMode, name: string): Promise<void> {
	if (busy) return;
	const capturer = capturers.get(tabId);
	if (!capturer) {
		addToast(t('capture.unavailable'), 'info');
		return;
	}
	busy = true;
	try {
		// Drawn before the save dialog opens, so the picture is of the moment
		// the button was pressed.
		const canvas = capturer();
		if (!canvas || canvas.width === 0 || canvas.height === 0) {
			addToast(t('capture.unavailable'), 'info');
			return;
		}
		const png = await toBase64(canvas);
		if (mode === 'copy') {
			await screenshotCopy(png);
			addToast(t('capture.copied'), 'success');
			return;
		}
		const path = await save({ defaultPath: screenshotName(name), filters: [{ name: 'PNG', extensions: ['png'] }] });
		if (!path) return;
		const file = /\.png$/i.test(path) ? path : `${path}.png`;
		await screenshotSave(file, png);
		addToast(t('capture.saved', { path: file }), 'success');
	} catch (e) {
		addToast(t('capture.failed', { error: e instanceof Error ? e.message : String(e) }), 'error', 6000);
	} finally {
		busy = false;
	}
}
