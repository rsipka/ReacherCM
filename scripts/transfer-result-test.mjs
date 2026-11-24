import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

const path = new URL('../src/lib/explorer/run-transfer.ts', import.meta.url);
const { outputText } = ts.transpileModule(readFileSync(path, 'utf8'), {
	compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 }
});
const listeners = new Map();
const seen = [];
let stop;
const modules = {
	'@tauri-apps/api/event': {
		async listen(event, listener) {
			listeners.set(event, listener);
			return () => listeners.delete(event);
		}
	},
	'#lib/ipc/sftp.js': {
		async sftpTransferCancel() { seen.push('stop requested'); }
	},
	'#lib/state/transfers.svelte.js': {
		addTransfer: (_id, _name, _size, _kind, cancel) => { stop = cancel; },
		completeTransfer: () => seen.push('completed'),
		failTransfer: (_id, error) => seen.push(error),
		removeTransfer: () => seen.push('removed'),
		updateTransferProgress: (_id, bytes) => seen.push(`progress ${bytes}`)
	}
};
const api = {};
new Function('exports', 'require', 'crypto', outputText)(api, (name) => modules[name], { randomUUID });

async function transfer(event, payload, requestStop = true) {
	let registration;
	const registered = new Promise((resolve) => { registration = resolve; });
	let started;
	const launching = new Promise((resolve) => { started = resolve; });
	let id;
	stop = undefined;
	const result = api.runTransfer({
		kind: 'uploading', connectionId: 'local-test', filename: 'report', totalBytes: 10,
		start(transferId) { id = transferId; started(); return registered; },
		onId() {
			if (requestStop) stop();
			listeners.get(`transfer-${event}-${id}`)({ payload });
		}
	});
	await launching;
	assert.equal(stop, undefined, 'stop must wait until the backend registers the ID');
	listeners.get(`transfer-progress-${id}`)({ payload: { bytesTransferred: 4, totalBytes: 10, percent: 40 } });
	registration();
	return result;
}

await assert.rejects(transfer('error', 'Remote filesystem is full'), /Remote filesystem is full/);
assert.deepEqual(seen.splice(0), ['progress 4', 'stop requested', 'Remote filesystem is full']);
assert.equal(await transfer('cancelled'), false);
assert.deepEqual(seen.splice(0), ['progress 4', 'stop requested', 'removed']);
assert.equal(await transfer('complete'), false);
assert.deepEqual(seen.splice(0), ['progress 4', 'stop requested', 'removed']);
assert.equal(await transfer('complete', undefined, false), true);
assert.deepEqual(seen.splice(0), ['progress 4', 'completed']);
assert.equal(listeners.size, 0);
console.log('Transfer registration and result checks passed');
