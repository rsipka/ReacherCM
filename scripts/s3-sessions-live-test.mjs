/**
 * S3 storage sessions in the real app, in a browser page with a stand-in
 * for Tauri: the editor's Transfers tab and S3 fields (provider preset,
 * plain HTTP moving the port, addressing), a session saved with them, a
 * connect that asks for the secret key, then a certificate, then opens
 * the buckets in the full-pane file manager, and a share link from a
 * file's menu.
 *
 * Needs Playwright's Chromium; skips cleanly without it. Starts `vite dev`
 * on a free port. SHOT_DIR=dir saves screenshots.
 *
 * Run: node scripts/s3-sessions-live-test.mjs
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
const SESSIONS = [
	{ id: 's-s3', name: 'Backups bucket', host: 's3.example.com', port: 443, username: 'AKIAEXAMPLE', kind: 's3', folder_id: null, tags: [], auth_method: { type: 'Password', password: null }, s3: { region: 'eu-central-1', path_style: true, bucket: null, http: false } },
	{ id: 's-ssh', name: 'Web server', host: '10.0.0.10', port: 22, username: 'root', folder_id: null, tags: [], auth_method: { type: 'Agent' } }
];
const t0 = Math.floor(Date.now() / 1000);
const entry = (dir, name, isDirectory, size = 0, ago = 3600) => ({ name, path: `${dir === '/' ? '' : dir}/${name}`, isDirectory, size, modified: t0 - ago, permissions: '', owner: '', group: '' });
const REMOTE = {
	'/': [entry('/', 'backups', true), entry('/', 'media', true)],
	'/backups': [entry('/backups', 'daily', true), entry('/backups', 'db-2026-11-27.sql.gz', false, 48211234)]
};
const LOCAL = { path: '/home/me', parent: '/home', entries: [entry('/home/me', 'Documents', true), entry('/home/me', 'notes.txt', false, 120)] };
let certTrusted = false;
let secretGiven = false;

const browser = await chromium.launch({ args: ['--disable-webgl', '--disable-3d-apis'] });
const context = await browser.newContext({ viewport: { width: 1280, height: 780 }, deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: `http://localhost:${port}` });
const page = await context.newPage();
await page.addInitScript(() => {
	localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: [], checkForUpdates: false, serverMonitoring: false, theme: 'dark' }));
});
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
		case 'session_get':
			return SESSIONS.find((s) => s.id === args.sessionId);
		case 'session_create':
			return { id: 's-new', name: args.name, host: args.host, port: args.port, username: args.username, kind: args.kind, folder_id: null, tags: [], auth_method: args.authMethod, s3: args.s3 };
		case 's3_connect':
			if (!args.secret && !secretGiven) throw 'S3_SECRET_NEEDED';
			secretGiven = true;
			if (!certTrusted) throw `S3_UNTRUSTED_CERT:${FP}`;
			return { home: '/' };
		case 's3_session_cert_trust':
			certTrusted = args.fingerprint === FP;
			return null;
		case 's3_presign':
			return `https://s3.example.com${args.path}?X-Amz-Expires=${args.expires}&X-Amz-Signature=abc`;
		case 's3_disconnect':
			return null;
		case 'sftp_home':
			return '/';
		case 'sftp_list_dir':
			return REMOTE[args.path] ?? [];
		case 'local_list_dir':
			return LOCAL;
		case 'local_home':
			return '/home/me';
		case 'saved_credential_list':
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
	check('the sessions are listed', await until(async () => (await page.locator('.session-card').count()) === 2));
	check('an S3 session carries an S3 badge', (await card('Backups bucket').locator('.auth-badge').innerText()) === 'S3');

	console.log('\nthe editor');
	await card('Backups bucket').hover();
	await card('Backups bucket').locator('.edit-btn').click();
	check('the editor opens on Transfers', await until(async () => (await page.locator('.category-tab.active').innerText().catch(() => '')) === 'Transfers'));
	check('with S3 storage picked', (await page.locator('.protocol-toggle .auth-btn.active').innerText()) === 'S3 storage');
	check('the user name is the access key', (await page.getByLabel('Access key', { exact: true }).inputValue()) === 'AKIAEXAMPLE');
	check('its region and addressing are filled in', (await page.getByLabel('Region', { exact: true }).inputValue()) === 'eu-central-1' && (await page.locator('#s3-style').inputValue()) === 'path');
	await shot('editor');
	await page.keyboard.press('Escape');
	await sleep(300);

	console.log('\na new session');
	await page.keyboard.press('Control+Shift+N');
	check('the new session editor opens', await until(async () => (await page.locator('.category-tab').count()) > 0));
	await page.locator('.category-tab', { hasText: 'Transfers' }).click();
	await page.locator('.protocol-toggle .auth-btn', { hasText: 'S3 storage' }).click();
	const portInput = page.getByLabel('Port', { exact: true });
	check('S3 takes port 443', await until(async () => (await portInput.inputValue().catch(() => '')) === '443'));
	await page.locator('#s3-provider').selectOption('minio');
	check('the MinIO preset fills host and port', await until(async () => (await page.getByLabel('Host', { exact: true }).inputValue()) === 'minio.example.com' && (await portInput.inputValue()) === '9000'));
	check('with plain HTTP and path addressing', (await page.getByLabel('Plain HTTP (no TLS)').isChecked()) && (await page.locator('#s3-style').inputValue()) === 'path');
	await page.getByLabel('Plain HTTP (no TLS)').uncheck();
	await portInput.fill('80');
	await page.getByLabel('Plain HTTP (no TLS)').check();
	await page.getByLabel('Plain HTTP (no TLS)').uncheck();
	check('turning HTTP off moves port 80 to 443', (await portInput.inputValue()) === '443');
	await page.getByLabel('Session name', { exact: true }).fill('Lab store');
	await page.getByLabel('Access key', { exact: true }).fill('labkey');
	await page.getByLabel('Bucket', { exact: true }).fill('/photos/');
	await shot('editor-new');
	await page.locator('button', { hasText: 'Save session' }).click();
	check('it is saved as an S3 session', await until(() => calls.some((c) => c.cmd === 'session_create' && c.args.kind === 's3')));
	const created = calls.find((c) => c.cmd === 'session_create')?.args;
	check('with its options', created?.s3?.bucket === 'photos' && created?.s3?.path_style === true && created?.s3?.http === false && created?.username === 'labkey' && created?.port === 443, JSON.stringify(created));
	await page.keyboard.press('Escape');
	await sleep(300);

	console.log('\nconnecting');
	await card('Backups bucket').hover();
	await card('Backups bucket').locator('.connect-btn').click();
	check('the tab asks for the secret key', await until(async () => (await page.getByLabel('Secret key').count()) === 1));
	await page.getByLabel('Secret key').fill('s3cr3t');
	await page.getByRole('button', { name: 'Connect', exact: true }).click();
	check('it connects with the typed key', await until(() => calls.some((c) => c.cmd === 's3_connect' && c.args.secret === 's3cr3t')));
	check('then asks about the certificate', await until(async () => (await page.locator('.connect .fp').count()) === 1));
	check('showing its fingerprint', (await page.locator('.connect .fp').innerText()) === FP);
	await page.getByRole('button', { name: 'Trust and connect' }).click();
	check('trusting it keeps the fingerprint', await until(() => certTrusted));
	check('the buckets open in the file manager', await until(async () => (await page.locator('.pane').nth(1).innerText().catch(() => '')).includes('backups')));
	await page.locator('.pane').nth(1).locator('.row', { hasText: 'backups' }).dblclick();
	check('a bucket opens', await until(async () => (await page.locator('.pane').nth(1).innerText()).includes('db-2026-11-27.sql.gz')));
	await shot('files');

	console.log('\nshare links');
	await page.locator('.pane').nth(1).locator('.row', { hasText: 'db-2026-11-27' }).click({ button: 'right' });
	check('a file menu offers share links', await until(async () => (await page.getByText('Copy link (valid 1 day)').count()) === 1));
	await shot('share-menu');
	await page.getByText('Copy link (valid 1 day)').click();
	check('it asks for a 1-day link to that file', await until(() => calls.some((c) => c.cmd === 's3_presign' && c.args.path === '/backups/db-2026-11-27.sql.gz' && c.args.expires === 86400)));
	check('and copies it', await until(async () => (await page.evaluate(() => navigator.clipboard.readText()).catch(() => '')).includes('X-Amz-Expires=86400')));
	await page.keyboard.press('Escape');
	await page.locator('.pane').nth(1).locator('.row', { hasText: 'daily' }).click({ button: 'right' });
	await sleep(300);
	check('a folder has no share links', (await page.getByText('Copy link (valid 1 day)').count()) === 0);
	await page.keyboard.press('Escape');

	console.log('\nclosing');
	const last = calls.filter((c) => c.cmd === 's3_connect').at(-1)?.args.id;
	await page.locator('.tab.active .tab-close, .tab.active [aria-label*="Close"]').first().click().catch(() => {});
	check('closing the tab disconnects', await until(() => calls.some((c) => c.cmd === 's3_disconnect' && c.args.connectionId === last)));
} finally {
	await browser.close();
	vite.kill();
}

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
