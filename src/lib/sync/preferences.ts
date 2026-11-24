export const PREFERENCE_VALUES = {
	theme: ['dark', 'light', 'system'],
	locale: ['en', 'cs', 'de', 'es', 'fr', 'el', 'it', 'bg', 'ru', 'zh'],
	terminalCursorStyle: ['block', 'underline', 'bar'],
} as const;

export type PreferenceKey = keyof typeof PREFERENCE_VALUES;
export type PreferenceValues = { [K in PreferenceKey]: typeof PREFERENCE_VALUES[K][number] };
export type Preferences = Partial<PreferenceValues>;

export const PREFERENCE_KEYS = Object.keys(PREFERENCE_VALUES) as PreferenceKey[];

export function portablePreferences(values: unknown): Preferences {
	if (typeof values !== 'object' || values === null) return {};
	const preferences: Preferences = {};
	for (const key of PREFERENCE_KEYS) {
		const value: unknown = Reflect.get(values, key);
		if ((PREFERENCE_VALUES[key] as readonly unknown[]).includes(value)) {
			Object.assign(preferences, { [key]: value });
		}
	}
	return preferences;
}

export function acknowledgePreferences(pending: Preferences, sent: Preferences): void {
	for (const key of PREFERENCE_KEYS) {
		if (pending[key] === sent[key]) delete pending[key];
	}
}

export function incomingPreferences(
	previous: Preferences, current: Preferences, pending: Preferences, defaults: Preferences
): Preferences {
	const applied: Preferences = {};
	for (const key of PREFERENCE_KEYS) {
		if (pending[key] !== undefined) continue;
		const value = current[key] ?? (previous[key] !== undefined ? defaults[key] : undefined);
		if (value !== undefined) Object.assign(applied, { [key]: value });
	}
	return applied;
}

export interface PreferenceSnapshot {
	revision: number;
	values: Preferences;
	conflict: boolean;
}

export interface PreferencePatch {
	revision: number;
	changes: Preferences;
}

export interface PreferenceJournal {
	initialized: boolean;
	pending: Preferences;
	remote: Preferences;
}

/** Rebase the local field queue onto the most recent server revision. */
export async function reconcilePreferences(
	journal: PreferenceJournal,
	request: (patch?: PreferencePatch) => Promise<PreferenceSnapshot>,
	local: () => Preferences,
	active: () => boolean,
	persist: () => void,
): Promise<Preferences | undefined> {
	let snapshot = await request();
	if (!active()) return;
	if (!journal.initialized && snapshot.revision === 0) {
		// A seed is only appropriate while the account is empty. If another
		// device seeds it first, retry our actual edits, not the local defaults.
		const seed = { ...local(), ...journal.pending };
		snapshot = await request({ revision: 0, changes: seed });
		if (!active()) return;
		if (!snapshot.conflict) acknowledgePreferences(journal.pending, seed);
	}
	journal.initialized = true;
	persist();

	for (let attempt = 0; attempt < 3 && Object.keys(journal.pending).length > 0; attempt++) {
		const sent = { ...journal.pending };
		snapshot = await request({ revision: snapshot.revision, changes: sent });
		if (!active()) return;
		if (!snapshot.conflict) {
			acknowledgePreferences(journal.pending, sent);
			break;
		}
	}
	return portablePreferences(snapshot.values);
}
