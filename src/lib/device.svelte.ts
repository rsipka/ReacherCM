/**
 * What Reach's Android activity knows about the device that the WebView does
 * not: whether a hardware keyboard is attached, and whether Samsung DeX is on.
 * The page asks once at start (window.ReachDevice) and is told again whenever
 * either changes (__reachDevice), as a keyboard is plugged in or DeX starts.
 * Elsewhere none of this exists and both stay false.
 */

type DeviceState = { keyboard: boolean; dex: boolean };

type DeviceWindow = Window & {
	ReachDevice?: { get(): string };
	__reachDevice?: (json: string) => void;
};

let state = $state<DeviceState>({ keyboard: false, dex: false });

function apply(json: string): void {
	try {
		const parsed = JSON.parse(json) as Partial<DeviceState>;
		state = { keyboard: parsed.keyboard === true, dex: parsed.dex === true };
	} catch {
		// Not the activity's answer.
	}
}

/** A hardware keyboard is attached (Bluetooth, USB or a DeX keyboard). */
export function hasHardwareKeyboard(): boolean {
	return state.keyboard;
}

/** Samsung DeX is on. */
export function inDex(): boolean {
	return state.dex;
}

export function initDevice(): void {
	if (typeof window === 'undefined') return;
	const w = window as DeviceWindow;
	w.__reachDevice = apply;
	try {
		const now = w.ReachDevice?.get();
		if (typeof now === 'string') apply(now);
	} catch {
		// Not Reach's Android activity.
	}
}
