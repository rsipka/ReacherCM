/**
 * The tmux chip in the terminal bar, in the real app with a stand-in for
 * Tauri: an SSH session in tmux connects, the bar names its tmux session
 * and window, and the chip's dropdown lists the server's tmux sessions,
 * switches to one, starts a new one and shows a window.
 *
 * Also checks the default name: `reachercm-…` is asked for, with the old
 * `reacher-…` as legacy, and the name the backend settled on is the one
 * the tab keeps.
 *
 * Needs Playwright's Chromium; skips cleanly without it. Starts `vite dev`
 * on a free port.
 *
 * Run: node scripts/tmux-bar-live-test.mjs   (SHOT_DIR=… for screenshots)
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
const scheme = process.env.SCHEME === 'light' ? 'light' : 'dark';
const ctx = await browser.newContext({ viewport: { width: 1200, height: 720 }, colorScheme: scheme, deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
const page = await ctx.newPage();
await page.addInitScript((theme) => {
	localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: ['devops', 'containers-k8s'], checkForUpdates: false, terminalBar: 'always', serverMonitoring: false, theme, tmuxConnectWindows: 'last' }));
}, scheme);
const auth = { type: 'Password', password: 'x' };
const SESSIONS = [
	{ id: 's1', name: 'Web 01', host: '10.0.0.10', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [], tmux: { mode: 'attach' } }
];
const now = Math.floor(Date.now() / 1000);
let current = 'reacher-web-01';
const LISTED = () => [
	{ name: current, windows: 3, attached: 1, activity: now },
	...['main', 'build', 'logs'].filter((n) => n !== current).map((name, i) => ({ name, windows: i + 1, attached: 0, activity: now - 600 * (i + 1) }))
];
const WINDOWS = [
	{ id: '@1', index: 0, name: 'bash', command: 'bash', panes: 1, active: false },
	{ id: '@2', index: 1, name: 'htop', command: 'htop', panes: 1, active: true },
	{ id: '@3', index: 2, name: 'logs', command: 'tail', panes: 2, active: false }
];
const calls = [];
const listeners = new Map();
page.on('pageerror', (e) => console.log(`  page error: ${e.message}`));
await page.exposeBinding('__bridge', async (_src, { cmd, args }) => {
	calls.push({ cmd, args });
	switch (cmd) {
		case 'plugin:event|listen':
			listeners.set(args.handler, args.event);
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
		case 'ssh_connect':
			return args.params?.id ?? args.id ?? null;
		case 'ssh_tmux_chosen':
			// The backend found only the old name on the server.
			return { name: 'reacher-web-01' };
		case 'ssh_tmux_list':
			return LISTED();
		case 'ssh_tmux_windows':
			return WINDOWS;
		case 'session_list_folders':
		case 'saved_credential_list':
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
			return { recording: false, level: null, logPath: null };
		default:
			return null;
	}
});
await page.addInitScript(() => {
	const callbacks = new Map();
	let next = 1;
	window.__mock = { fire: (id, data) => callbacks.get(id)?.(data) };
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

async function emit(event, payload) {
	const ids = [...listeners].filter(([, e]) => e === event).map(([id]) => id);
	await page.evaluate(({ ids, event, payload }) => ids.forEach((id) => window.__mock.fire(id, { event, id, payload })), { ids, event, payload });
}
async function shot(name, clip) {
	if (!process.env.SHOT_DIR) return;
	await sleep(300);
	await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png`, ...(clip ? { clip } : {}) }).catch(() => {});
}
const connects = () => calls.filter((c) => c.cmd === 'ssh_connect');
const tmuxOf = (c) => c.args.params?.tmux ?? c.args.tmux;

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	const card = page.locator('.session-card', { hasText: 'Web 01' }).locator('.card-main');
	check('the session list shows', await until(async () => (await card.count()) === 1));
	await card.dblclick();
	check('it connects', await until(async () => connects().length === 1));
	const first = tmuxOf(connects()[0]);
	check('the new default name is asked for', first?.name === 'reachercm-web-01', JSON.stringify(first));
	check('with the old one as legacy', first?.legacy === 'reacher-web-01', JSON.stringify(first));

	const chip = page.locator('.term-bar .chip:visible');
	check('the bar shows the tmux chip', await until(async () => (await chip.count()) === 1));
	check('naming the session the backend settled on', (await chip.innerText()).includes('reacher-web-01'), await chip.innerText());
	// The view sets the terminal title to "@ID NAME" of its window.
	const connId = connects()[0].args.params?.id ?? connects()[0].args.id;
	await emit(`ssh-data-${connId}`, '\x1b]2;@2 htop\x07');
	await until(async () => (await chip.innerText()).includes('htop'), 3000);
	console.log(`  chip: ${(await chip.innerText()).replace(/\s+/g, ' ')}`);
	// Out of the picture: the "Connected" toast.
	await page.evaluate(() => document.querySelector('.toast-container')?.remove());
	const bar = await page.locator('.term-bar').boundingBox();
	await shot('bar', { x: bar.x, y: bar.y, width: Math.min(bar.width, 760), height: bar.height });

	console.log('\ndropdown');
	await chip.click();
	const pop = page.locator('.pop');
	check('opens', await until(async () => (await pop.count()) === 1));
	check('lists the server\'s sessions', await until(async () => (await pop.locator('.row').count()) >= 4));
	const text = (await pop.innerText()).replace(/\s+/g, ' ');
	check('marks the current one', (await pop.locator('.row.current').first().innerText()).includes('reacher-web-01'), text);
	check('shows the windows', text.includes('htop') && text.includes('logs'), text);
	const box = await pop.boundingBox();
	await shot('dropdown', { x: Math.max(0, box.x - 260), y: bar.y, width: box.width + 520, height: box.y - bar.y + box.height + 12 });

	await pop.locator('.row', { hasText: 'main' }).first().click();
	check('closes on pick', await until(async () => (await pop.count()) === 0));
	check('picking a session connects again into it', await until(async () => connects().length === 2), String(connects().length));
	const second = tmuxOf(connects()[1] ?? { args: {} });
	check('as that session, with no legacy name', second?.name === 'main' && !second?.legacy, JSON.stringify(second));
	check('the chip follows', await until(async () => (await chip.innerText()).includes('main')));

	console.log('\nnew session and windows');
	await chip.click();
	await until(async () => (await pop.locator('.row').count()) >= 4);
	await pop.locator('.new input').fill('deploy');
	await pop.locator('.new button').click();
	check('a new name connects into it', await until(async () => connects().length === 3 && tmuxOf(connects()[2])?.name === 'deploy'));
	await chip.click();
	await until(async () => (await pop.locator('.row').count()) >= 4);
	await pop.locator('.row', { hasText: '2: logs' }).click();
	check('picking a window shows it', await until(async () => calls.some((c) => c.cmd === 'ssh_tmux_window' && c.args.op?.kind === 'select' && c.args.op.window === '@3')));
	console.log('\nwindows in new tabs');
	const tabsBefore = await page.locator('.tabbar .tab').count();
	const before = connects().length;
	await chip.click();
	await until(async () => (await pop.locator('.win').count()) >= 4);
	await shot('dropdown-tab-buttons', await (async () => { const b = await pop.boundingBox(); return { x: Math.max(0, b.x - 20), y: b.y - 50, width: b.width + 40, height: b.height + 60 }; })());
	await pop.locator('.win', { hasText: '0: bash' }).locator('.tab-btn').click();
	check('the tab button connects a new view of that window', await until(async () => connects().length === before + 1 && tmuxOf(connects().at(-1))?.window === '@1'), JSON.stringify(tmuxOf(connects().at(-1) ?? { args: {} })));
	check('in a new tab', await until(async () => (await page.locator('.tabbar .tab').count()) === tabsBefore + 1));
	check('of the same session, in a view of its own', tmuxOf(connects().at(-1))?.name === 'deploy' && !!tmuxOf(connects().at(-1))?.view);
	// Back to the first tab, whose bar shows the chip.
	await page.locator('.tabbar .tab').first().click();
	await chip.click();
	await until(async () => (await pop.locator('.win').count()) >= 4);
	await pop.locator('.row', { hasText: '2: logs' }).click({ modifiers: ['Control'] });
	check('Ctrl+click opens the window in a new tab too', await until(async () => tmuxOf(connects().at(-1))?.window === '@3'));
	await page.locator('.tabbar .tab').first().click();
	await chip.click();
	await until(async () => (await pop.locator('.win').count()) >= 4);
	await pop.locator('.win').last().locator('.tab-btn').click();
	check('a new window in a new tab', await until(async () => tmuxOf(connects().at(-1))?.window === 'new'));

	await page.locator('.tabbar .tab').first().click();
	await chip.click();
	await until(async () => (await pop.count()) === 1);
	await page.keyboard.press('Escape');
	check('Escape closes it', await until(async () => (await pop.count()) === 0));
} catch (e) {
	failures++;
	console.log(`  FAIL  ${e.stack ?? e}`);
} finally {
	await browser.close();
	vite.kill();
}
console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
