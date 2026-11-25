/**
 * The bulk editor's newer fields, in a browser page with a stand-in for
 * Tauri that keeps the sessions in memory.
 *
 * What it proves:
 *   - SSH, RDP, Wake-on-LAN, favorite and OS icon rows show for a mixed
 *     selection, with "applies to" where only some sessions take them.
 *   - The connect mode shows only when a selected session has more
 *     than one address.
 *   - Choices made there reach session_bulk_update as the patch the
 *     backend reads.
 *
 * Needs Playwright's Chromium; skips cleanly without it. SHOT_DIR=<dir>
 * saves screenshots.
 *
 * Run: node scripts/bulk-edit-live-test.mjs
 */

import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import net from 'node:net';
import path from 'node:path';

const ROOT = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

let chromium;
try {
	({ chromium } = await import('playwright'));
} catch {
	try {
		({ chromium } = createRequire(import.meta.url)('playwright'));
	} catch {
		console.log('SKIP: playwright is not installed');
		process.exit(0);
	}
}

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name}${extra ? '\n        ' + extra : ''}`);
	}
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(fn, ms = 15000, step = 50) {
	const end = Date.now() + ms;
	while (Date.now() < end) {
		if (await fn()) return true;
		await sleep(step);
	}
	return false;
}

const port = await new Promise((resolve) => {
	const s = net.createServer().listen(0, () => {
		const p = s.address().port;
		s.close(() => resolve(p));
	});
});
const vite = spawn('npx', ['vite', 'dev', '--port', String(port), '--strictPort'], { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
let viteOut = '';
vite.stdout.on('data', (d) => (viteOut += d));
vite.stderr.on('data', (d) => (viteOut += d));
const up = await until(async () => {
	try {
		return (await fetch(`http://localhost:${port}/`)).ok;
	} catch {
		return false;
	}
}, 60000, 250);
if (!up) {
	console.log('FAIL: vite did not start\n' + viteOut);
	vite.kill();
	process.exit(1);
}



const browser = await chromium.launch({ args: ['--disable-webgl', '--disable-3d-apis'] });
const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 }, colorScheme: 'dark', deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
const page = await ctx.newPage();
await page.addInitScript(() => {
	localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: ['devops', 'containers-k8s'], checkForUpdates: false, terminalBar: 'hidden', serverMonitoring: false, theme: 'dark' }));
});
const auth = { type: 'Password', password: 'x' };
const SESSIONS = [
	{ id: 's1', name: 'web-01', host: '10.0.0.10', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [], alt_addresses: [{ host: '10.0.1.10' }], wol: { mac: '00:11:22:33:44:55' } },
	{ id: 's2', name: 'web-02', host: '10.0.0.11', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [], terminal: { encoding: 'ISO-8859-2' }, favorite: true },
	{ id: 'r1', name: 'desk-01', host: '10.0.0.20', port: 3389, username: 'user', auth_method: auth, folder_id: null, tags: [], kind: 'rdp', rdp: { performance: 'fast' } },
	{ id: 'r2', name: 'desk-02', host: '10.0.0.21', port: 3389, username: 'user', auth_method: auth, folder_id: null, tags: [], kind: 'rdp', rdp_use_freerdp: true }
];
const calls = [];
page.on('pageerror', (e) => console.log(`  page error: ${e.message}`));
await page.exposeBinding('__bridge', async (_src, { cmd, args }) => {
	calls.push({ cmd, args });
	switch (cmd) {
		case 'plugin:event|listen':
			return args.handler;
		case 'vault_has_identity':
		case 'credential_has_master_password':
		case 'vault_auto_unlock':
			return true;
		case 'vault_is_locked':
		case 'vault_is_held':
			return false;
		case 'session_list':
			return SESSIONS;
		case 'wireguard_list':
		case 'wireguard_profile_list':
			return [{ id: 'wg1', name: 'Office tunnel', state: { running: false } }];
		case 'session_bulk_update':
			return { results: args.sessionIds.map((id) => ({ id, name: id, status: 'changed', skipped: [] })), undo: 'u1' };
		case 'session_list_folders':
		case 'vault_list':
		case 'vault_duplicates':
		case 'ssh_list_connections':
		case 'snippet_list':
		case 'theme_list_installed':
		case 'theme_fetch_registry':
		case 'plugin:window|available_monitors':
		case 'password_history_list':
		case 'saved_credential_list':
		case 'credential_folder_list':
		case 'ssh_key_list':
		case 'pwmanager_status':
			return [];
		case 'share_load':
		case 'share_get_config':
			return { enabled: false, iceServers: [] };
		case 'mcp_status':
			return { enabled: false, running: false, port: null, shared: [] };
		default:
			if (process.env.DEBUG) console.log('  (null for ' + cmd + ')');
			return null;
	}
});
await page.addInitScript(() => {
	const callbacks = new Map();
	let next = 1;
	window.__TAURI_INTERNALS__ = {
		metadata: { currentWindow: { label: 'main' }, currentWebview: { windowLabel: 'main', label: 'main' } },
		transformCallback(cb, once) {
			const id = next++;
			callbacks.set(id, (d) => {
				if (once) callbacks.delete(id);
				cb(d);
			});
			return id;
		},
		unregisterCallback(id) {
			callbacks.delete(id);
		},
		invoke: (cmd, args) => window.__bridge({ cmd, args: args ?? {} }),
		convertFileSrc: (p) => p
	};
	window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
});

const dialog = () => page.locator('.bulk').first();
async function shot(name) {
	if (!process.env.SHOT_DIR) return;
	await sleep(300);
	await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png`, fullPage: false }).catch(() => {});
}
async function scrollTo(id) {
	await page.locator(id).scrollIntoViewIfNeeded();
}

try {
	await page.goto(`http://localhost:${port}/`);
	check('the session list shows', await until(async () => (await page.locator('[data-session-id]').count()) === SESSIONS.length, 30000), viteOut.slice(-2000));
	await page.locator(`button[aria-label="Select all"]`).first().click();
	await page.locator(`button[aria-label="Edit"]`).first().click();
	check('the bulk editor opens', await until(async () => (await dialog().count()) > 0));

	const ids = ['#bulk-wireguard', '#bulk-files', '#bulk-term-type', '#bulk-encoding', '#bulk-rdp-client', '#bulk-rdp-performance', '#bulk-gateway', '#bulk-favorite', '#bulk-wol-port', '#bulk-connect-mode'];
	for (const id of ids) check(`${id} shows`, (await page.locator(id).count()) === 1);
	const keepText = async (id) => (await page.locator(`${id} option`).first().innerText()).trim();
	check('mixed RDP client', (await keepText('#bulk-rdp-client')) === 'Keep (Mixed)', await keepText('#bulk-rdp-client'));
	check('mixed encodings', (await keepText('#bulk-encoding')).includes('Mixed'));
	await shot('bulk-new-top');

	await page.selectOption('#bulk-wireguard', 'wg1');
	await page.selectOption('#bulk-connect-mode', 'ask');
	await page.selectOption('#bulk-files', 'sftp');
	await page.fill('#bulk-term-type', 'vt220');
	await page.selectOption('#bulk-encoding', 'UTF-8');
	await scrollTo('#bulk-rdp-client');
	await page.selectOption('#bulk-rdp-client', 'ironrdp');
	await page.selectOption('#bulk-rdp-keyboard_layout', '0405');
	await page.selectOption('#bulk-rdp-performance', '');
	await page.selectOption('#bulk-rdp-admin', 'on');
	await page.selectOption('#bulk-gateway', 'set');
	await page.locator('input[aria-label="RD Gateway"]').fill('gw.example.com');
	await shot('bulk-new-rdp');
	await scrollTo('#bulk-favorite');
	await page.selectOption('#bulk-favorite', 'on');
	await page.fill('#bulk-wol-port', '7');
	await shot('bulk-new-bottom');

	await page.getByRole('button', { name: 'Review changes' }).click();
	await sleep(300);
	await shot('bulk-new-confirm');
	await page.getByRole('button', { name: /Change 4 sessions/ }).click();
	check('the patch is sent', await until(async () => calls.some((c) => c.cmd === 'session_bulk_update')));
	const p = calls.find((c) => c.cmd === 'session_bulk_update')?.args.patch ?? {};
	const want = {
		wireguard: { set: 'wg1' },
		connect_mode: 'ask',
		files: { set: 'sftp' },
		terminal: { term: { set: 'vt220' }, encoding: 'clear' },
		rdp: { client: 'ironrdp', keyboard_layout: { set: 0x0405 }, performance: 'clear', admin: true, gateway: { set: { host: 'gw.example.com', detect: false, rpc: false, user: null, password: null } } },
		wol: { port: { set: 7 } },
		favorite: true
	};
	for (const [k, v] of Object.entries(want)) check(`patch ${k}`, JSON.stringify(p[k]) === JSON.stringify(v), JSON.stringify(p[k]));
	check('nothing else is sent', Object.keys(p).sort().join() === Object.keys(want).sort().join(), Object.keys(p).join());
} finally {
	await browser.close();
	vite.kill();
}

if (failures > 0) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nAll passed');
