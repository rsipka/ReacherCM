import { invoke } from '@tauri-apps/api/core';
import { isMac, isMobile, isWebKit } from '#lib/platform.js';
import { getSettings } from '#lib/state/settings.svelte.js';
import { recordNotification } from '#lib/state/notification-history.svelte.js';
import { routeToast } from '#lib/state/notify-rules.js';
import { windowSeen } from '#lib/terminal/bell.js';
import { APP_NAME } from '#lib/brand.js';

export type ToastType = 'info' | 'success' | 'warning' | 'error';

export interface Toast {
	id: string;
	message: string;
	type: ToastType;
	duration: number;
	dismissing: boolean;
	/** A button on the toast, such as Undo; pressing it closes the toast. */
	action?: ToastAction;
}

export interface ToastAction {
	label: string;
	run: () => void;
}

let toasts = $state<Toast[]>([]);

const timers = new Map<string, ReturnType<typeof setTimeout>>();

const VOLUME = 0.4;

/**
 * The toast's chime. On Linux, Reach plays it itself: an `<audio>` element
 * there goes through WebKitGTK to GStreamer, and where GStreamer has no audio
 * output (common on Arch, and always in the AppImage) the page's process
 * crashed on the first toast, leaving the window frozen (issue #87).
 */
function playSound(): void {
	if (!getSettings().playSounds || getSettings().muted) return;
	if (isWebKit() && !isMac() && !isMobile()) {
		invoke('play_notification_sound', { volume: VOLUME }).catch(() => {});
		return;
	}
	try {
		const audio = new Audio('/sounds/notification.wav');
		audio.volume = VOLUME;
		void audio.play().catch(() => {});
	} catch {
		// Audio playback not available
	}
}

/** The toast as a system notification (Settings > General > System
 *  notifications). Permission was asked when the setting was turned on. */
async function notifySystem(message: string): Promise<void> {
	try {
		const n = await import('@tauri-apps/plugin-notification');
		if (!(await n.isPermissionGranted())) return;
		n.sendNotification({ title: APP_NAME, body: message });
	} catch {
		// Notifications not available
	}
}

export function getToasts(): Toast[] {
	return toasts;
}

export function addToast(
	message: string,
	type: ToastType = 'info',
	duration: number = 3000,
	action?: ToastAction
): Toast {
	const id = crypto.randomUUID();

	const toast: Toast = { id, message, type, duration, dismissing: false, action };
	recordNotification(message, type);

	const route = routeToast(getSettings().systemNotifications, windowSeen(), !!action);
	if (route.system) void notifySystem(message);
	// Sent to the system only: the system shows and sounds it, so nothing
	// here. The toast is still returned, for a caller that dismisses it.
	if (!route.inApp) return toast;

	toasts.push(toast);
	// The system plays its own sound for its notification.
	if (!route.system) playSound();

	if (duration > 0) {
		const timer = setTimeout(() => {
			dismissToast(id);
		}, duration);
		timers.set(id, timer);
	}

	return toast;
}

export function dismissToast(id: string): void {
	const toast = toasts.find((t) => t.id === id);
	if (toast && !toast.dismissing) {
		toast.dismissing = true;
		setTimeout(() => removeToast(id), 250);
	}
}

export function removeToast(id: string): void {
	const timer = timers.get(id);
	if (timer) {
		clearTimeout(timer);
		timers.delete(id);
	}

	const index = toasts.findIndex((t) => t.id === id);
	if (index !== -1) {
		toasts.splice(index, 1);
	}
}

export function clearAllToasts(): void {
	for (const timer of timers.values()) {
		clearTimeout(timer);
	}
	timers.clear();
	toasts.length = 0;
}
