import assert from 'node:assert/strict';
import test from 'node:test';
import en from '../src/lib/i18n/locales/en.json' with { type: 'json' };
import {
	DEFAULT_SKIPPED_KEYS, LEGACY_PREFERENCE_KEYS, PREFERENCE_CATALOG, decodePreferences, encodePreferences,
} from '../src/lib/sync/preference-catalog.ts';
import {
	incomingPreferences, portablePreferences, PREFERENCE_KEYS, readPreferenceJournal,
	reconcilePreferences, setPreferenceIncluded,
} from '../src/lib/sync/preferences.ts';

function journal(pending = {}, initialized = false) {
	return { initialized, pending, remote: {}, skipped: [...DEFAULT_SKIPPED_KEYS], catalogVersion: 3, seed: [] };
}

test('only allowlisted preferences leave the device', () => {
	assert.deepEqual(portablePreferences({ theme: 'system', locale: 'cs', terminalCursorStyle: 'bar',
		fileEditorCommand: '/example', openrouterApiKey: 'example' }),
	{ theme: 'system', locale: 'cs', terminalCursorStyle: 'bar' });
	assert.deepEqual(portablePreferences({ locale: 'unknown', theme: null }), {});
});

test('offline changes stay queued and rebase onto another device’s edits', async () => {
	const state = journal({ terminalCursorStyle: 'block' }, true);
	await assert.rejects(reconcilePreferences(state, async () => { throw Error('offline'); },
		() => ({}), () => true, () => {}), /offline/);
	assert.deepEqual(state.pending, { terminalCursorStyle: 'block' });

	const snapshots = [
		{ revision: 1, values: { theme: 'dark' }, conflict: false },
		{ revision: 2, values: { theme: 'light' }, conflict: true },
		{ revision: 3, values: { theme: 'light', terminalCursorStyle: 'block' }, conflict: false },
	];
	const requests = [];
	const latest = await reconcilePreferences(state, async (patch) => {
		requests.push(patch);
		return snapshots.shift();
	}, () => ({}), () => true, () => {});
	assert.deepEqual(requests, [
		undefined,
		{ revision: 1, changes: { terminalCursorStyle: 'block' } },
		{ revision: 2, changes: { terminalCursorStyle: 'block' } },
	]);
	assert.deepEqual(latest, { theme: 'light', terminalCursorStyle: 'block' });
	assert.deepEqual(state.pending, {});
});

test('losing the first seed race does not upload local defaults over another device', async () => {
	const state = journal({ locale: 'cs' });
	const snapshots = [
		{ revision: 0, values: {}, conflict: false },
		{ revision: 1, values: { theme: 'dark', locale: 'en' }, conflict: true },
		{ revision: 2, values: { theme: 'dark', locale: 'cs' }, conflict: false },
	];
	const requests = [];
	const latest = await reconcilePreferences(state, async (patch) => {
		requests.push(patch);
		return snapshots.shift();
	}, () => ({ theme: 'light', locale: 'en' }), () => true, () => {});
	assert.deepEqual(requests[2], { revision: 1, changes: { locale: 'cs' } });
	assert.deepEqual(latest, { theme: 'dark', locale: 'cs' });
});

test('a newer local edit survives an in-flight acknowledgment', async () => {
	const state = journal({ locale: 'cs' }, true);
	const latest = await reconcilePreferences(state, async (patch) => {
		if (!patch) return { revision: 1, values: { locale: 'en' }, conflict: false };
		state.pending.locale = 'de';
		return { revision: 2, values: { locale: 'cs' }, conflict: false };
	}, () => ({}), () => true, () => {});
	assert.deepEqual(state.pending, { locale: 'de' });
	assert.deepEqual(incomingPreferences({}, latest, state.pending, { locale: 'en' }), {});
	assert.deepEqual(incomingPreferences({ theme: 'light' }, {}, {}, { theme: 'dark' }), { theme: 'dark' });
});

test('a response from a previous account cannot apply or acknowledge edits', async () => {
	const state = journal({ theme: 'system' }, true);
	const latest = await reconcilePreferences(state, async () => ({
		revision: 1, values: { theme: 'dark' }, conflict: false,
	}), () => ({}), () => false, () => assert.fail('inactive account was persisted'));
	assert.equal(latest, undefined);
	assert.deepEqual(state.pending, { theme: 'system' });
});

test('saved choices survive a restart and existing journals still sync every preference', () => {
	const legacy = { initialized: true, pending: { theme: 'light' }, remote: { theme: 'dark', locale: 'cs' } };
	assert.deepEqual(readPreferenceJournal(legacy), { ...legacy, skipped: DEFAULT_SKIPPED_KEYS, catalogVersion: 3, seed: [] });
	const saved = JSON.parse(JSON.stringify({ ...legacy, catalogVersion: 3, skipped: ['theme', 'theme', 'unknown'] }));
	assert.deepEqual(readPreferenceJournal(saved), {
		initialized: true, pending: {}, remote: { locale: 'cs' }, skipped: ['theme'], catalogVersion: 3, seed: [],
	});
});

test('a skipped preference is excluded from local edits, the first upload, and remote values', async () => {
	const state = journal();
	setPreferenceIncluded(state, 'theme', false);
	const local = { theme: 'dark', locale: 'cs' };
	assert.deepEqual(portablePreferences(local, state.skipped), { locale: 'cs' });
	const requests = [];
	const latest = await reconcilePreferences(state, async (patch) => {
		requests.push(patch);
		return patch
			? { revision: 1, values: { theme: 'light', locale: 'cs' }, conflict: false }
			: { revision: 0, values: {}, conflict: false };
	}, () => local, () => true, () => {});
	assert.deepEqual(requests, [undefined, { revision: 0, changes: { locale: 'cs' } }]);
	assert.deepEqual(incomingPreferences(state.remote, latest, state.pending, { theme: 'dark' }), { locale: 'cs' });
});

test('skipping during a conflicting upload drops the queued field and leaves other edits syncing', async () => {
	const state = journal({ theme: 'light', locale: 'cs' }, true);
	state.remote = { theme: 'dark' };
	const requests = [];
	const latest = await reconcilePreferences(state, async (patch) => {
		requests.push(patch);
		if (!patch) return { revision: 1, values: { theme: 'dark' }, conflict: false };
		if (patch.revision === 1) {
			setPreferenceIncluded(state, 'theme', false);
			return { revision: 2, values: { theme: 'system' }, conflict: true };
		}
		return { revision: 3, values: { locale: 'cs', theme: 'system' }, conflict: false };
	}, () => ({}), () => true, () => {});
	assert.deepEqual(requests, [
		undefined,
		{ revision: 1, changes: { theme: 'light', locale: 'cs' } },
		{ revision: 2, changes: { locale: 'cs' } },
	]);
	assert.deepEqual(state.pending, {});
	assert.deepEqual(incomingPreferences(state.remote, latest, state.pending, { theme: 'dark' }), { locale: 'cs' });

	setPreferenceIncluded(state, 'theme', true);
	const resumed = await reconcilePreferences(state, async (patch) => {
		assert.equal(patch, undefined);
		return { revision: 3, values: { locale: 'cs', theme: 'system' }, conflict: false };
	}, () => ({ theme: 'light' }), () => true, () => {});
	assert.deepEqual(incomingPreferences(latest, resumed, state.pending, { theme: 'dark' }), {
		theme: 'system', locale: 'cs',
	});
});

test('skipping every preference leaves sync idle', async () => {
	const state = journal();
	for (const key of PREFERENCE_KEYS) setPreferenceIncluded(state, key, false);
	assert.equal(await reconcilePreferences(state, async () => assert.fail('unexpected request'),
		() => ({}), () => true, () => {}), undefined);
	assert.equal(state.initialized, false);
});

test('the preference catalog uses localized labels and preserves typed values across the wire', () => {
	for (const definition of Object.values(PREFERENCE_CATALOG)) {
		assert.equal(typeof en[definition.label], 'string', definition.label);
	}
	const local = {
		theme: 'light', compactLayout: false, fontSize: 18, uiZoom: 1.25, mobileBarKeys: null,
		themeId: undefined, terminalHighlightKinds: ['error'], keyboardShortcuts: { 'zoom.in': ['Mod+='] },
		rdpFullscreenButtonAt: { x: 0.2, y: 0.8 }, defaultShell: '/example', pendingTursoApiToken: 'example',
	};
	const encoded = encodePreferences(local);
	assert.deepEqual(encoded, {
		theme: 'light', compactLayout: 'false', fontSize: '18', uiZoom: '1.25', mobileBarKeys: 'null',
		themeId: '', terminalHighlightKinds: '["error"]', keyboardShortcuts: '{"zoom.in":["Mod+="]}',
		rdpFullscreenButtonAt: '{"x":0.2,"y":0.8}',
	});
	const { defaultShell, pendingTursoApiToken, ...expected } = local;
	assert.deepEqual(decodePreferences(encoded), expected);
	assert.deepEqual(portablePreferences({
		compactLayout: '"true"', fontSize: '4000', mobileBarKeys: '["newKey"]',
		rdpFullscreenButtonAt: '{"x":-1,"y":0}', terminalHighlightCustom: 'á'.repeat(9000), fileEditorCommand: 'example',
	}), {});
});

test('an old server still syncs legacy edits and keeps new selections queued for an upgrade', async () => {
	const state = journal({ locale: 'cs', fontSize: '18' }, true);
	setPreferenceIncluded(state, 'fontSize', true);
	const legacyRequest = [];
	await reconcilePreferences(state, async (patch) => {
		legacyRequest.push(patch);
		return { revision: patch ? 2 : 1, values: { locale: patch ? 'cs' : 'en' }, conflict: false };
	}, () => ({}), () => true, () => {});
	assert.deepEqual(legacyRequest, [undefined, { revision: 1, changes: { locale: 'cs' } }]);
	assert.deepEqual(state.pending, { fontSize: '18' });
	assert.deepEqual(state.seed, ['fontSize']);

	const supported = [...LEGACY_PREFERENCE_KEYS, 'fontSize'];
	const upgradedRequest = [];
	const latest = await reconcilePreferences(state, async (patch) => {
		upgradedRequest.push(patch);
		return { revision: patch ? 3 : 2, supported, values: patch ? { locale: 'cs', fontSize: '18' } : { locale: 'cs' }, conflict: false };
	}, () => ({}), () => true, () => {});
	assert.deepEqual(upgradedRequest, [undefined, { revision: 2, changes: { fontSize: '18' } }]);
	assert.deepEqual(latest, { locale: 'cs', fontSize: '18' });
	assert.deepEqual(state.pending, {});
});

test('first opt-in only seeds missing values and preserves a value another device uploads first', async () => {
	const state = journal({}, true);
	setPreferenceIncluded(state, 'fontSize', true);
	setPreferenceIncluded(state, 'compactLayout', true);
	const sent = [];
	const supported = [...LEGACY_PREFERENCE_KEYS, 'fontSize', 'compactLayout'];
	const latest = await reconcilePreferences(state, async (patch) => {
		sent.push(patch);
		if (!patch) return { revision: 2, values: { compactLayout: 'true' }, supported, conflict: false };
		return { revision: 3, values: { fontSize: '22', compactLayout: 'true' }, supported, conflict: true };
	}, () => ({ fontSize: '18', compactLayout: 'false' }), () => true, () => {});
	assert.deepEqual(sent, [undefined, { revision: 2, changes: { fontSize: '18' } }]);
	assert.deepEqual(latest, { fontSize: '22', compactLayout: 'true' });
	assert.deepEqual(state.seed, []);
});

test('an expanded journal keeps the newly supported log folder local by default', () => {
	const previous = readPreferenceJournal({ catalogVersion: 2, skipped: ['theme'] });
	assert.deepEqual(previous.skipped, ['theme', 'sessionLogFolder']);
	setPreferenceIncluded(previous, 'sessionLogFolder', true);
	assert.deepEqual(readPreferenceJournal(previous).skipped, ['theme']);
});

test('a missing log folder can be held back while other preference edits are sent', async () => {
	const state = journal({ sessionLogFolder: '/example/missing', locale: 'cs' }, true);
	setPreferenceIncluded(state, 'sessionLogFolder', true);
	const sent = [];
	await reconcilePreferences(state, async (patch) => {
		sent.push(patch);
		return {
			revision: patch ? 2 : 1, supported: ['sessionLogFolder', 'locale'], conflict: false,
			values: { locale: patch ? 'cs' : 'en' },
		};
	}, () => ({}), () => true, () => {}, ({ sessionLogFolder: _unavailable, ...other }) => other);
	assert.deepEqual(sent, [undefined, { revision: 1, changes: { locale: 'cs' } }]);
	assert.deepEqual(state.pending, { sessionLogFolder: '/example/missing' });
	setPreferenceIncluded(state, 'sessionLogFolder', false);
	assert.deepEqual(state.pending, {});
});
