/**
 * FTP, SFTP and SCP sessions in the real app, in a browser page with a
 * stand-in for Tauri: the editor's Transfers tab, an FTP session that
 * meets an untrusted certificate, is trusted and opens in the full-pane
 * file manager, and an SFTP session that opens there too.
 *
 * Needs Playwright's Chromium; skips cleanly without it. Starts `vite dev`
 * on a free port. SHOT_DIR=dir saves screenshots.
 *
 * Run: node scripts/transfer-sessions-live-test.mjs
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



const FP = 'D5:DC:B3:C4:8E:43:AA:37:A2:D6:37:D0:C7:4B:EC:A5:E8:CF:49:73:9B:9A:35:C9:96:A8:D9:AC:27:5D:C3:6E';
const base = { folder_id: null, tags: [], auth_method: { type: 'Password', password: 'x' } };
const SESSIONS = [
	{ ...base, id: 's-ftp', name: 'Web hosting', host: 'ftp.example.com', port: 21, username: 'deploy', kind: 'ftp', ftp: {} },
	{ ...base, id: 's-sftp', name: 'Backup box', host: '10.0.0.20', port: 22, username: 'backup', files: 'sftp', auth_method: { type: 'Agent' } },
	{ ...base, id: 's-scp', name: 'Old router', host: '10.0.0.1', port: 22, username: 'admin', files: 'scp' },
	{ ...base, id: 's-ssh', name: 'Web server', host: '10.0.0.10', port: 22, username: 'root', auth_method: { type: 'Agent' } }
];
const t0 = Math.floor(Date.now() / 1000);
const entry = (dir, name, isDirectory, size = 0, ago = 3600) => ({ name, path: `${dir === '/' ? '' : dir}/${name}`, isDirectory, size, modified: t0 - ago, permissions: isDirectory ? 'drwxr-xr-x' : '-rw-r--r--', owner: 'deploy', group: 'www' });
const REMOTE = {
	'/srv/www': [entry('/srv/www', 'assets', true), entry('/srv/www', 'blog', true), entry('/srv/www', 'index.html', false, 18234), entry('/srv/www', 'robots.txt', false, 120, 86400 * 30), entry('/srv/www', 'sitemap.xml', false, 6230, 86400)],
	'/home/backup': [entry('/home/backup', 'daily', true), entry('/home/backup', 'weekly', true), entry('/home/backup', 'restore.sh', false, 2048)]
};
const LOCAL = { path: '/home/me/Projects/site', parent: '/home/me/Projects', entries: [entry('/home/me/Projects/site', 'dist', true), entry('/home/me/Projects/site', 'src', true), entry('/home/me/Projects/site', 'package.json', false, 1422), entry('/home/me/Projects/site', 'README.md', false, 3920)] };
let certTrusted = false;

const browser = await chromium.launch({ args: ['--disable-webgl', '--disable-3d-apis'] });
const page = await browser.newPage({ viewport: { width: 1280, height: 780 }, deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
await page.addInitScript(() => {
	localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: [], checkForUpdates: false, serverMonitoring: false, theme: 'dark' }));
});
const calls = [];
page.on('pageerror', (e) => console.log(`  page error: ${e.message}`, process.env.STACK ? e.stack : ""));
await page.exposeBinding('__bridge', async (_src, { cmd, args }) => {
	calls.push({ cmd, args });
	switch (cmd) {
		case 'plugin:event|listen':
			return args.handler;
		case 'pty_spawn':
			return args.id ?? null;
		case 'vault_has_identity':
		case 'credential_has_master_password':
		case 'vault_auto_unlock':
			return true;
		case 'vault_is_locked':
		case 'vault_is_held':
			return false;
		case 'session_list':
			return SESSIONS;
		case 'session_get':
			return SESSIONS.find((s) => s.id === args.sessionId);
		case 'ftp_connect':
			if (!certTrusted) throw `FTP_UNTRUSTED_CERT:${FP}`;
			return { home: '/srv/www' };
		case 'ftp_session_cert_trust':
			certTrusted = args.fingerprint === FP;
			return null;
		case 'ssh_connect':
			return args.id;
		case 'sftp_home':
			return args.connectionId.length > 30 && calls.some((c) => c.cmd === 'ftp_connect' && c.args.id === args.connectionId) ? '/srv/www' : '/home/backup';
		case 'sftp_list_dir':
			return REMOTE[args.path] ?? [];
		case 'local_list_dir':
			return LOCAL;
		case 'saved_credential_list':
			return [{ id: 'cred-key', name: 'Deploy key', username: 'deploy', auth_method: { type: 'Key', path: '~/.ssh/id_ed25519' } }];
		case 'session_list_folders':
		case 'vault_list':
		case 'vault_duplicates':
		case 'ssh_list_connections':
		case 'snippet_list':
		case 'theme_list_installed':
		case 'credential_folder_list':
		case 'plugin:window|available_monitors':
			return [];
		case 'share_load':
		case 'share_get_config':
			return { enabled: false, iceServers: [] };
		case 'mcp_status':
			return { enabled: false, running: false, port: null, shared: [] };
		case 'history_start':
			return { recording: 'off', paused: false, logFile: null };
		default:
			// Lists (password managers, password history, SSH algorithms…) are empty.
			return /list|history|catalog|pwmanager_status/.test(cmd) ? [] : null;
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

async function shot(name) {
	if (!process.env.SHOT_DIR) return;
	await sleep(400);
	await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png` }).catch(() => {});
}
const card = (name) => page.locator('.session-card', { hasText: name }).first();

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	check('the sessions are listed', await until(async () => (await page.locator('.session-card').count()) === 4));
	const badges = await page.locator('.session-card .auth-badge').allInnerTexts();
	check('file sessions carry their protocol', ['FTPS', 'SFTP', 'SCP'].every((b) => badges.includes(b)), JSON.stringify(badges));
	await shot('list');

	console.log('\nthe editor');
	await card('Web hosting').hover();
	await card('Web hosting').locator('.edit-btn').click();
	check('the editor opens on Transfers', await until(async () => (await page.locator('.category-tab.active').innerText().catch(() => '')) === 'Transfers'));
	const protos = await page.locator('.protocol-toggle .auth-btn').allInnerTexts();
	check('Transfers has SFTP, SCP, FTP and S3', JSON.stringify(protos) === JSON.stringify(['SFTP', 'SCP', 'FTP / FTPS', 'S3 storage']), JSON.stringify(protos));
	check('FTP is the one picked', (await page.locator('.protocol-toggle .auth-btn.active').innerText()) === 'FTP / FTPS');
	check('the encryption choice is there', (await page.locator('#ftp-tls').count()) === 1);
	await page.locator('#ftp-tls').selectOption('implicit');
	check('implicit TLS moves the port to 990', (await page.locator('input[type=number]').first().inputValue()) === '990');
	await page.locator('#ftp-tls').selectOption('');
	await shot('editor-ftp');
	await page.locator('.protocol-toggle .auth-btn', { hasText: 'SFTP' }).click();
	check('SFTP takes SSH logins (key, agent)', await until(async () => (await page.getByText('Agent', { exact: false }).count()) > 0));
	check('and no terminal settings', (await page.locator('#session-term-type').count()) === 0);
	await shot('editor-sftp');
	await page.locator('#session-login').selectOption('cred-key');
	check('a saved SSH key login hides the own fields', await until(async () => (await page.getByLabel('Username').count()) === 0));
	await page.locator('.protocol-toggle .auth-btn', { hasText: 'FTP / FTPS' }).click();
	check('switching to FTP shows user name and password again', await until(async () => (await page.getByLabel('Username').count()) === 1 && (await page.getByText('Password (optional)').count()) > 0));
	check('and no login picker for FTP', (await page.locator('#session-login').count()) === 0);
	await page.keyboard.press('Escape');
	await sleep(300);

	console.log('\nFTP with a self-signed certificate');
	await card('Web hosting').hover();
	await card('Web hosting').locator('.connect-btn').click();
	check('an FTP tab asks about the certificate', await until(async () => (await page.locator('.ftp-panel .fp').count()) === 1));
	check('showing its fingerprint', (await page.locator('.ftp-panel .fp').innerText()) === FP);
	await shot('ftp-cert');
	await page.getByRole('button', { name: 'Trust and connect' }).click();
	check('trusting it keeps the fingerprint', await until(() => certTrusted));
	check('and the file manager opens at the login folder', await until(async () => (await page.locator('.pane').nth(1).innerText()).includes('index.html')));
	check('no way back to a terminal FTP has none of', !(await page.locator('.terminal-wrapper.active').innerText()).includes('F10'));
	await shot('ftp-files');

	console.log('\nSFTP');
	await card('Backup box').hover();
	await card('Backup box').locator('.connect-btn').click();
	check('an SFTP session connects over SSH', await until(() => calls.some((c) => c.cmd === 'ssh_connect' && c.args.host === '10.0.0.20')));
	const sshArgs = calls.find((c) => c.cmd === 'ssh_connect' && c.args.host === '10.0.0.20').args;
	check('without tmux, log or shell setup', !sshArgs.tmux && !sshArgs.sessionLog && !sshArgs.injectColors);
	check('and opens in the file manager', await until(async () => (await page.locator('.terminal-wrapper.active').innerText()).includes('restore.sh')));
	await shot('sftp-files');
} finally {
	await browser.close();
	vite.kill();
}

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
