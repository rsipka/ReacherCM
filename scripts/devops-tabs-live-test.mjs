/**
 * DevOps workspaces as tabs, in a browser page with a stand-in for Tauri:
 * the Sessions list's DevOps group lists the switched-on tools and each
 * saved Kubernetes cluster; a cluster opens its own tab, named after it, a
 * second pick of it brings that tab forward, another cluster opens a tab of
 * its own, closing a cluster's tab closes its connection, and switching a
 * tool off closes its tabs. There is no page switcher in the tab bar.
 *
 * Needs Playwright's Chromium; skips cleanly without it. SHOT_DIR=<dir>
 * saves screenshots.
 *
 * Run: node scripts/devops-tabs-live-test.mjs
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
	localStorage.setItem(
		'reach-settings',
		JSON.stringify({
			setupComplete: true,
			seenFeatures: ['devops', 'containers-k8s'],
			checkForUpdates: false,
			terminalBar: 'hidden',
			serverMonitoring: false,
			theme: 'dark',
			devopsTools: ['ansible', 'tofu', 'databases', 'containers', 'kubernetes']
		})
	);
});
const auth = { type: 'Password', password: 'x' };
const SESSIONS = [{ id: 's1', name: 'web-01', host: '10.0.0.10', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [] }];
const cluster = (id, name, environment) => ({ id, name, context: name, server: 'https://10.0.0.5:6443', user: 'admin', route: { kind: 'direct' }, environment, readOnly: false, lastUsedAt: 0, namespace: null });
const CLUSTERS = [cluster('c1', 'prod', 'production'), cluster('c2', 'staging', 'staging')];
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
		// Left broken on purpose: the Databases workspace fails on it, which
		// must stay inside its own tab.
		case 'db_list_connections':
			return null;
		case 'k8s_clusters':
			return CLUSTERS;
		case 'k8s_open':
			return { key: args.clusterId, version: 'v1.30.2', namespace: 'default', readOnly: false };
		case 'k8s_namespaces':
			return ['default', 'kube-system'];
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
		case 'db_list':
		case 'container_hosts':
		case 'k8s_pods':
		case 'k8s_workloads':
		case 'k8s_events':
		case 'k8s_objects':
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
	await sleep(400);
	await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png`, ...(clip ? { clip } : {}) }).catch(() => {});
}
const count = (cmd) => calls.filter((c) => c.cmd === cmd).length;
const last = (cmd) => [...calls].reverse().find((c) => c.cmd === cmd);
const group = () => page.locator('.devops-group');
const row = (name) => group().locator('.row', { hasText: new RegExp(`^\\s*${name}\\s*`) }).first();
const tabTitles = async () => (await page.locator('.tabbar .tab-title').allInnerTexts()).map((x) => x.trim());
const activeTitle = async () => (await page.locator('.tabbar [role=tab][aria-selected=true] .tab-title').innerText().catch(() => '')).trim();

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	check('sessions show', await until(async () => (await page.locator('.session-card').count()) >= 1));

	console.log('\nsessions list');
	check('a DevOps group under the sessions', await until(async () => (await group().count()) === 1));
	const rows = (await group().locator('.row').allInnerTexts()).map((x) => x.trim().replace(/\s+/g, ' '));
	check('every tool switched on is listed', ['Ansible', 'OpenTofu', 'Databases', 'Containers', 'Kubernetes'].every((n) => rows.some((r) => r.startsWith(n))), JSON.stringify(rows));
	check('each saved cluster is a row', await until(async () => (await group().locator('.row.cluster').count()) === 2));
	check('no page switcher in the tab bar', (await page.locator('.page-switcher, .page-btn').count()) === 0);
	await shot('sessions', { x: 0, y: 0, width: 640, height: 520 });

	console.log('\na cluster opens its own tab');
	await group().locator('.row.cluster', { hasText: 'prod' }).click();
	check('a tab named after the cluster', await until(async () => (await activeTitle()) === 'prod'), JSON.stringify(await tabTitles()));
	check('it connects that cluster', await until(async () => last('k8s_open')?.args.clusterId === 'c1'));
	check('the workspace shows in the tab', await until(async () => (await page.locator('.terminal-wrapper.active .devops-body').count()) === 1));
	await shot('prod', false);

	await group().locator('.row.cluster', { hasText: 'staging' }).click();
	check('another cluster opens another tab', await until(async () => (await activeTitle()) === 'staging'));
	check('and connects it', await until(async () => last('k8s_open')?.args.clusterId === 'c2'));
	const opens = count('k8s_open');
	await group().locator('.row.cluster', { hasText: 'prod' }).click();
	check('picking an open cluster brings its tab forward', await until(async () => (await activeTitle()) === 'prod'));
	check('without a second tab', (await tabTitles()).filter((x) => x === 'prod').length === 1, JSON.stringify(await tabTitles()));
	check('or a second connect', count('k8s_open') === opens);
	check('the open clusters are tinted in the list', (await group().locator('.row.cluster.open').count()) === 2);

	console.log('\nother tools');
	await row('Databases').click();
	check('Databases opens in a tab', await until(async () => (await activeTitle()) === 'Databases'));
	await row('Databases').click();
	check('once', (await tabTitles()).filter((x) => x === 'Databases').length === 1);
	check('a workspace that fails says so in its tab', await until(async () => (await page.locator('.terminal-wrapper.active .devops-failed').count()) === 1));
	check('and the cluster tabs live on', !calls.some((c) => c.cmd === 'k8s_close'));
	await shot('tabs', { x: 0, y: 0, width: 1280, height: 120 });

	console.log('\nclosing');
	await page.locator('.tabbar [role=tab]', { hasText: 'prod' }).locator('.tab-close').click();
	check('the tab goes', await until(async () => !(await tabTitles()).includes('prod')));
	check('its cluster connection closes', await until(async () => calls.some((c) => c.cmd === 'k8s_close' && c.args.key === 'c1')));
	await sleep(1500);
	check('the other cluster stays open', !calls.some((c) => c.cmd === 'k8s_close' && c.args.key === 'c2'), JSON.stringify(calls.filter((c) => c.cmd.startsWith('k8s_open') || c.cmd === 'k8s_close').map((c) => c.cmd + ':' + (c.args.clusterId ?? c.args.key))));

	console.log('\ntool switched off');
	await page.evaluate(async () => {
		const m = await import('/src/lib/state/devops.svelte.ts');
		await m.setDevopsEnabled('kubernetes', false);
	});
	check('its tabs close', await until(async () => !(await tabTitles()).includes('staging')), JSON.stringify(await tabTitles()));
	check('it leaves the list', await until(async () => (await group().locator('.row', { hasText: 'Kubernetes' }).count()) === 0));
	check('the others stay', (await tabTitles()).includes('Databases'));

	console.log('\nphone width');
	await page.setViewportSize({ width: 390, height: 780 });
	await sleep(300);
	const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
	check('no sideways scroll', overflow <= 0, String(overflow));
	await shot('phone', false);
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
