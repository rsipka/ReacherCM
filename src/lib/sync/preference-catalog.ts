import catalog from './preference-catalog.json' with { type: 'json' };
import type { Settings } from '../state/settings-store.js';

export type PreferenceKey = keyof typeof catalog;
export type Preferences = Partial<Record<PreferenceKey, string>>;
export type PreferenceSettings = Partial<Pick<Settings, PreferenceKey>>;

interface PreferenceDefinition {
	label: string;
	group: string;
	kind: string;
	choices?: string[];
	min?: number;
	max?: number;
	maxBytes?: number;
}

export const PREFERENCE_CATALOG: Record<PreferenceKey, PreferenceDefinition> = catalog;
export const PREFERENCE_KEYS = (Object.keys(catalog) as PreferenceKey[])
	.filter((key) => PREFERENCE_CATALOG[key].kind !== 'local');
export const LEGACY_PREFERENCE_KEYS: readonly PreferenceKey[] = ['locale', 'theme', 'terminalCursorStyle'];
export const DEFAULT_SKIPPED_KEYS = PREFERENCE_KEYS.filter((key) => !LEGACY_PREFERENCE_KEYS.includes(key));

const textEncoder = new TextEncoder();

function stringList(value: unknown, choices?: string[]): value is string[] {
	return Array.isArray(value) && value.length <= (choices?.length ?? 8)
		&& value.every((item) => typeof item === 'string'
			&& (choices ? choices.includes(item) : item.length > 0 && textEncoder.encode(item).length <= 128));
}

function fraction(value: unknown): boolean {
	return typeof value === 'number' && value >= 0 && value <= 1;
}

function validValue(definition: PreferenceDefinition, value: unknown): boolean {
	switch (definition.kind) {
		case 'enum': return typeof value === 'string' && definition.choices!.includes(value);
		case 'text':
		case 'optionalText': return typeof value === 'string' && textEncoder.encode(value).length <= definition.maxBytes!;
		case 'boolean': return typeof value === 'boolean';
		case 'integer':
		case 'number': return typeof value === 'number' && (definition.kind !== 'integer' || Number.isSafeInteger(value))
			&& value >= definition.min! && value <= definition.max!;
		case 'nullableList':
			if (value === null) return true;
			return stringList(value, definition.choices) && value.length > 0;
		case 'list': return stringList(value, definition.choices);
		case 'point': return value === null || (typeof value === 'object' && !Array.isArray(value)
			&& Object.keys(value).length === 2 && fraction(Reflect.get(value, 'x')) && fraction(Reflect.get(value, 'y')));
		case 'shortcuts': return typeof value === 'object' && value !== null && !Array.isArray(value)
			&& Object.entries(value).length <= 128
			&& Object.entries(value).every(([id, keys]) => /^[a-zA-Z0-9._-]{1,64}$/.test(id) && stringList(keys));
		default: return false;
	}
}

function decodeValue(definition: PreferenceDefinition, value: string): unknown {
	if (definition.kind === 'enum' || definition.kind === 'text' || definition.kind === 'optionalText') return value;
	try {
		return JSON.parse(value) as unknown;
	} catch {
		return undefined;
	}
}

/** Validate wire values while keeping their string representation for the queue. */
export function portablePreferences(values: unknown, skipped: readonly PreferenceKey[] = []): Preferences {
	if (typeof values !== 'object' || values === null) return {};
	const preferences: Preferences = {};
	for (const key of PREFERENCE_KEYS) {
		if (skipped.includes(key)) continue;
		const value: unknown = Reflect.get(values, key);
		const definition = PREFERENCE_CATALOG[key];
		if (typeof value === 'string' && textEncoder.encode(value).length <= (definition.maxBytes ?? 16_384)
			&& validValue(definition, decodeValue(definition, value))) {
			preferences[key] = value;
		}
	}
	return preferences;
}

export function encodePreferences(settings: Partial<Settings>): Preferences {
	const encoded: Preferences = {};
	for (const key of PREFERENCE_KEYS) {
		if (!Object.hasOwn(settings, key)) continue;
		const definition = PREFERENCE_CATALOG[key];
		const current = settings[key];
		const value = definition.kind === 'optionalText' ? (current ?? '') : current;
		if (validValue(definition, value)) {
			encoded[key] = typeof value === 'string' ? value : JSON.stringify(value);
		}
	}
	return portablePreferences(encoded);
}

export function decodePreferences(values: Preferences): PreferenceSettings {
	const settings: PreferenceSettings = {};
	for (const [key, value] of Object.entries(portablePreferences(values))) {
		const definition = PREFERENCE_CATALOG[key as PreferenceKey];
		const decoded = definition.kind === 'optionalText' && value === '' ? undefined : decodeValue(definition, value);
		Object.assign(settings, { [key]: decoded });
	}
	return settings;
}
