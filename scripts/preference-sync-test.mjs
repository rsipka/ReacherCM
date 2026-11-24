import assert from 'node:assert/strict';
import test from 'node:test';
import { incomingPreferences, portablePreferences, reconcilePreferences } from '../src/lib/sync/preferences.ts';

function journal(pending = {}, initialized = false) {
	return { initialized, pending, remote: {} };
}

test('only allowlisted preferences leave the device', () => {
	assert.deepEqual(portablePreferences({ theme: 'system', locale: 'cs', terminalCursorStyle: 'bar',
		themeId: 'custom', sessionLogFolder: '/example', openrouterApiKey: 'example' }),
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
