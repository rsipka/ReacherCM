/**
 * Network tools in a browser page with a stand-in for Tauri: the session
 * menu's Ping / Scan ports / Wake open the sidebar's Network tools on that
 * session and start; a session with several addresses offers each; the
 * session editor keeps a Wake-on-LAN MAC.
 *
 * Needs Playwright's Chromium; skips cleanly without it. SHOT_DIR=<dir>
 * saves screenshots.
 *
 * Run: node scripts/net-tools-live-test.mjs
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
const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: 'dark', deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
const page = await ctx.newPage();
await page.addInitScript(() => {
	localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: ['devops', 'containers-k8s'], checkForUpdates: false, terminalBar: 'hidden', serverMonitoring: false, theme: 'dark' }));
});
const auth = { type: 'Password', password: 'x' };
let SESSIONS = [
	{ id: 's1', name: 'web-01', host: '10.0.0.10', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [], host_label: 'Office', alt_addresses: [{ label: 'Internet', host: 'server.example.com', port: 2222 }] },
	{ id: 's2', name: 'nas', host: '10.0.0.20', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [], wol: { mac: '00:11:22:AA:BB:CC', broadcast: '10.0.0.255' } },
	{ id: 's3', name: 'console', host: '/dev/ttyUSB0', port: 0, username: '', auth_method: auth, folder_id: null, tags: [], kind: 'serial' },
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
		case 'session_update':
			SESSIONS = SESSIONS.map((s) => (s.id === args.session.id ? args.session : s));
			return args.session;
		case 'net_resolve':
			return [args.host === 'server.example.com' ? '192.0.2.10' : args.host];
		case 'net_ping':
			return args.seq === 3 ? { ok: false, ms: null, method: 'icmp', ttl: null, error: null } : { ok: true, ms: 0.4 + args.seq / 10, method: 'icmp', ttl: 64, error: null };
		case 'net_scan_ports':
			return {
				__events: [{ type: 'open', port: 22, ms: 0.5 }, { type: 'open', port: 443, ms: 0.7 }, { type: 'open', port: 8006, ms: 0.9 }, { type: 'progress', done: args.ports.length, total: args.ports.length }],
				result: { open: [22, 443, 8006], closed: args.ports.length - 5, filtered: 2, scanned: args.ports.length, cancelled: false, ms: 1240 },
			};
		case 'net_wake':
		case 'net_scan_cancel':
			return null;
		case 'session_list_folders':
		case 'vault_list':
		case 'vault_duplicates':
		case 'ssh_list_connections':
		case 'snippet_list':
		case 'theme_list_installed':
		case 'theme_fetch_registry':
		case 'plugin:window|available_monitors':
		case 'password_history_list':
		case 'session_password_history':
		case 'ssh_algorithm_catalog':
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
		case 'history_start':
			return { recording: 'commands', paused: false, logFile: null };
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
		// A command's events (a Channel's messages) come back with its
		// result, and are delivered first.
		invoke: async (cmd, args) => {
			const channel = args?.onEvent;
			const plain = channel ? { ...args, onEvent: channel.id } : (args ?? {});
			const r = await window.__bridge({ cmd, args: plain });
			if (r && r.__events) {
				r.__events.forEach((message, index) => callbacks.get(channel.id)?.({ message, index }));
				return r.result;
			}
			return r;
		},
		convertFileSrc: (p) => p
	};
	window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
});

async function shot(name, clip) {
	if (!process.env.SHOT_DIR) return;
	await sleep(300);
	await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png`, ...(clip === false ? {} : { clip: clip ?? { x: 0, y: 0, width: 720, height: 640 } }) }).catch(() => {});
}
const last = (cmd) => [...calls].reverse().find((c) => c.cmd === cmd);
const card = (name) => page.locator('.session-card', { hasText: name }).first();
const menuText = async () => (await page.locator('.context-menu').innerText()).replace(/\s+/g, ' ');

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	check('sessions show', await until(async () => (await page.locator('.session-card').count()) >= 2));

	console.log('\nsession menu');
	await card('web-01').click({ button: 'right' });
	let menu = await menuText();
	check('offers Ping and Scan ports', menu.includes('Ping…') && menu.includes('Scan ports…'), menu);
	check('no Wake without a MAC', !menu.includes('Wake (Wake-on-LAN)'), menu);
	await shot('menu', { x: 0, y: 0, width: 640, height: 720 });
	await page.locator('.context-item', { hasText: 'Ping…' }).click();
	check('the Network tools section opens', await until(async () => (await page.locator('.net-tools').count()) === 1));
	check('pings the session main address', await until(async () => last('net_ping')?.args.address === '10.0.0.10'));
	check('four pings, one lost', await until(async () => (await page.locator('.out-stats').innerText().catch(() => '')).includes('4 sent, 3 received, 25% lost'), 8000));
	const opts = await page.locator('.net-tools select').nth(1).locator('option').allInnerTexts();
	check('both addresses offered', opts.length === 2 && opts[0].includes('10.0.0.10') && opts[1].includes('server.example.com'), JSON.stringify(opts));
	await shot('ping');

	console.log('\nother address');
	await page.locator('.net-tools select').nth(1).selectOption('1');
	check('host follows the address', (await page.locator('.net-tools input[type=text]').first().inputValue()) === 'server.example.com');
	await page.locator('.seg', { hasText: 'Port scan' }).click();
	await page.locator('.btn', { hasText: 'Scan' }).click();
	check('scans the other address, resolved', await until(async () => last('net_scan_ports')?.args.address === '192.0.2.10'));
	check('common ports', (last('net_scan_ports')?.args.ports ?? []).includes(3389));
	check('open ports listed with names', await until(async () => (await page.locator('.output').innerText()).includes('Proxmox VE')));
	await shot('scan');

	console.log('\ncustom ports');
	await page.locator('.net-tools select').nth(2).selectOption('custom');
	await page.locator('input[placeholder="22, 80, 443, 8000-8100"]').fill('22, ssh');
	check('a bad list is marked', (await page.locator('input[placeholder="22, 80, 443, 8000-8100"]').getAttribute('class')).includes('invalid'));
	await page.locator('input[placeholder="22, 80, 443, 8000-8100"]').fill('22, 8000-8002');
	const before = calls.filter((c) => c.cmd === 'net_scan_ports').length;
	await page.locator('.btn', { hasText: 'Scan' }).click();
	check('scans the list', await until(async () => calls.filter((c) => c.cmd === 'net_scan_ports').length > before && JSON.stringify(last('net_scan_ports').args.ports) === '[22,8000,8001,8002]'));

	console.log('\nwake');
	await page.locator('.nav-btn[aria-label="Sessions"]').click();
	await card('nas').click({ button: 'right' });
	menu = await menuText();
	check('Wake offered with a MAC', menu.includes('Wake (Wake-on-LAN)'), menu);
	await page.locator('.context-item', { hasText: 'Wake (Wake-on-LAN)' }).click();
	check('sends the magic packet to the session broadcast', await until(async () => last('net_wake')?.args.mac === '00:11:22:AA:BB:CC' && last('net_wake')?.args.broadcast === '10.0.0.255'));
	check('offers to ping until up', await until(async () => (await page.locator('.btn', { hasText: 'Ping until it is up' }).count()) === 1));
	await shot('wake');

	console.log('\nserial');
	await page.locator('.nav-btn[aria-label="Sessions"]').click();
	await card('console').click({ button: 'right' });
	check('no network tools for a serial session', !(await menuText()).includes('Ping…'));
	await page.locator('.context-backdrop').click();

	console.log('\neditor');
	await card('nas').click({ button: 'right' });
	await page.locator('.context-item', { hasText: /^\s*Edit\s*$/ }).click();
	check('the editor shows the MAC', await until(async () => (await page.locator('#session-wol-mac').inputValue().catch(() => '')) === '00:11:22:AA:BB:CC'));
	await page.locator('#session-wol-mac').scrollIntoViewIfNeeded();
	await shot('editor', false);
	await page.locator('#session-wol-mac').fill('00-11-22-aa-bb-dd');
	await page.locator('.modal-actions button', { hasText: 'Update session' }).click();
	check('saves it, written the usual way', await until(async () => last('session_update')?.args.session.wol?.mac === '00:11:22:AA:BB:DD'), JSON.stringify(last('session_update')?.args.session.wol));
} catch (e) {
	failures++;
	console.log('  FAIL  ' + (e?.stack ?? e));
} finally {
	await browser.close();
	vite.kill();
}

if (failures) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nall passed');
process.exit(0);
