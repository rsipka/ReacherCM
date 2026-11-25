<script lang="ts">
	/**
	 * A server management tab: logs in to a server's BMC (iDRAC, iLO...)
	 * over Redfish, asking for a password or about a certificate when it
	 * must, then shows its pages. The login lives in the backend; closing
	 * the tab logs out (see closeTab), which frees the BMC's session slot.
	 */
	import { onMount, tick, untrack } from 'svelte';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { setBmcConnection, setTabLink, type Tab } from '#lib/state/tabs.svelte.js';
	import {
		bmcConnect,
		bmcDisconnect,
		bmcTrustCert,
		bmcOverview,
		bmcSensors,
		bmcHardware,
		bmcRemoteConsole,
		BMC_PASSWORD_NEEDED,
		BMC_LOGIN_REFUSED,
		BMC_UNTRUSTED_CERT,
		type BmcOpened,
		type BmcOverview,
		type BmcSensors,
		type BmcHardware,
		type BmcItem,
		type BmcSensor
	} from '#lib/ipc/bmc.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import { isMobile } from '#lib/platform.js';
	import { tooltip } from '#lib/utils/tooltip.js';
	import { requestSessionAction } from '#lib/state/palette.svelte.js';
	import { BMC_SSH_PORT, rememberedConsole, rememberConsole } from '#lib/sessions/bmc-console.js';
	import BmcLogs from './BmcLogs.svelte';
	import BmcTasks from './BmcTasks.svelte';
	import { healthClass, healthLabel, word, when, num, bmcError, vendorName } from './bmc-format.js';
	import { sessionGet, sessionUpdate } from '#lib/ipc/sessions.js';
	import { bmcVendorOs } from '#lib/data/distro-icons.js';

	interface Props {
		tab: Tab;
		active: boolean;
	}

	let { tab, active }: Props = $props();

	type Stage =
		| { kind: 'connecting' }
		| { kind: 'password'; error?: string }
		| { kind: 'cert'; fingerprint: string }
		| { kind: 'error'; message: string }
		| { kind: 'up' };

	type Page = 'overview' | 'sensors' | 'hardware' | 'logs' | 'tasks';
	const PAGES: Page[] = ['overview', 'sensors', 'hardware', 'logs', 'tasks'];
	/** Sensors read again this often while their page is shown. */
	const SENSOR_REFRESH_MS = 30_000;

	let stage = $state<Stage>({ kind: 'connecting' });
	let password = $state('');
	let passwordEl: HTMLInputElement | undefined = $state();
	let seenReconnect = untrack(() => tab.reconnect);
	let opened = $state<BmcOpened | null>(null);
	let page = $state<Page>('overview');

	let overview = $state<BmcOverview | null>(null);
	let sensors = $state<BmcSensors | null>(null);
	let hardware = $state<BmcHardware | null>(null);
	let loading = $state<Page | null>(null);
	let pageError = $state<string | null>(null);
	/** Bumped to make the logs and tasks pages read again. */
	let refreshCount = $state(0);

	let params = $derived(tab.bmcConnectParams);
	let where = $derived(params ? `${params.username ? params.username + '@' : ''}${params.host}${params.port && params.port !== 443 ? ':' + params.port : ''}` : '');
	let serverName = $derived(tab.sessionName ?? tab.title);
	let connectionId = $derived(tab.bmcConnectionId);

	/** The session list shows the maker's logo (Dell, Lenovo) or the server
	 *  icon: saved like an SSH session's detected OS, unless one was picked
	 *  by hand. */
	async function saveVendorIcon(sessionId: string, vendor: string | null | undefined): Promise<void> {
		try {
			const stored = await sessionGet(sessionId);
			const os = bmcVendorOs(vendor);
			if (stored.os_manual || stored.detected_os === os) return;
			await sessionUpdate({ ...stored, detected_os: os });
			window.dispatchEvent(new Event('reach:sessions-changed'));
		} catch {
			// Only the icon: it shows next time.
		}
	}

	async function connect(typed?: string): Promise<void> {
		const p = params;
		if (!p) return;
		stage = { kind: 'connecting' };
		setTabLink(tab.id, 'reconnecting');
		const old = tab.bmcConnectionId;
		if (old) {
			setBmcConnection(tab.id, undefined);
			await bmcDisconnect(old).catch(() => {});
		}
		const id = crypto.randomUUID();
		try {
			opened = await bmcConnect(id, p.sessionId, typed);
			void saveVendorIcon(p.sessionId, opened.vendor);
			setBmcConnection(tab.id, id);
			setTabLink(tab.id, undefined);
			password = '';
			stage = { kind: 'up' };
			overview = sensors = hardware = null;
			await load(page, true);
		} catch (err) {
			setTabLink(tab.id, 'down');
			const msg = String(err);
			if (msg === BMC_PASSWORD_NEEDED) {
				stage = { kind: 'password' };
				await tick();
				passwordEl?.focus();
			} else if (msg.startsWith(BMC_LOGIN_REFUSED)) {
				const detail = msg.slice(BMC_LOGIN_REFUSED.length);
				stage = { kind: 'password', error: detail ? `${t('bmc.login_refused')} ${detail}` : t('bmc.login_refused') };
				await tick();
				passwordEl?.focus();
			} else if (msg.startsWith(BMC_UNTRUSTED_CERT)) {
				stage = { kind: 'cert', fingerprint: msg.slice(BMC_UNTRUSTED_CERT.length) };
			} else {
				stage = { kind: 'error', message: msg };
			}
		}
	}

	async function trust(fingerprint: string): Promise<void> {
		const p = params;
		if (!p) return;
		try {
			await bmcTrustCert(p.sessionId, fingerprint);
			await connect();
		} catch (err) {
			stage = { kind: 'error', message: String(err) };
		}
	}

	/** Read a page; `force` reads it again even when it was read before. */
	async function load(which: Page, force = false): Promise<void> {
		const id = tab.bmcConnectionId;
		if (!id) return;
		pageError = null;
		if (which === 'logs' || which === 'tasks') {
			if (force) refreshCount++;
			return;
		}
		if (!force && ((which === 'overview' && overview) || (which === 'sensors' && sensors) || (which === 'hardware' && hardware))) return;
		loading = which;
		try {
			if (which === 'overview') overview = await bmcOverview(id);
			else if (which === 'sensors') sensors = await bmcSensors(id);
			else if (which === 'hardware') hardware = await bmcHardware(id);
		} catch (err) {
			pageError = bmcError(err);
		} finally {
			if (loading === which) loading = null;
		}
	}

	function show(p: Page): void {
		page = p;
		void load(p);
	}

	function openWeb(): void {
		const p = params;
		if (!p) return;
		const host = p.host.includes(':') && !p.host.startsWith('[') ? `[${p.host}]` : p.host;
		void openUrl(`https://${host}${p.port && p.port !== 443 ? ':' + p.port : ''}/`).catch((e) => addToast(String(e), 'error'));
	}

	/** The controller's own console, in a web window (desktop). */
	let consoleOpening = $state(false);
	async function remoteConsole(): Promise<void> {
		const p = params;
		const id = tab.bmcConnectionId;
		if (!p || !id || consoleOpening) return;
		consoleOpening = true;
		try {
			await bmcRemoteConsole(id, p.sessionId);
		} catch (err) {
			addToast(bmcError(err), 'error');
		} finally {
			consoleOpening = false;
		}
	}

	/** Text console: SSH to the controller, then its serial console command. */
	let textConsole = $state<{ port: string; command: string } | null>(null);
	function askTextConsole(): void {
		const p = params;
		if (!p) return;
		const last = rememberedConsole(p.sessionId);
		textConsole = {
			port: String(last?.port ?? (opened?.vendor === 'openbmc' ? 2200 : BMC_SSH_PORT)),
			command: last?.command ?? opened?.consoleCommand ?? ''
		};
	}
	function openTextConsole(): void {
		const p = params;
		const c = textConsole;
		if (!p || !c) return;
		const port = Number.parseInt(c.port, 10);
		if (!(port > 0 && port < 65536)) {
			addToast(t('bmc.console_bad_port'), 'error');
			return;
		}
		const command = c.command.trim();
		rememberConsole(p.sessionId, port, command);
		textConsole = null;
		requestSessionAction({ kind: 'bmc-console', sessionId: p.sessionId, port, command });
	}

	/** A plain SSH shell to the controller (racadm, SMASH CLP), as the same
	 *  user, on the text console's port: no command typed. */
	function openSsh(): void {
		const p = params;
		if (!p) return;
		const port = rememberedConsole(p.sessionId)?.port ?? (opened?.vendor === 'openbmc' ? 2200 : BMC_SSH_PORT);
		requestSessionAction({ kind: 'bmc-console', sessionId: p.sessionId, port, command: '' });
	}

	onMount(() => {
		if (tab.bmcConnectionId) {
			stage = { kind: 'up' };
			void load('overview');
		} else void connect();
	});

	// The tab menu's Reconnect.
	$effect(() => {
		const bump = tab.reconnect;
		if (bump === seenReconnect) return;
		seenReconnect = bump;
		void connect();
	});

	// Sensors keep moving: read them again while they are in view.
	$effect(() => {
		if (!active || page !== 'sensors' || stage.kind !== 'up') return;
		const timer = setInterval(() => {
			if (loading !== 'sensors') void load('sensors', true);
		}, SENSOR_REFRESH_MS);
		return () => clearInterval(timer);
	});

	/** After a task changed something (power, boot once): the overview again. */
	function changed(): void {
		void load('overview', true);
	}

	/** The controller was restarted: its login is gone. */
	function bmcRestarted(): void {
		setBmcConnection(tab.id, undefined);
		setTabLink(tab.id, 'down');
		stage = { kind: 'error', message: t('bmc.reset_done') };
	}

	let headline = $derived(overview?.healthRollup ?? overview?.health ?? null);
</script>

{#snippet dot(h: string | null | undefined)}
	<span class="dot {healthClass(h)}" title={healthLabel(h)}></span>
{/snippet}

{#snippet items(title: string, list: BmcItem[])}
	{#if list.length > 0}
		<section class="block">
			<h3>{title} <span class="count">{list.length}</span></h3>
			<ul class="items">
				{#each list as item, i (i)}
					<li>
						{@render dot(item.health)}
						<span class="item-name">{item.name}</span>
						<span class="item-detail">{item.detail}</span>
					</li>
				{/each}
			</ul>
		</section>
	{/if}
{/snippet}

{#snippet sensorTable(title: string, list: BmcSensor[])}
	{#if list.length > 0}
		<section class="block">
			<h3>{title} <span class="count">{list.length}</span></h3>
			<div class="table-box"><table class="rf-table">
				<thead>
					<tr><th></th><th>{t('bmc.col_name')}</th><th class="r">{t('bmc.col_reading')}</th><th class="r">{t('bmc.col_limits')}</th></tr>
				</thead>
				<tbody>
					{#each list as s, i (i)}
						<tr>
							<td class="dot-cell">{@render dot(s.health)}</td>
							<td>{s.name}</td>
							<td class="r mono">{s.reading === null ? '—' : `${num(s.reading, s.units === 'V' ? 2 : 0)} ${s.units}`}</td>
							<td class="r mono dim">{s.warning === null && s.critical === null ? '' : `${num(s.warning, s.units === 'V' ? 2 : 0)} / ${num(s.critical, s.units === 'V' ? 2 : 0)}`}</td>
						</tr>
					{/each}
				</tbody>
			</table></div>
		</section>
	{/if}
{/snippet}

{#if stage.kind === 'up' && connectionId}
	<div class="bmc">
		<header class="top">
			<div class="who">
				{@render dot(headline)}
				<div class="who-text">
					<span class="name">{serverName}</span>
					<span class="sub">{[opened?.product ?? vendorName(opened?.vendor), overview?.model, where].filter(Boolean).join(' · ')}</span>
				</div>
			</div>
			<div class="top-actions">
				{#if overview?.powerState}
					<span class="power {overview.powerState === 'On' ? 'on' : 'off'}">{word('bmc.power_state_', overview.powerState)}</span>
				{/if}
				{#if !isMobile()}
					<span class="tip-wrap" use:tooltip={t('bmc.remote_console_tip')}>
						<Button variant="secondary" size="sm" disabled={consoleOpening} onclick={() => void remoteConsole()}>{t('bmc.remote_console')}</Button>
					</span>
				{/if}
				<Button variant="secondary" size="sm" onclick={askTextConsole}>{t('bmc.text_console')}</Button>
				<span class="tip-wrap" use:tooltip={t('bmc.ssh_tip')}>
					<Button variant="secondary" size="sm" onclick={openSsh}>{t('bmc.ssh')}</Button>
				</span>
				<Button variant="ghost" size="sm" onclick={openWeb}>{t('bmc.open_web')}</Button>
				<Button variant="secondary" size="sm" disabled={loading !== null} onclick={() => void load(page, true)}>{t('bmc.refresh')}</Button>
			</div>
		</header>
		<div class="pages" role="tablist">
			{#each PAGES as p (p)}
				<button type="button" role="tab" class="page-tab" class:active={page === p} aria-selected={page === p} onclick={() => show(p)}>{t(`bmc.page_${p}`)}</button>
			{/each}
		</div>
		<div class="body">
			{#if pageError}
				<p class="note error">{pageError}</p>
			{/if}
			{#if loading === page && !(page === 'overview' ? overview : page === 'sensors' ? sensors : hardware)}
				<p class="note">{t('bmc.loading')}</p>
			{/if}

			{#if page === 'overview' && overview}
				<dl class="facts">
					<dt>{t('bmc.health')}</dt>
					<dd>{@render dot(headline)} {healthLabel(headline)}</dd>
					<dt>{t('bmc.power')}</dt>
					<dd>{word('bmc.power_state_', overview.powerState)}{overview.powerWatts !== null ? ` · ${num(overview.powerWatts)} W` : ''}</dd>
					<dt>{t('bmc.model')}</dt>
					<dd>{[overview.manufacturer, overview.model].filter(Boolean).join(' ') || '—'}</dd>
					<dt>{t('bmc.serial')}</dt>
					<dd class="mono">{[overview.sku, overview.serial].filter(Boolean).join(' · ') || '—'}</dd>
					{#if overview.hostName}
						<dt>{t('bmc.host_name')}</dt>
						<dd>{overview.hostName}</dd>
					{/if}
					<dt>{t('bmc.cpus')}</dt>
					<dd>{overview.cpuCount !== null ? `${overview.cpuCount} × ` : ''}{overview.cpuModel ?? '—'}</dd>
					<dt>{t('bmc.memory')}</dt>
					<dd>{overview.memoryGib !== null ? `${num(overview.memoryGib)} GiB` : '—'}</dd>
					{#if overview.inletTemp !== null}
						<dt>{t('bmc.inlet')}</dt>
						<dd>{num(overview.inletTemp)} °C</dd>
					{/if}
					<dt>{t('bmc.bios')}</dt>
					<dd class="mono">{overview.biosVersion ?? '—'}</dd>
					<dt>{t('bmc.controller')}</dt>
					<dd>{[overview.bmcModel, overview.bmcFirmware].filter(Boolean).join(' · ') || '—'}</dd>
					{#if overview.bootEnabled && overview.bootEnabled !== 'Disabled' && overview.bootTarget && overview.bootTarget !== 'None'}
						<dt>{t('bmc.next_boot')}</dt>
						<dd>{word('bmc.boot_', overview.bootTarget)}</dd>
					{/if}
					<dt>{t('bmc.bmc_time')}</dt>
					<dd>{when(overview.bmcTime)}</dd>
					<dt>{t('bmc.redfish')}</dt>
					<dd class="dim">{overview.redfishVersion ?? '—'}</dd>
				</dl>
			{:else if page === 'sensors' && sensors}
				{#if sensors.powerWatts !== null}
					<p class="note">{t('bmc.power_draw', { watts: num(sensors.powerWatts) })}</p>
				{/if}
				{@render sensorTable(t('bmc.temperatures'), sensors.temperatures)}
				{@render sensorTable(t('bmc.fans'), sensors.fans)}
				{@render items(t('bmc.power_supplies'), sensors.powerSupplies)}
				{@render sensorTable(t('bmc.voltages'), sensors.voltages)}
				{#if !sensors.temperatures.length && !sensors.fans.length && !sensors.powerSupplies.length && !sensors.voltages.length}
					<p class="note">{t('bmc.nothing')}</p>
				{/if}
			{:else if page === 'hardware' && hardware}
				{@render items(t('bmc.processors'), hardware.processors)}
				{@render items(t('bmc.memory_modules'), hardware.memory)}
				{@render items(t('bmc.storage'), hardware.storage)}
				{@render items(t('bmc.drives'), hardware.drives)}
				{@render items(t('bmc.volumes'), hardware.volumes)}
				{@render items(t('bmc.network'), hardware.network)}
				{@render items(t('bmc.firmware'), hardware.firmware)}
			{:else if page === 'logs'}
				<BmcLogs {connectionId} server={serverName} refresh={refreshCount} />
			{:else if page === 'tasks'}
				<BmcTasks {connectionId} server={serverName} refresh={refreshCount} onchanged={changed} onbmcrestart={bmcRestarted} />
			{/if}
		</div>
	</div>
{:else}
	<div class="connect">
		<div class="card">
			<div class="title">{serverName}</div>
			<div class="where">{where}</div>
			{#if stage.kind === 'connecting'}
				<p class="note">{t('bmc.connecting')}</p>
			{:else if stage.kind === 'password'}
				<form
					onsubmit={(e) => {
						e.preventDefault();
						if (password) void connect(password);
					}}
				>
					<label class="field">
						<span>{t('session.password')}</span>
						<input bind:this={passwordEl} type="password" bind:value={password} autocomplete="off" />
					</label>
					{#if stage.error}<p class="note error">{stage.error}</p>{/if}
					<div class="actions">
						<Button type="submit" disabled={!password}>{t('bmc.connect')}</Button>
					</div>
				</form>
			{:else if stage.kind === 'cert'}
				<p class="note">{t('bmc.cert_untrusted')}</p>
				<code class="fp">{stage.fingerprint}</code>
				<p class="note small">{t('bmc.cert_hint')}</p>
				<div class="actions">
					<Button variant="secondary" onclick={() => (stage = { kind: 'error', message: t('bmc.cert_refused') })}>{t('common.cancel')}</Button>
					<Button onclick={() => void trust((stage as { fingerprint: string }).fingerprint)}>{t('bmc.cert_trust')}</Button>
				</div>
			{:else if stage.kind === 'error'}
				<p class="note error">{stage.message}</p>
				<div class="actions">
					<Button variant="ghost" onclick={openWeb}>{t('bmc.open_web')}</Button>
					<Button onclick={() => void connect()}>{t('bmc.retry')}</Button>
				</div>
			{/if}
		</div>
	</div>
{/if}

<Modal open={textConsole !== null} onclose={() => (textConsole = null)} title={t('bmc.text_console')} maxWidth="440px">
	{#if textConsole}
		<form class="console-form" onsubmit={(e) => { e.preventDefault(); openTextConsole(); }}>
			<p class="note">{t('bmc.text_console_intro')}</p>
			<label>
				<span>{t('bmc.console_port')}</span>
				<input type="text" inputmode="numeric" bind:value={textConsole.port} />
			</label>
			<label>
				<span>{t('bmc.console_command')}</span>
				<input type="text" class="mono" bind:value={textConsole.command} placeholder={t('bmc.console_command_none')} autocomplete="off" spellcheck="false" />
			</label>
			<p class="note dim">{t('bmc.console_command_hint')}</p>
			<div class="console-actions">
				<Button variant="ghost" size="sm" type="button" onclick={() => (textConsole = null)}>{t('common.cancel')}</Button>
				<Button variant="primary" size="sm" type="submit">{t('bmc.console_open')}</Button>
			</div>
		</form>
	{/if}
</Modal>

<style>
	.tip-wrap {
		display: inline-flex;
	}

	.bmc {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		min-width: 0;
	}
	.top {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 12px 16px 8px;
		flex-wrap: wrap;
	}
	.who {
		display: flex;
		align-items: center;
		gap: 10px;
		min-width: 0;
	}
	.who-text {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.name {
		font-weight: 600;
		font-size: 15px;
	}
	.sub {
		font-size: 12px;
		color: var(--color-text-tertiary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.console-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.console-form label {
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.console-form input {
		padding: 6px 8px;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.console-actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}
	.top-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.power {
		font-size: 12px;
		padding: 2px 8px;
		border-radius: 999px;
		border: 1px solid var(--color-border);
		color: var(--color-text-secondary);
	}
	.power.on {
		color: var(--color-success, #2e9d5b);
		border-color: currentColor;
	}
	.pages {
		display: flex;
		gap: 2px;
		padding: 0 12px;
		border-bottom: 1px solid var(--color-border);
		overflow-x: auto;
		flex-shrink: 0;
	}
	.page-tab {
		background: none;
		border: none;
		border-bottom: 2px solid transparent;
		padding: 8px 12px;
		font-size: 13px;
		color: var(--color-text-secondary);
		cursor: pointer;
		white-space: nowrap;
	}
	.page-tab.active {
		color: var(--color-text-primary);
		border-bottom-color: var(--color-accent);
	}
	.body {
		flex: 1;
		overflow: auto;
		padding: 14px 16px 24px;
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 8px 18px;
		margin: 0;
		font-size: 13px;
		max-width: 760px;
	}
	.facts dt {
		color: var(--color-text-tertiary);
	}
	.facts dd {
		margin: 0;
		display: flex;
		align-items: center;
		gap: 6px;
		word-break: break-word;
	}
	.block h3 {
		margin: 0 0 6px;
		font-size: 13px;
		font-weight: 600;
		color: var(--color-text-secondary);
	}
	.count {
		font-weight: 400;
		color: var(--color-text-tertiary);
		margin-left: 4px;
	}
	.items {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		border: 1px solid var(--color-border);
		border-radius: 8px;
		overflow: hidden;
	}
	.items li {
		display: grid;
		grid-template-columns: 14px minmax(120px, 0.35fr) 1fr;
		gap: 10px;
		align-items: center;
		padding: 6px 10px;
		font-size: 13px;
		border-top: 1px solid var(--color-border);
	}
	.items li:first-child {
		border-top: none;
	}
	.item-name {
		overflow-wrap: anywhere;
	}
	.item-detail {
		color: var(--color-text-secondary);
		overflow-wrap: anywhere;
	}
	.table-box {
		border: 1px solid var(--color-border);
		border-radius: 8px;
		overflow-x: auto;
	}
	.rf-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 13px;
	}
	.rf-table td:nth-child(2) {
		width: 100%;
	}
	.rf-table th {
		text-align: left;
		font-weight: 500;
		font-size: 12px;
		color: var(--color-text-tertiary);
		padding: 6px 10px;
		border-bottom: 1px solid var(--color-border);
		white-space: nowrap;
	}
	.rf-table td {
		padding: 5px 10px;
		border-top: 1px solid var(--color-border);
		white-space: nowrap;
	}
	.rf-table td:nth-child(2) {
		white-space: normal;
	}
	.r {
		text-align: right !important;
	}
	.mono {
		font-family: var(--font-mono, monospace);
		font-size: 12px;
	}
	.dim {
		color: var(--color-text-tertiary);
	}
	.dot-cell {
		width: 14px;
	}
	.dot {
		display: inline-block;
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: var(--color-text-tertiary);
		flex-shrink: 0;
	}
	.dot.ok {
		background: var(--color-success, #2e9d5b);
	}
	.dot.warn {
		background: var(--color-warning, #d99a1e);
	}
	.dot.crit {
		background: var(--color-danger, #d64545);
	}
	.dot.none {
		background: transparent;
		border: 1px solid var(--color-text-tertiary);
	}
	.connect {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-bg-primary);
		padding: 16px;
	}
	.card {
		width: min(460px, 100%);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: 10px;
		padding: 20px;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.title {
		font-weight: 600;
		color: var(--color-text-primary);
	}
	.where {
		font-size: 12px;
		color: var(--color-text-tertiary);
		word-break: break-all;
	}
	.note {
		margin: 0;
		font-size: 13px;
		color: var(--color-text-secondary);
	}
	.note.small {
		font-size: 12px;
	}
	.note.error {
		color: var(--color-danger);
		white-space: pre-wrap;
	}
	.fp {
		font-size: 11px;
		word-break: break-all;
		padding: 8px;
		border-radius: 6px;
		background: var(--color-surface-sunken);
		color: var(--color-text-primary);
	}
	form {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.field input {
		padding: 7px 9px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}
	@media (max-width: 600px) {
		.items li {
			grid-template-columns: 14px 1fr;
		}
		.item-detail {
			grid-column: 2;
		}
	}
</style>
