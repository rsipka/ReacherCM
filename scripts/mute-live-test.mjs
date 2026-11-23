/**
 * The top bar's mute button, in a browser page with a stand-in for Tauri.
 *
 * What it proves:
 *   - The button shows, unmuted, and tells the backend so at start.
 *   - A click mutes: the crossed-out speaker, aria-pressed, the setting
 *     saved, and set_audio_muted(true) sent to the backend.
 *   - Ctrl+Shift+M unmutes again, and a reload starts muted when left so.
 *
 * Needs Playwright's Chromium; skips cleanly without it. SHOT_DIR=<dir>
 * saves screenshots.
 *
 * Run: node scripts/mute-live-test.mjs
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
const ctx = await browser.newContext({ viewport: { width: 1200, height: 700 }, colorScheme: 'dark', deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
const page = await ctx.newPage();
await page.addInitScript(() => {
	if (!localStorage.getItem('reach-settings')) localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: ['devops', 'containers-k8s'], checkForUpdates: false, terminalBar: 'hidden', serverMonitoring: false, theme: 'dark' }));
});
const auth = { type: 'Password', password: 'x' };
const SESSIONS = [
	{ id: 's1', name: 'web-01', host: '10.0.0.10', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [], alt_addresses: [{ label: 'External', host: 'web.example.com', port: 22 }] },
	{ id: 's2', name: 'db-01', host: '10.0.0.20', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [] }
];
const CREDS = [
	{ id: 'c1', name: 'Ops', username: 'ops', auth_method: auth },
	{ id: 'c2', name: 'Backup', username: 'backup', auth_method: auth }
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
		case 'saved_credential_list':
			return CREDS;
		case 'session_list_folders':
		case 'vault_list':
		case 'vault_duplicates':
		case 'ssh_list_connections':
		case 'snippet_list':
		case 'theme_list_installed':
		case 'theme_fetch_registry':
		case 'plugin:window|available_monitors':
			return [];
		case 'share_load':
		case 'share_get_config':
			return { enabled: false, iceServers: [] };
		case 'mcp_status':
			return { enabled: false, running: false, port: null, shared: [] };
		case 'history_start':
			return { recording: 'commands', paused: false, logFile: null };
		default:
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

const mutes = () => calls.filter((c) => c.cmd === 'set_audio_muted').map((c) => c.args.muted);
const btn = page.locator('.mute-btn');
const stored = () => page.evaluate(() => JSON.parse(localStorage.getItem('reach-settings') || '{}').muted);
async function shot(name) {
	if (!process.env.SHOT_DIR) return;
	await sleep(300);
	await page.locator('.titlebar').screenshot({ path: `${process.env.SHOT_DIR}/${name}.png` }).catch(() => {});
}

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.titlebar', { timeout: 30000 });
	check('the mute button shows', await until(async () => (await btn.count()) === 1));
	check('starts unmuted', (await btn.getAttribute('aria-pressed')) === 'false');
	check('tells the backend at start', await until(async () => mutes().at(-1) === false), JSON.stringify(mutes()));
	check('labelled Mute sounds', ((await btn.getAttribute('title')) ?? '').startsWith('Mute sounds'), await btn.getAttribute('title'));
	await shot('unmuted');

	await btn.click();
	check('a click mutes', (await btn.getAttribute('aria-pressed')) === 'true');
	check('the backend is told', await until(async () => mutes().at(-1) === true), JSON.stringify(mutes()));
	check('the setting is saved', (await stored()) === true);
	check('labelled Unmute sounds', ((await btn.getAttribute('title')) ?? '').startsWith('Unmute sounds'));
	await shot('muted');

	await page.reload();
	await page.waitForSelector('.mute-btn', { timeout: 30000 });
	check('a restart keeps it muted', (await btn.getAttribute('aria-pressed')) === 'true');
	check('and tells the backend', await until(async () => mutes().at(-1) === true), JSON.stringify(mutes()));

	await page.locator('body').click({ position: { x: 600, y: 400 } }).catch(() => {});
	await page.keyboard.press('Control+Shift+M');
	check('Ctrl+Shift+M unmutes', await until(async () => (await btn.getAttribute('aria-pressed')) === 'false'));
	check('the backend is told again', await until(async () => mutes().at(-1) === false), JSON.stringify(mutes()));
} catch (e) {
	failures++;
	console.log(`  FAIL  ${e.message}`);
} finally {
	await browser.close();
	vite.kill();
}

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
