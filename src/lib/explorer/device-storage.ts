/**
 * Android's "All files access", which the file manager's phone side needs to
 * list the shared storage (Download, DCIM, other apps' folders) by path. Without
 * it the side still works in the app's own folder (Download/ReacherCM). The
 * activity (src-tauri/android/MainActivity.kt) answers through `ReachStorage`;
 * anywhere else there is nothing to ask for.
 */

interface StorageBridge {
	granted(): boolean;
	request(): void;
}

function bridge(): StorageBridge | undefined {
	return (globalThis as { ReachStorage?: StorageBridge }).ReachStorage;
}

/** Whether this device has a storage permission to ask for at all. */
export function canAskStorage(): boolean {
	return !!bridge();
}

/** Whether the shared storage can be listed: true where nothing is asked. */
export function storageGranted(): boolean {
	try {
		return bridge()?.granted() ?? true;
	} catch {
		return true;
	}
}

/** Open the system screen that grants it. */
export function requestStorage(): void {
	try {
		bridge()?.request();
	} catch {
		// An older app without the bridge: nothing to open.
	}
}
