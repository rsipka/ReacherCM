/**
 * The Servers section (local TFTP, FTP, SFTP and HTTP servers), in a browser
 * page with a stand-in for Tauri that answers the server commands.
 *
 * What it proves:
 *   - Desktop shows a Servers button in the sidebar; a phone does not.
 *   - Each server gets a generated login (not TFTP), the first LAN address
 *     and its standard port.
 *   - A port that cannot be used is said so, with a free one offered, and
 *     one click takes it.
 *   - Start sends the whole configuration; a running server lists the
 *     addresses to copy and its connection log, which follows log events.
 *   - All networks warns about exposure.
 *
 * Needs Playwright's Chromium; skips cleanly without it. SHOT_DIR=<dir>
 * saves screenshots.
 *
 * Run: node scripts/local-servers-live-test.mjs
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
const IFACES = [
	{ name: 'eth0', ip: '10.0.0.5', private: true, loopback: false },
	{ name: 'wg0', ip: '10.8.0.2', private: true, loopback: false },
	{ name: 'lo', ip: '127.0.0.1', private: true, loopback: true }
];
const USUAL = { tftp: 6969, ftp: 2121, sftp: 2222, http: 8080 };
const stopped = (kind) => ({ kind, running: false, bind: '', port: 0, root: '', addresses: [], log: [] });

async function openApp(mobile) {
	const ctx = await browser.newContext({
		viewport: mobile ? { width: 390, height: 844 } : { width: 1280, height: 860 },
		colorScheme: 'dark',
		deviceScaleFactor: process.env.SHOT_DIR && !mobile ? 2 : 1,
		...(mobile ? { userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148', hasTouch: true } : {})
	});
	const page = await ctx.newPage();
	const state = { calls: [], statuses: Object.fromEntries(['tftp', 'ftp', 'sftp', 'http'].map((k) => [k, stopped(k)])), logHandler: null };
	await page.addInitScript(() => {
		localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: ['devops', 'containers-k8s', 'local-servers'], checkForUpdates: false, terminalBar: 'hidden', serverMonitoring: false, theme: 'dark' }));
	});
	page.on('pageerror', (e) => console.log(`  page error: ${e.message}`));
	await page.exposeBinding('__bridge', async (_src, { cmd, args }) => {
		state.calls.push({ cmd, args });
		switch (cmd) {
			case 'plugin:event|listen':
				if (args.event === 'local-server-log') state.logHandler = args.handler;
				return args.handler;
			case 'plugin:path|resolve_directory':
				return '/home/user/Downloads';
			case 'vault_has_identity':
			case 'credential_has_master_password':
			case 'vault_auto_unlock':
				return true;
			case 'vault_is_locked':
			case 'vault_is_held':
				return false;
			case 'local_server_interfaces':
				return IFACES;
			case 'local_server_status':
				return Object.values(state.statuses);
			case 'local_server_check_port':
				if (args.port < 1024) return { free: false, problem: 'privileged', suggestion: USUAL[args.kind] };
				if (args.port === 8080) return { free: false, problem: 'in_use', suggestion: 8000 };
				return { free: true };
			case 'local_server_start': {
				const c = args.config;
				const addresses = c.bind === '0.0.0.0' ? IFACES.map((i) => i.ip) : [c.bind];
				state.statuses[c.kind] = {
					kind: c.kind, running: true, bind: c.bind, port: c.port, root: c.root, addresses,
					...(c.kind === 'sftp' ? { fingerprint: 'SHA256:q0bMxV4m3Yt1mJpO1bq8rUeQ2c6eX5Yb2dVYtq3mZ0g' } : {}),
					log: [{ kind: c.kind, time: '17:02:11', peer: '', message: `Started on ${c.bind}:${c.port}, serving ${c.root}, read only, 1 user(s)` }]
				};
				return state.statuses[c.kind];
			}
			case 'local_server_stop':
				state.statuses[args.kind] = { ...stopped(args.kind), log: state.statuses[args.kind].log };
				return state.statuses[args.kind];
			case 'session_list_folders':
			case 'session_list':
			case 'saved_credential_list':
			case 'credential_folder_list':
			case 'vault_list':
			case 'vault_duplicates':
			case 'ssh_list_connections':
			case 'snippet_list':
			case 'theme_list_installed':
			case 'theme_fetch_registry':
			case 'plugin:window|available_monitors':
			case 'password_history_list':
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
				return null;
		}
	});
	await page.addInitScript(() => {
		const callbacks = new Map();
		let next = 1;
		window.__callbacks = callbacks;
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
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	return { ctx, page, state };
}

try {
	console.log('phone');
	{
		const { ctx, page } = await openApp(true);
		await sleep(1000);
		await page.locator('.nav-btn[aria-label="Network"]').click();
		check('no Servers section on a phone', (await page.locator('.switch-btn', { hasText: 'Servers' }).count()) === 0);
		await ctx.close();
	}

	console.log('\ndesktop');
	const { page, state } = await openApp(false);
	const last = (cmd) => [...state.calls].reverse().find((c) => c.cmd === cmd);
	async function shot(name, height = 860) {
		if (!process.env.SHOT_DIR) return;
		await sleep(300);
		await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png`, clip: { x: 0, y: 0, width: 640, height } }).catch(() => {});
	}
	await page.locator('.nav-btn[aria-label="Network"]').click();
	await page.locator('.switch-btn', { hasText: 'Servers' }).click();
	check('the Servers section opens', await until(async () => (await page.locator('.server-card').count()) === 4));
	await page.evaluate(() => {
		// A wider panel, as a user would drag it, for the screenshots.
		localStorage.setItem('reach-sidebar-width', '420');
	});
	await page.reload();
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	await page.locator('.nav-btn[aria-label="Network"]').click();
	await page.locator('.switch-btn', { hasText: 'Servers' }).click();
	await until(async () => (await page.locator('.server-card').count()) === 4);
	const names = await page.locator('.server-card .name').allInnerTexts();
	check('TFTP, FTP, SFTP and HTTP', names.join(',') === 'TFTP,FTP,SFTP,HTTP', names.join(','));
	await shot('list', 420);

	console.log('\nTFTP');
	const tftp = page.locator('.server-card').nth(0);
	await tftp.locator('.title').click();
	check('folder defaults to Downloads', (await tftp.locator('.field input[type=text]').first().inputValue()) === '/home/user/Downloads');
	check('listens on the first LAN address', (await tftp.locator('select').inputValue()) === '10.0.0.5');
	check('port 69', (await tftp.locator('input[type=number]').inputValue()) === '69');
	check('no users for TFTP', (await tftp.locator('.user-row').count()) === 0 && (await tftp.innerText()).includes('TFTP has no logins'));
	check('a privileged port is said so, with 6969 offered', await until(async () => (await tftp.locator('.notice.warn').first().innerText().catch(() => '')).includes('administrator rights')));
	await shot('tftp-port');
	await tftp.locator('button', { hasText: 'Use 6969' }).click();
	check('one click takes the free port', (await tftp.locator('input[type=number]').inputValue()) === '6969');
	check('and the warning goes', await until(async () => (await tftp.locator('.notice.warn').count()) === 0));
	await tftp.locator('.header button', { hasText: 'Start' }).click();
	check('start sends the configuration', await until(async () => last('local_server_start')?.args.config.port === 6969));
	const sent = last('local_server_start').args.config;
	check('with the folder, address and write access', sent.root === '/home/user/Downloads' && sent.bind === '10.0.0.5' && sent.allowWrite === true && sent.kind === 'tftp', JSON.stringify(sent));
	check('shows the address to copy', await until(async () => (await tftp.locator('code').first().innerText().catch(() => '')) === 'tftp://10.0.0.5:6969/'));
	await page.evaluate(([id]) => {
		window.__callbacks.get(id)({ event: 'local-server-log', id: 1, payload: { kind: 'tftp', time: '17:03:40', peer: '10.0.0.21:51234', message: 'Download /fw/switch-9.2.bin (octet)' } });
		window.__callbacks.get(id)({ event: 'local-server-log', id: 1, payload: { kind: 'tftp', time: '17:03:44', peer: '10.0.0.21:51234', message: 'Download of /fw/switch-9.2.bin done: 18874368 bytes in 3.9 s' } });
	}, [state.logHandler]);
	check('the log follows events', await until(async () => (await tftp.locator('.log').innerText()).includes('switch-9.2.bin done')));
	await shot('tftp-running');
	await tftp.locator('.title').click();

	console.log('\nFTP');
	const ftp = page.locator('.server-card').nth(1);
	await ftp.locator('.title').click();
	check('one generated user', (await ftp.locator('.user-row').count()) === 1);
	const pass = await ftp.locator('.user-pass').inputValue();
	check('with a 14-character password', /^[a-km-zA-HJ-NP-Z2-9]{14}$/.test(pass), pass);
	check('hidden until shown', (await ftp.locator('.user-pass').getAttribute('type')) === 'password');
	await ftp.locator('button', { hasText: 'Show' }).click();
	check('Show reveals it', (await ftp.locator('.user-pass').getAttribute('type')) === 'text');
	await ftp.locator('button', { hasText: 'New password' }).click();
	check('New password changes it', (await ftp.locator('.user-pass').inputValue()) !== pass);
	await ftp.locator('button', { hasText: 'Add user' }).click();
	check('Add user adds another', (await ftp.locator('.user-row').count()) === 2);
	await ftp.locator('button', { hasText: 'Remove' }).nth(1).click();
	check('Remove takes it away', (await ftp.locator('.user-row').count()) === 1);
	check('warns FTP is unencrypted', (await ftp.innerText()).includes('FTP sends passwords and files unencrypted'));
	await page.locator('.server-card').nth(1).locator('select').selectOption('0.0.0.0');
	check('all networks warns', await until(async () => (await ftp.innerText()).includes('every network this computer is on')));
	await ftp.locator('button', { hasText: 'Use 2121' }).click();
	await shot('ftp');
	await page.locator('.server-card').nth(1).locator('select').selectOption('10.0.0.5');
	await ftp.locator('.title').click();

	console.log('\nSFTP');
	const sftp = page.locator('.server-card').nth(2);
	await sftp.locator('.title').click();
	await sftp.locator('button', { hasText: 'Use 2222' }).click();
	await sftp.locator('.header button', { hasText: 'Start' }).click();
	check('SFTP starts with its user', await until(async () => last('local_server_start')?.args.config.kind === 'sftp' && last('local_server_start').args.config.users.length === 1));
	const user = last('local_server_start').args.config.users[0].username;
	check('shows user@address and the host key', await until(async () => (await sftp.locator('code').allInnerTexts()).join(' ').includes(`sftp://${user}@10.0.0.5:2222/`) && (await sftp.innerText()).includes('SHA256:')));
	check('settings are locked while running', await sftp.locator('input[type=number]').isDisabled() && await sftp.locator('.user-pass').isDisabled());
	check('but the password can still be copied', !(await sftp.locator('button', { hasText: 'Copy password' }).isDisabled()));
	await shot('sftp-running');
	await sftp.locator('.header button', { hasText: 'Stop' }).click();
	check('Stop stops it', await until(async () => last('local_server_stop')?.args.kind === 'sftp'));
	await sftp.locator('.title').click();

	console.log('\nHTTP');
	const http = page.locator('.server-card').nth(3);
	await http.locator('.title').click();
	await http.locator('button', { hasText: 'Use 8080' }).click();
	check('a port in use is said so', await until(async () => (await http.locator('.notice.warn').first().innerText().catch(() => '')).includes('in use by another program')));
	check('with the next free one', (await http.locator('button', { hasText: 'Use 8000' }).count()) === 1);
	await http.locator('button', { hasText: 'Remove' }).click();
	check('no user and no anonymous: Start is off', await http.locator('.header button', { hasText: 'Start' }).isDisabled());
	await http.locator('.toggle, [role=switch]').first().click();
	check('anonymous turns it back on', await until(async () => !(await http.locator('.header button', { hasText: 'Start' }).isDisabled())));

	console.log('\nsaving');
	check('settings saved in the vault', await until(async () => last('settings_set')?.args.key === 'local_servers' && JSON.parse(last('settings_set').args.value).http.anonymous === true));
	const saved = JSON.parse(last('settings_set').args.value);
	check('with all four', ['tftp', 'ftp', 'sftp', 'http'].every((k) => saved[k]?.kind === k) && saved.http.anonymous === true, JSON.stringify(saved).slice(0, 300));
} finally {
	await browser.close();
	vite.kill();
}
console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
