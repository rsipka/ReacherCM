/**
 * MQTT broker sessions in the real app, in a browser page with a stand-in
 * for Tauri that answers like a broker: the editor (transport and port,
 * topics), a self-signed certificate trusted on first connect, the topic
 * tree filling from message batches, the payload views, publishing,
 * subscribing and unsubscribing, a dropped connection, and closing.
 *
 * Needs Playwright's Chromium; skips cleanly without it. Starts `vite dev`
 * on a free port. SHOT_DIR=dir saves screenshots.
 *
 * Run: node scripts/mqtt-sessions-live-test.mjs
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
	{ id: 's-mqtt', name: 'Home broker', host: 'broker.example.com', port: 8883, username: 'iot', kind: 'mqtt', folder_id: null, tags: [], auth_method: { type: 'Password', password: 'x' }, mqtt: { transport: 'tls', subscriptions: ['home/#', '$SYS/#'] } },
	{ id: 's-ssh', name: 'Web server', host: '10.0.0.10', port: 22, username: 'root', folder_id: null, tags: [], auth_method: { type: 'Agent' } }
];
let certTrusted = false;

const browser = await chromium.launch({ args: ['--disable-webgl', '--disable-3d-apis'] });
const page = await browser.newPage({ viewport: { width: 1280, height: 780 }, deviceScaleFactor: process.env.SHOT_DIR ? 2 : 1 });
await page.addInitScript(() => {
	localStorage.setItem('reach-settings', JSON.stringify({ setupComplete: true, seenFeatures: [], checkForUpdates: false, serverMonitoring: false, theme: 'dark' }));
});
const calls = [];
/** Event name -> callback ids listening. */
const listeners = new Map();
page.on('pageerror', (e) => console.log(`  page error: ${e.message}`));
await page.exposeBinding('__bridge', async (_src, { cmd, args }) => {
	calls.push({ cmd, args });
	switch (cmd) {
		case 'plugin:event|listen':
			listeners.set(args.event, [...(listeners.get(args.event) ?? []), args.handler]);
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
		case 'session_create':
			return { id: 's-new', name: args.name, host: args.host, port: args.port, username: args.username, kind: args.kind, folder_id: null, tags: [], auth_method: args.authMethod, mqtt: args.mqtt };
		case 'mqtt_connect':
			if (!certTrusted) throw `MQTT_UNTRUSTED_CERT:${FP}`;
			return { clientId: 'reachercm-1a2b3c4d', filters: ['home/#', '$SYS/#'] };
		case 'mqtt_session_cert_trust':
			certTrusted = args.fingerprint === FP;
			return null;
		case 'mqtt_publish':
		case 'mqtt_unsubscribe':
		case 'mqtt_disconnect':
			return null;
		case 'mqtt_subscribe':
			return args.filters.map((f) => (f.startsWith('secret') ? 128 : 1));
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

/** Hand the page an event, as the backend's emit would. */
async function emit(event, payload) {
	const ids = listeners.get(event) ?? [];
	await page.evaluate(({ ids, event, payload }) => {
		for (const id of ids) window.__callbacks.get(id)?.({ event, id: 0, payload });
	}, { ids, event, payload });
}
const b64 = (s) => Buffer.from(s).toString('base64');
const msg = (topic, payload, retain = false, qos = 0) => ({ topic, payload: b64(payload), size: Buffer.byteLength(payload), qos, retain, time: Date.UTC(2026, 10, 27, 12, 0, 0) });

async function shot(name) {
	if (!process.env.SHOT_DIR) return;
	await sleep(400);
	await page.screenshot({ path: `${process.env.SHOT_DIR}/${name}.png` }).catch(() => {});
}
const card = (name) => page.locator('.session-card', { hasText: name }).first();
const rowsText = () => page.locator('.mqtt .tree').innerText().catch(() => '');
const side = () => page.locator('.mqtt .side').innerText().catch(() => '');

try {
	await page.goto(`http://localhost:${port}/`);
	await page.waitForSelector('.tabbar', { timeout: 30000 });
	check('the sessions are listed', await until(async () => (await page.locator('.session-card').count()) === 2));
	check('an MQTT-over-TLS session carries an MQTTS badge', (await card('Home broker').locator('.auth-badge').innerText()) === 'MQTTS');

	console.log('\nthe editor');
	await card('Home broker').hover();
	await card('Home broker').locator('.edit-btn').click();
	check('the editor opens on Other', await until(async () => (await page.locator('.category-tab.active').innerText().catch(() => '')) === 'Other'));
	check('with MQTT broker picked', (await page.locator('.protocol-toggle .auth-btn.active').innerText()) === 'MQTT broker');
	check('TLS as the transport', (await page.locator('#mqtt-transport').inputValue()) === 'tls');
	check('and its topics, one a line', (await page.locator('#mqtt-subs').inputValue()) === 'home/#\n$SYS/#');
	check('a client certificate can be given over TLS', (await page.locator('#mqtt-cert').count()) === 1);
	await shot('editor');
	await page.keyboard.press('Escape');
	await sleep(300);

	console.log('\na new session');
	await page.keyboard.press('Control+Shift+N');
	check('the new session editor opens', await until(async () => (await page.locator('.category-tab').count()) > 0));
	await page.locator('.category-tab', { hasText: 'Other' }).click();
	await page.locator('.protocol-toggle .auth-btn', { hasText: 'MQTT broker' }).click();
	const portInput = page.getByLabel('Port', { exact: true });
	check('MQTT takes port 1883', await until(async () => (await portInput.inputValue().catch(() => '')) === '1883'));
	await page.locator('#mqtt-transport').selectOption('wss');
	check('secure WebSocket moves it to 443', await until(async () => (await portInput.inputValue()) === '443'));
	check('and asks for the WebSocket path', (await page.getByLabel('WebSocket path').count()) === 1);
	await page.getByLabel('Session name', { exact: true }).fill('Cloud broker');
	await page.getByLabel('Host', { exact: true }).fill('mqtt.example.com');
	await page.getByLabel('WebSocket path').fill('/ws');
	await page.locator('#mqtt-subs').fill('devices/+/state\n');
	await shot('editor-new');
	await page.locator('button', { hasText: 'Save session' }).click();
	check('it is saved as an MQTT session', await until(() => calls.some((c) => c.cmd === 'session_create' && c.args.kind === 'mqtt')));
	const created = calls.find((c) => c.cmd === 'session_create')?.args;
	check('with its transport, path and topics', created?.mqtt?.transport === 'wss' && created?.mqtt?.ws_path === '/ws' && created?.mqtt?.subscriptions?.join() === 'devices/+/state' && created?.port === 443, JSON.stringify(created?.mqtt));
	await page.keyboard.press('Escape');
	await sleep(300);

	console.log('\nconnecting, with a self-signed certificate');
	await card('Home broker').hover();
	await card('Home broker').locator('.connect-btn').click();
	check('the tab asks about the certificate', await until(async () => (await page.locator('.connect .fp').count()) === 1));
	check('showing its fingerprint', (await page.locator('.connect .fp').innerText()) === FP);
	await page.getByRole('button', { name: 'Trust and connect' }).click();
	check('trusting it keeps the fingerprint', await until(() => certTrusted));
	check('the explorer opens', await until(async () => (await page.locator('.mqtt .bar').count()) === 1));
	const id = calls.filter((c) => c.cmd === 'mqtt_connect').at(-1)?.args.id;
	check('it listened before connecting', calls.findIndex((c) => c.cmd === 'plugin:event|listen' && c.args.event === `mqtt-event-${id}`) < calls.findLastIndex((c) => c.cmd === 'mqtt_connect'));
	check('the subscriptions are shown', (await page.locator('.mqtt .chip').allInnerTexts()).map((s) => s.replace('×', '').trim()).join() === 'home/#,$SYS/#');
	check('it waits for messages', (await rowsText()).includes('Waiting for messages'));

	console.log('\nmessages');
	await emit(`mqtt-event-${id}`, {
		type: 'messages',
		messages: [
			msg('home/kitchen/temperature', '{"value":21.5,"unit":"C"}', true, 1),
			msg('home/kitchen/light', 'on'),
			msg('home/hall/motion', 'false'),
			msg('home/kitchen/temperature', '{"value":21.7,"unit":"C"}', true, 1),
			msg('$SYS/broker/uptime', '3600 seconds', true),
			msg('home/firmware/blob', '\u0000\u0001ÿ')
		]
	});
	check('the top levels appear', await until(async () => (await rowsText()).includes('home') && (await rowsText()).includes('$SYS')));
	check('with what is below them counted', (await page.locator('.mqtt .row', { hasText: 'home' }).first().locator('.num').innerText()) === '4·5');
	await page.locator('.mqtt .row', { hasText: /^▸\s*home/ }).first().click();
	await page.locator('.mqtt .row', { hasText: 'kitchen' }).first().click();
	check('opening levels shows topics and their last value', await until(async () => (await rowsText()).includes('{"value":21.7,"unit":"C"}')));
	await page.locator('.mqtt .row', { hasText: 'temperature' }).first().click();
	check('a topic shows its full name', await until(async () => (await page.locator('.mqtt .topic').innerText().catch(() => '')) === 'home/kitchen/temperature'));
	check('its last message as indented JSON', (await page.locator('.mqtt .payload').innerText()).includes('"value": 21.7'));
	check('retained, at QoS 1', (await side()).includes('Retained') && (await side()).includes('QoS 1'));
	check('and its history', (await page.locator('.mqtt .hist').count()) === 2);
	await shot('explorer');
	await page.locator('.mqtt .hist').nth(1).click();
	check('picking an older message shows it', await until(async () => (await page.locator('.mqtt .payload').innerText()).includes('21.5')));
	await page.locator('.mqtt .views button', { hasText: 'Hex' }).click();
	check('the hex view dumps the bytes', (await page.locator('.mqtt .payload').innerText()).startsWith('00000000  7b 22 76 61'));
	await page.locator('.mqtt .views button', { hasText: 'Auto' }).click();

	console.log('\nfinding topics');
	await page.locator('.mqtt .search').fill('motion');
	check('a search opens the way to its matches', await until(async () => {
		const t = await rowsText();
		return t.includes('motion') && !t.includes('temperature') && !t.includes('$SYS');
	}));
	await page.locator('.mqtt .search').fill('');

	console.log('\npublishing');
	await page.getByRole('button', { name: 'Use in publish' }).click();
	check('Use in publish fills the topic and message', (await page.getByLabel('Topic', { exact: true }).inputValue()) === 'home/kitchen/temperature' && (await page.getByLabel('Message', { exact: true }).inputValue()).includes('21.5'));
	await page.getByLabel('Topic', { exact: true }).fill('home/kitchen/light/set');
	await page.getByLabel('Message', { exact: true }).fill('off');
	await page.locator('.publish select').nth(1).selectOption('1');
	await page.locator('.publish input[type="checkbox"]').uncheck();
	await page.locator('.publish button[type="submit"]').click();
	const pub = () => calls.find((c) => c.cmd === 'mqtt_publish' && c.args.topic === 'home/kitchen/light/set');
	check('it publishes the message', await until(() => !!pub()));
	check('as UTF-8, at QoS 1, not retained', pub()?.args.payload === b64('off') && pub()?.args.qos === 1 && pub()?.args.retain === false, JSON.stringify(pub()?.args));
	await page.locator('.publish select').first().selectOption('hex');
	await page.getByLabel('Message', { exact: true }).fill('01 ff');
	await page.locator('.publish button[type="submit"]').click();
	check('hex is sent as bytes', await until(() => calls.some((c) => c.cmd === 'mqtt_publish' && c.args.payload === Buffer.from([1, 255]).toString('base64'))));
	await page.getByLabel('Topic', { exact: true }).fill('home/#');
	await page.locator('.publish button[type="submit"]').click();
	check('a wildcard topic is not published to', await until(async () => (await page.locator('.toast, [role="alert"]').allInnerTexts()).join().includes('cannot hold')) && !calls.some((c) => c.cmd === 'mqtt_publish' && c.args.topic === 'home/#'));
	await page.getByRole('button', { name: 'Clear retained' }).click();
	check('Clear retained publishes an empty retained message', await until(() => calls.some((c) => c.cmd === 'mqtt_publish' && c.args.topic === 'home/kitchen/temperature' && c.args.payload === '' && c.args.retain === true)));

	console.log('\nsubscriptions');
	await page.getByLabel('Subscribe', { exact: true }).fill('garden/+/moisture');
	await page.locator('.add-filter button[type="submit"]').click();
	check('a new filter is subscribed', await until(() => calls.some((c) => c.cmd === 'mqtt_subscribe' && c.args.filters.join() === 'garden/+/moisture')));
	check('and shown', await until(async () => (await page.locator('.mqtt .chip').count()) === 3));
	await page.getByLabel('Subscribe', { exact: true }).fill('secret/#');
	await page.locator('.add-filter button[type="submit"]').click();
	check('one the broker refuses is not added', await until(async () => (await page.locator('.toast, [role="alert"]').allInnerTexts()).join().includes('refused')) && (await page.locator('.mqtt .chip').count()) === 3);
	await page.locator('.mqtt .chip', { hasText: '$SYS/#' }).getByRole('button').click();
	check('× unsubscribes', await until(() => calls.some((c) => c.cmd === 'mqtt_unsubscribe' && c.args.filters.join() === '$SYS/#')));
	check('and drops the chip', await until(async () => (await page.locator('.mqtt .chip').count()) === 2));
	await shot('subscriptions');

	console.log('\npause and clear');
	await page.getByRole('button', { name: 'Pause', exact: true }).click();
	await emit(`mqtt-event-${id}`, { type: 'messages', messages: [msg('garden/bed1/moisture', '40')] });
	await sleep(300);
	check('paused, new topics are not added', !(await rowsText()).includes('garden'));
	check('but counted', (await page.locator('.mqtt .count').innerText()) === '7');
	await page.getByRole('button', { name: 'Resume', exact: true }).click();
	await page.getByRole('button', { name: 'Clear', exact: true }).click();
	check('Clear empties the tree', await until(async () => (await rowsText()).includes('Waiting for messages')));

	console.log('\na dropped connection');
	await emit(`mqtt-event-${id}`, { type: 'closed', error: 'The broker stopped answering' });
	check('says why, with Try again', await until(async () => (await page.locator('.connect .note.error').innerText().catch(() => '')).includes('stopped answering')));
	await page.getByRole('button', { name: 'Try again' }).click();
	check('Try again connects again', await until(async () => (await page.locator('.mqtt .bar').count()) === 1));

	console.log('\nclosing');
	const last = calls.filter((c) => c.cmd === 'mqtt_connect').at(-1)?.args.id;
	await page.locator('.tab.active .tab-close, .tab.active [aria-label*="Close"]').first().click().catch(() => {});
	check('closing the tab disconnects', await until(() => calls.some((c) => c.cmd === 'mqtt_disconnect' && c.args.connectionId === last)));
} finally {
	await browser.close();
	vite.kill();
}

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
