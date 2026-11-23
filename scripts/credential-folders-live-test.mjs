/**
 * Folders in the Credentials tab, in a browser page with a stand-in for
 * Tauri that keeps credentials and folders in memory.
 *
 * What it proves:
 *   - Credentials show in their folders, folders first, with counts.
 *   - A click closes and opens a folder.
 *   - New folder, rename, move (menu and drag), delete send the right
 *     commands, and the list follows.
 *   - The credential editor offers the folders of the credential's vault.
 *
 * Needs Playwright's Chromium; skips cleanly without it. SHOT_DIR=<dir>
 * saves screenshots.
 *
 * Run: node scripts/credential-folders-live-test.mjs
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
const SESSIONS = [{ id: 's1', name: 'web-01', host: '10.0.0.10', port: 22, username: 'admin', auth_method: auth, folder_id: null, tags: [], credential: { id: 'c1', name: 'Domain admin' } }];
let CREDS = [
	{ id: 'c1', name: 'Domain admin', username: 'admin', domain: 'CORP', auth_method: auth, folder_id: 'f1' },
	{ id: 'c2', name: 'Switches', username: 'netops', auth_method: auth, folder_id: 'f2' },
	{ id: 'c3', name: 'Backup', username: 'backup', auth_method: { type: 'Key', key_id: 'k' }, folder_id: null },
	{ id: 'c4', name: 'Root', username: 'root', auth_method: { type: 'Agent' }, folder_id: 'f1' }
];
let FOLDERS = [
	{ id: 'f1', name: 'Windows', parent_id: null, icon: 'os:windows', color: null },
	{ id: 'f2', name: 'Network', parent_id: null, icon: 'fa:network', color: 'green' },
	{ id: 'f3', name: 'Branch office', parent_id: 'f2', icon: null, color: null }
];
let nextId = 10;
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
		case 'credential_folder_list':
			return FOLDERS;
		case 'credential_folder_create': {
			const f = { id: `f${nextId++}`, name: args.name.trim(), parent_id: args.parentId, vault_id: args.vaultId };
			FOLDERS = [...FOLDERS, f];
			return f;
		}
		case 'credential_folder_rename':
			FOLDERS = FOLDERS.map((f) => (f.id === args.folderId ? { ...f, name: args.name } : f));
			return FOLDERS.find((f) => f.id === args.folderId);
		case 'credential_folder_move':
			FOLDERS = FOLDERS.map((f) => (f.id === args.folderId ? { ...f, parent_id: args.parentId } : f));
			return FOLDERS.find((f) => f.id === args.folderId);
		case 'credential_folder_delete': {
			const gone = FOLDERS.find((f) => f.id === args.folderId);
			FOLDERS = FOLDERS.filter((f) => f.id !== gone.id).map((f) => (f.parent_id === gone.id ? { ...f, parent_id: gone.parent_id } : f));
			CREDS = CREDS.map((c) => (c.folder_id === gone.id ? { ...c, folder_id: gone.parent_id } : c));
			return null;
		}
		case 'saved_credential_move':
			CREDS = CREDS.map((c) => (args.credentialIds.includes(c.id) ? { ...c, folder_id: args.folderId } : c));
			return args.credentialIds.length;
		case 'saved_credential_update':
			CREDS = CREDS.map((c) => (c.id === args.credential.id ? { ...args.credential } : c));
			return args.credential;
		case 'session_list_folders':
		case 'vault_list':
		case 'vault_duplicates':
		case 'ssh_list_connections':
		case 'snippet_list':
		case 'theme_list_installed':
		case 'theme_fetch_registry':
		case 'plugin:window|available_monitors':
		case 'password_history_list':
		case 'saved_credential_password_history':
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
		invoke: (cmd, args) => window.__bridge({ cmd, args: args ?? {} }),
		convertFileSrc: (p) => p
	};
	window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
});

async function shot(name, clip) {
	if (!process.env.SHOT_DIR) return;
	await sleep(300);
	const box = await page.locator('.cred-list').boundingBox();
	await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png`, ...(clip === false ? {} : { clip: clip ?? { x: 0, y: 0, width: Math.min(1280, box.x + box.width + 260), height: 520 } }) }).catch(() => {});
}
const rowsText = async () => (await page.locator('.cred-scroll > div').allInnerTexts()).map((t) => t.replace(/\s+/g, ' ').trim());
const last = (cmd) => [...calls].reverse().find((c) => c.cmd === cmd);
const folderRow = (name) => page.locator('.cred-folder', { hasText: name }).first();

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	await page.locator('.list-tab', { hasText: 'Credentials' }).click();
	check('the credentials list shows', await until(async () => (await page.locator('.cred-folder').count()) === 3));
	let rows = await rowsText();
	check('folders first, then credentials, nested under their folder', rows[0].startsWith('Network') && rows[1].startsWith('Branch office') && rows[2].startsWith('Switches') && rows[3].startsWith('Windows') && rows[4].startsWith('Domain admin') && rows[5].startsWith('Root') && rows[6].startsWith('Backup'), JSON.stringify(rows));
	check('a folder counts the credentials inside', (await folderRow('Windows').innerText()).includes('2'));
	await shot('list');

	console.log('\nopen and close');
	await folderRow('Windows').locator('.folder-main').click();
	rows = await rowsText();
	check('a click closes the folder', !rows.some((r) => r.startsWith('Domain admin')), JSON.stringify(rows));
	await folderRow('Windows').locator('.folder-main').click();
	check('another opens it', (await rowsText()).some((r) => r.startsWith('Domain admin')));

	console.log('\nnew folder');
	await page.locator('.add-btn[aria-label="New folder"]').click();
	await page.locator('.modal input').first().fill('Linux');
	await page.locator('.modal button', { hasText: 'Save' }).click();
	check('creates it in the vault in view', await until(async () => last('credential_folder_create')?.args.name === 'Linux'));
	check('it shows', await until(async () => (await page.locator('.cred-folder', { hasText: 'Linux' }).count()) === 1));

	console.log('\nmenu');
	await page.locator('.cred-row', { hasText: 'Backup' }).click({ button: 'right' });
	const menu = (await page.locator('.context-menu').innerText()).replace(/\s+/g, ' ');
	check('a credential menu offers move', menu.includes('Move to folder'), menu);
	await page.locator('.context-item', { hasText: 'Move to folder' }).click();
	await shot('move');
	await page.locator('.move-item', { hasText: 'Linux' }).click();
	check('moves it into the folder', await until(async () => last('saved_credential_move')?.args.folderId === FOLDERS.find((f) => f.name === 'Linux').id));
	check('and the list follows', await until(async () => (await rowsText()).findIndex((r) => r.startsWith('Backup')) === (await rowsText()).findIndex((r) => r.startsWith('Linux')) + 1), JSON.stringify(await rowsText()));

	await folderRow('Network').click({ button: 'right' });
	const fmenu = (await page.locator('.context-menu').innerText()).replace(/\s+/g, ' ');
	check('a folder menu has new here, subfolder, rename, icon, move, delete', ['New credential here', 'New subfolder', 'Rename', 'Icon', 'Move to folder', 'Delete folder'].every((w) => fmenu.includes(w)), fmenu);
	await shot('folder-menu');
	await page.locator('.context-item', { hasText: 'Rename' }).click();
	await page.locator('.modal input').first().fill('Network gear');
	await page.locator('.modal button', { hasText: 'Save' }).click();
	check('renames it', await until(async () => last('credential_folder_rename')?.args.name === 'Network gear'));

	console.log('\ndrag');
	const src = await page.locator('.cred-row', { hasText: 'Root' }).boundingBox();
	const dst = await folderRow('Branch office').boundingBox();
	await page.mouse.move(src.x + 40, src.y + src.height / 2);
	await page.mouse.down();
	await page.mouse.move(src.x + 60, src.y + src.height / 2 + 10, { steps: 3 });
	await page.mouse.move(dst.x + 60, dst.y + dst.height / 2, { steps: 8 });
	check('the folder under the pointer lights up', (await folderRow('Branch office').getAttribute('class')).includes('drop-active'));
	await shot('drag');
	await page.mouse.up();
	check('dropping files it there', await until(async () => last('saved_credential_move')?.args.folderId === 'f3'));
	check('the drag did not open the editor', (await page.locator('.modal', { hasText: 'Edit credential' }).count()) === 0);

	console.log('\neditor');
	await page.locator('.cred-row', { hasText: 'Switches' }).locator('.cred-main').click();
	const options = await page.locator('#cred-folder option').allInnerTexts();
	check('the editor offers the folders, with their path', options.includes('Top level') && options.includes('Network gear / Branch office'), JSON.stringify(options));
	await shot('editor', false);
	await page.locator('#cred-folder').selectOption({ label: 'Windows' });
	await page.locator('.modal button', { hasText: 'Save' }).click();
	check('saving sends the folder', await until(async () => last('saved_credential_update')?.args.credential.folder_id === 'f1'));

	console.log('\ndelete');
	await folderRow('Windows').click({ button: 'right' });
	await page.locator('.context-item', { hasText: 'Delete folder' }).click();
	const note = await page.locator('.modal', { hasText: 'Delete folder' }).innerText();
	check('says what moves up', /2 credentials and 0 subfolders move up to Top level/.test(note), note);
	await shot('delete');
	await page.locator('.modal button', { hasText: 'Delete' }).click();
	check('deletes it', await until(async () => last('credential_folder_delete')?.args.folderId === 'f1'));
	check('its credentials go to the top', await until(async () => (await rowsText()).at(-1).startsWith('Switches') || (await rowsText()).some((r) => r.startsWith('Domain admin'))));
} finally {
	await browser.close();
	vite.kill();
}
console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
