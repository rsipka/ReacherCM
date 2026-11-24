/**
 * Server management (Redfish) sessions in the real app, in a browser page
 * with a stand-in for Tauri that answers like an iDRAC: the editor, a
 * self-signed certificate trusted on first connect, the overview, sensors,
 * hardware and logs pages, and the tasks page (power with confirmation,
 * boot once, virtual media, jobs). Closing the tab logs out.
 *
 * Needs Playwright's Chromium; skips cleanly without it. Starts `vite dev`
 * on a free port. SHOT_DIR=dir saves screenshots.
 *
 * Run: node scripts/bmc-sessions-live-test.mjs
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
	{ id: 's-idrac', name: 'R640 rack 3', host: '10.0.0.30', port: 443, username: 'root', kind: 'bmc', folder_id: null, tags: [], auth_method: { type: 'Password', password: 'x' } },
	{ id: 's-ssh', name: 'Web server', host: '10.0.0.10', port: 22, username: 'root', folder_id: null, tags: [], auth_method: { type: 'Agent' } }
];
const OVERVIEW = {
	vendor: 'dell', product: 'Integrated Dell Remote Access Controller', redfishVersion: '1.17.0', model: 'PowerEdge R640', manufacturer: 'Dell Inc.',
	serial: 'CN0000000000000', sku: 'ABC1234', biosVersion: '2.19.1', hostName: 'r640.example.com', powerState: 'On', health: 'OK', healthRollup: 'Warning',
	cpuCount: 2, cpuModel: 'Intel(R) Xeon(R) Gold 6130 CPU @ 2.10GHz', memoryGib: 192, identify: false, bmcModel: '14G Monolithic', bmcFirmware: '7.00.00.172',
	bmcTime: '2026-11-24T10:02:00+01:00', powerWatts: 182, inletTemp: 22,
	resetTypes: ['On', 'ForceOff', 'ForceRestart', 'GracefulRestart', 'GracefulShutdown', 'PushPowerButton', 'Nmi', 'PowerCycle'],
	bootTargets: ['None', 'Pxe', 'Cd', 'Hdd', 'BiosSetup', 'UefiHttp'], bootTarget: 'None', bootEnabled: 'Disabled', canResetBmc: true
};
const s = (name, reading, units, health = 'OK', warning = null, critical = null) => ({ name, reading, units, health, state: 'Enabled', warning, critical });
const SENSORS = {
	temperatures: [s('System Board Inlet Temp', 22, '°C', 'OK', 38, 42), s('System Board Exhaust Temp', 37, '°C', 'OK', 70, 75), s('CPU1 Temp', 51, '°C', 'OK', null, 93), s('CPU2 Temp', 49, '°C', 'OK', null, 93)],
	fans: [s('System Board Fan1A', 7080, 'RPM'), s('System Board Fan2A', 7200, 'RPM'), s('System Board Fan3A', 960, 'RPM', 'Warning', null, 600)],
	voltages: [s('CPU1 VCORE PG', 1, 'V'), s('System Board 3.3V PG', 3.3, 'V')],
	powerSupplies: [
		{ name: 'PS1 Status', detail: 'PWR SPLY,750W,RDNT,DELTA · 750 W · in 230 V · FW 00.1B.53', health: 'OK', state: 'Enabled' },
		{ name: 'PS2 Status', detail: 'PWR SPLY,750W,RDNT,DELTA · 750 W', health: 'Critical', state: 'Enabled' }
	],
	powerWatts: 182
};
const item = (name, detail, health = 'OK') => ({ name, detail, health, state: 'Enabled' });
const HARDWARE = {
	processors: [item('CPU.Socket.1', 'Intel Xeon Gold 6130 · 16 cores · 32 threads · 3.7 GHz'), item('CPU.Socket.2', 'Intel Xeon Gold 6130 · 16 cores · 32 threads · 3.7 GHz')],
	memory: Array.from({ length: 6 }, (_, i) => item(`DIMM.Socket.A${i + 1}`, '32 GB · DDR4 · 2666 MT/s · Hynix')),
	storage: [item('PERC H730P Mini', 'PERC H730P Mini · FW 25.5.9.0001')],
	drives: [item('Physical Disk 0:1:0', '480 GB · SSD · SATA · 97% life left'), item('Physical Disk 0:1:1', '480 GB · SSD · SATA · failure predicted', 'Warning')],
	volumes: [item('Virtual Disk 0', '480 GB · RAID1')],
	network: [item('NIC.Integrated.1-1-1', '00:00:5E:00:53:01 · 10 Gb/s · LinkUp · 10.0.0.31'), item('NIC.Integrated.1-2-1', '00:00:5E:00:53:02 · LinkDown')],
	firmware: [item('BIOS', '2.19.1'), item('Integrated Dell Remote Access Controller', '7.00.00.172'), item('PERC H730P Mini', '25.5.9.0001')]
};
const LOGS = [
	{ path: '/redfish/v1/Managers/iDRAC.Embedded.1/LogServices/Sel', name: 'Log Service SEL', canClear: true },
	{ path: '/redfish/v1/Managers/iDRAC.Embedded.1/LogServices/Lclog', name: 'Log Service Lclog', canClear: false }
];
const ENTRIES = [
	{ id: '9', created: '2026-11-23T21:14:03+01:00', severity: 'Critical', message: 'Power supply 2 has lost its input.', messageId: 'PSU0003', sensor: 'Power Supply' },
	{ id: '8', created: '2026-11-23T21:13:58+01:00', severity: 'Warning', message: 'Fan 3A RPM is less than the lower warning threshold.', messageId: 'FAN0000', sensor: 'Fan' },
	{ id: '7', created: '2026-11-20T08:02:11+01:00', severity: 'OK', message: 'The process of installing an operating system or hypervisor is successfully completed.', messageId: 'OSE1002', sensor: 'OS' },
	{ id: '6', created: '2026-11-19T17:40:00+01:00', severity: 'OK', message: 'Log cleared.', messageId: 'SEL0010', sensor: 'Event Log' }
];
const MEDIA = [
	{ path: '/redfish/v1/Systems/System.Embedded.1/VirtualMedia/1', name: 'Virtual CD', mediaTypes: ['CD', 'DVD'], image: null, inserted: false },
	{ path: '/redfish/v1/Systems/System.Embedded.1/VirtualMedia/2', name: 'Virtual Removable Disk', mediaTypes: ['USBStick'], image: null, inserted: false }
];
const JOBS = [
	{ id: 'JID_001', name: 'Firmware Update: BIOS', state: 'Completed', percent: 100, message: 'Job completed successfully.', start: 'TIME_NOW', end: '2026-11-20T09:12:00' },
	{ id: 'JID_002', name: 'Configure: RAID.Integrated.1-1', state: 'Scheduled', percent: 0, message: 'Task successfully scheduled.', start: 'TIME_NOW', end: null }
];
let certTrusted = false;

const browser = await chromium.launch({ args: ['--disable-webgl', '--disable-3d-apis'] });
const page = await browser.newPage({ viewport: { width: 1280, height: 780 }, deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
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
			return SESSIONS.find((x) => x.id === args.sessionId);
		case 'bmc_connect':
			if (!certTrusted) throw `BMC_UNTRUSTED_CERT:${FP}`;
			return { vendor: 'dell', product: 'Integrated Dell Remote Access Controller' };
		case 'bmc_session_cert_trust':
			certTrusted = args.fingerprint === FP;
			return null;
		case 'bmc_overview':
			return OVERVIEW;
		case 'bmc_sensors':
			return SENSORS;
		case 'bmc_hardware':
			return HARDWARE;
		case 'bmc_log_services':
			return LOGS;
		case 'bmc_log_entries':
			return args.path.endsWith('Sel') ? ENTRIES : ENTRIES.slice(2);
		case 'bmc_virtual_media':
			return MEDIA;
		case 'bmc_jobs':
			return JOBS;
		case 'bmc_power':
			return null;
		case 'bmc_boot_once':
			OVERVIEW.bootTarget = args.target;
			OVERVIEW.bootEnabled = args.target === 'None' ? 'Disabled' : 'Once';
			return null;
		case 'bmc_media_insert':
			Object.assign(MEDIA.find((m) => m.path === args.path), { image: args.image, inserted: true });
			return null;
		case 'bmc_disconnect':
			return null;
		case 'session_list_folders':
		case 'vault_list':
		case 'vault_duplicates':
		case 'ssh_list_connections':
		case 'snippet_list':
		case 'theme_list_installed':
		case 'credential_folder_list':
		case 'saved_credential_list':
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
const tab = (name) => page.locator('.page-tab', { hasText: name }).first();
const body = () => page.locator('.bmc .body').innerText().catch(() => '');

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	check('the sessions are listed', await until(async () => (await page.locator('.session-card').count()) === 2));
	check('a server management session carries a Redfish badge', (await card('R640').locator('.auth-badge').innerText()).toLowerCase() === 'redfish');

	console.log('\nthe editor');
	await card('R640').hover();
	await card('R640').locator('.edit-btn').click();
	check('the editor opens on Other', await until(async () => (await page.locator('.category-tab.active').innerText().catch(() => '')) === 'Other'));
	check('with Server management picked', (await page.locator('.protocol-toggle .auth-btn.active').innerText()) === 'Server management (iDRAC, iLO)');
	check('and a password field', (await page.getByText('Password', { exact: true }).count()) > 0);
	check('and a saved login picker', (await page.locator('#session-login').count()) === 1);
	await shot('editor');
	await page.keyboard.press('Escape');
	await sleep(300);

	console.log('\nconnecting, with a self-signed certificate');
	await card('R640').hover();
	await card('R640').locator('.connect-btn').click();
	check('the tab asks about the certificate', await until(async () => (await page.locator('.connect .fp').count()) === 1));
	check('showing its fingerprint', (await page.locator('.connect .fp').innerText()) === FP);
	await shot('cert');
	await page.getByRole('button', { name: 'Trust and connect' }).click();
	check('trusting it keeps the fingerprint', await until(() => certTrusted));
	check('the overview shows the server', await until(async () => (await body()).includes('PowerEdge R640')));
	const ov = await body();
	check('with the rolled-up health and power draw', ov.includes('Warning') && ov.includes('182 W'), ov);
	check('and the service tag', ov.includes('ABC1234'));
	await shot('overview');

	console.log('\nsensors and hardware');
	await tab('Sensors').click();
	check('sensors list temperatures and fans', await until(async () => (await body()).includes('System Board Inlet Temp') && (await body()).includes('7080 RPM')));
	check('with limits', (await body()).includes('38 / 42'));
	await shot('sensors');
	await tab('Hardware').click();
	check('hardware lists memory and drives', await until(async () => (await body()).includes('DIMM.Socket.A1') && (await body()).includes('failure predicted')));
	await shot('hardware');

	console.log('\nlogs');
	await tab('Logs').click();
	check('the first log is read', await until(async () => (await body()).includes('Power supply 2 has lost its input.')));
	await page.locator('.logs select').nth(1).selectOption('critical');
	check('errors only leaves the critical entry', await until(async () => (await page.locator('.logs tbody tr').count()) === 1));
	await page.locator('.logs select').nth(1).selectOption('all');
	await shot('logs');
	await page.getByRole('button', { name: 'Clear log' }).click();
	check('clearing asks first', await until(async () => (await page.locator('.modal').innerText().catch(() => '')).includes('cannot be brought back')));
	await page.locator('.modal').getByRole('button', { name: 'Cancel' }).click();
	check('and nothing is cleared on cancel', !calls.some((c) => c.cmd === 'bmc_log_clear'));

	console.log('\ntasks');
	await tab('Tasks').click();
	check('power buttons are listed', await until(async () => (await page.locator('.tasks .buttons').first().innerText().catch(() => '')).includes('Shut down')));
	await page.getByRole('button', { name: 'Force off' }).click();
	check('a forced power off asks, and warns', await until(async () => (await page.locator('.modal').innerText().catch(() => '')).includes('pulling its plug')));
	await shot('power-confirm');
	await page.locator('.modal').getByRole('button', { name: 'Force off' }).click();
	check('and sends ForceOff once confirmed', await until(() => calls.some((c) => c.cmd === 'bmc_power' && c.args.resetType === 'ForceOff')));
	await page.locator('.tasks select').first().selectOption('Pxe');
	await page.getByRole('button', { name: 'Set', exact: true }).click();
	check('boot once is set to PXE', await until(() => calls.some((c) => c.cmd === 'bmc_boot_once' && c.args.target === 'Pxe')));
	check('and shown as the next start', await until(async () => (await body()).includes('Network (PXE)') && (await body()).includes('Cancel boot once')));
	await page.locator('.media input').first().fill('http://10.0.0.1/isos/install.iso');
	await page.getByRole('button', { name: 'Mount' }).first().click();
	check('an image is mounted', await until(() => calls.some((c) => c.cmd === 'bmc_media_insert' && c.args.image === 'http://10.0.0.1/isos/install.iso')));
	check('and shown with Eject', await until(async () => (await page.getByRole('button', { name: 'Eject' }).count()) === 1));
	check('jobs are listed', (await body()).includes('Configure: RAID.Integrated.1-1'));
	await shot('tasks');

	console.log('\nclosing');
	const id = calls.find((c) => c.cmd === 'bmc_connect' && certTrusted)?.args.id;
	await page.locator('.tab.active .tab-close, .tab.active [aria-label*="Close"]').first().click().catch(() => {});
	check('closing the tab logs out', await until(() => calls.some((c) => c.cmd === 'bmc_disconnect')), JSON.stringify(id));
} finally {
	await browser.close();
	vite.kill();
}

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
