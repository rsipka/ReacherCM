import {
	DEFAULT_SKIPPED_KEYS, LEGACY_PREFERENCE_KEYS, PREFERENCE_KEYS, portablePreferences,
	type PreferenceKey, type Preferences,
} from './preference-catalog.ts';

export { PREFERENCE_KEYS, portablePreferences };
export type { PreferenceKey, Preferences };

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
	supported?: string[];
}

export interface PreferencePatch {
	revision: number;
	changes: Preferences;
}

export interface PreferenceJournal {
	initialized: boolean;
	pending: Preferences;
	remote: Preferences;
	skipped: PreferenceKey[];
	catalogVersion: 3;
	seed: PreferenceKey[];
}

export function readPreferenceJournal(value: Partial<Omit<PreferenceJournal, 'catalogVersion'>> & { catalogVersion?: number }): PreferenceJournal {
	const saved = Array.isArray(value.skipped) ? value.skipped : [];
	const savedSeed = Array.isArray(value.seed) ? value.seed : [];
	const skipped = PREFERENCE_KEYS.filter((key) => saved.includes(key)
		|| (value.catalogVersion !== 2 && value.catalogVersion !== 3 && DEFAULT_SKIPPED_KEYS.includes(key))
		|| (value.catalogVersion === 2 && key === 'sessionLogFolder'));
	return {
		initialized: value.initialized === true,
		catalogVersion: 3,
		seed: PREFERENCE_KEYS.filter((key) => savedSeed.includes(key) && !skipped.includes(key)),
		pending: portablePreferences(value.pending, skipped),
		remote: portablePreferences(value.remote, skipped),
		skipped,
	};
}

export function supportedPreferenceKeys(snapshot: PreferenceSnapshot): readonly string[] {
	return snapshot.supported ?? LEGACY_PREFERENCE_KEYS;
}

export function setPreferenceIncluded(journal: PreferenceJournal, key: PreferenceKey, included: boolean): void {
	if (included === !journal.skipped.includes(key)) return;
	if (included) {
		journal.skipped = journal.skipped.filter((skipped) => skipped !== key);
		journal.seed.push(key);
	} else {
		journal.skipped = [...journal.skipped, key];
		journal.seed = journal.seed.filter((seed) => seed !== key);
		delete journal.pending[key];
		delete journal.remote[key];
	}
}

/** Rebase the local field queue onto the most recent server revision. */
export async function reconcilePreferences(
	journal: PreferenceJournal,
	request: (patch?: PreferencePatch) => Promise<PreferenceSnapshot>,
	local: () => Preferences,
	active: () => boolean,
	persist: () => void,
	filterOutgoing: (changes: Preferences) => Preferences = (changes) => changes,
): Promise<Preferences | undefined> {
	if (journal.skipped.length === PREFERENCE_KEYS.length) return;
	let snapshot = await request();
	if (!active() || journal.skipped.length === PREFERENCE_KEYS.length) return;
	for (let attempt = 0; attempt < 3; attempt++) {
		const supported = supportedPreferenceKeys(snapshot);
		const excluded = PREFERENCE_KEYS.filter((key) => journal.skipped.includes(key) || !supported.includes(key));
		const initialAccount = !journal.initialized && snapshot.revision === 0;
		const initial = PREFERENCE_KEYS.filter((key) => initialAccount
			|| (journal.seed.includes(key) && snapshot.values[key] === undefined));
		const seed = initial.length > 0 ? portablePreferences(local(), excluded) : {};
		const sent = filterOutgoing(portablePreferences({
			...Object.fromEntries(initial.map((key) => [key, seed[key]])),
			...journal.pending,
		}, excluded));

		journal.seed = journal.seed.filter((key) => !supported.includes(key) || snapshot.values[key] === undefined);
		if (Object.keys(sent).length === 0) break;
		snapshot = await request({ revision: snapshot.revision, changes: sent });
		if (!active()) return;
		if (!snapshot.conflict) {
			acknowledgePreferences(journal.pending, sent);
			journal.seed = journal.seed.filter((key) => sent[key] === undefined);
			break;
		}
	}
	journal.initialized = true;
	persist();
	const unsupported = PREFERENCE_KEYS.filter((key) => !supportedPreferenceKeys(snapshot).includes(key));
	return portablePreferences(snapshot.values, [...journal.skipped, ...unsupported]);
}
