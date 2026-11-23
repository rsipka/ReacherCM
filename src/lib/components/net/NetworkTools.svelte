<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { sessionList, type SessionConfig } from '#lib/ipc/sessions.js';
	import { netPing, netResolve, netScanCancel, netScanPorts, netWake, type PingMethod } from '#lib/ipc/net.js';
	import { sessionAddresses, type ResolvedAddress } from '#lib/sessions/addresses.js';
	import { formatMac, formatMs, parseMac, parsePorts, pingStats, presetPorts, serviceName, type PortPreset } from '#lib/net/tools.js';
	import { hasNetworkAddress, netToolsRequest, takeNetToolsRequest, type NetTool, type NetToolsRequest } from '#lib/state/net-tools.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { isMobile } from '#lib/platform.js';

	let tool = $state<NetTool>('ping');
	let sessions = $state<SessionConfig[]>([]);
	let sessionId = $state('');
	let addressIndex = $state(0);
	let host = $state('');

	let session = $derived(sessions.find((s) => s.id === sessionId) ?? null);
	let addresses = $derived<ResolvedAddress[]>(session ? sessionAddresses(session) : []);
	let address = $derived(addresses.find((a) => a.index === addressIndex) ?? addresses[0] ?? null);

	async function loadSessions(): Promise<void> {
		try {
			sessions = (await sessionList()).filter(hasNetworkAddress).sort((a, b) => a.name.localeCompare(b.name));
		} catch (err) {
			console.error('Network tools: sessions', err);
		}
	}

	onMount(() => {
		void loadSessions();
		const reload = () => void loadSessions();
		window.addEventListener('reach:sessions-changed', reload);
		return () => {
			window.removeEventListener('reach:sessions-changed', reload);
			stopPing();
			if (scanId) void netScanCancel(scanId);
		};
	});

	/** A session picked: its main address, its port, its Wake-on-LAN. */
	function pickSession(id: string, index = 0): void {
		sessionId = id;
		const s = sessions.find((x) => x.id === id);
		if (!s) return;
		const list = sessionAddresses(s);
		const a = list.find((x) => x.index === index) ?? list[0];
		addressIndex = a.index;
		host = a.host;
		tcpPort = String(a.port || 443);
		mac = s.wol?.mac ?? '';
		broadcast = s.wol?.broadcast ?? '';
		wakePort = s.wol?.port ? String(s.wol.port) : '';
	}

	function pickAddress(index: number): void {
		addressIndex = index;
		const a = addresses.find((x) => x.index === index);
		if (a) {
			host = a.host;
			tcpPort = String(a.port || 443);
		}
	}

	// A request from a session's menu: that session, that tool, started.
	$effect(() => {
		const r = netToolsRequest();
		if (!r) return;
		untrack(() => {
			takeNetToolsRequest(r.n);
			void applyRequest(r);
		});
	});

	async function applyRequest(r: NetToolsRequest): Promise<void> {
		if (r.sessionId && !sessions.some((s) => s.id === r.sessionId)) await loadSessions();
		stopPing();
		if (r.sessionId) pickSession(r.sessionId);
		tool = r.tool;
		if (!r.run) return;
		if (r.tool === 'ping') void startPing();
		else if (r.tool === 'scan') void startScan();
		else void wake();
	}

	/** The address a tool goes to: the one typed, resolved; null after a toast. */
	async function resolveTarget(): Promise<string | null> {
		const h = host.trim();
		if (!h) {
			addToast(t('net.no_host'), 'error');
			return null;
		}
		try {
			return (await netResolve(h))[0] ?? null;
		} catch (err) {
			addToast(String(err), 'error');
			return null;
		}
	}

	// ---- Ping -------------------------------------------------------------
	type PingLine = { seq: number; ok: boolean; ms: number | null; method: string; ttl: number | null; error: string | null };
	let pingCount = $state<'4' | '10' | '0'>('4');
	let pingMethod = $state<PingMethod>('auto');
	let tcpPort = $state('443');
	let pinging = $state(false);
	let pingTarget = $state('');
	let pingLines = $state<PingLine[]>([]);
	let pingTimes = $state<(number | null)[]>([]);
	let pingRun = 0;
	let stats = $derived(pingStats(pingTimes));
	let usedTcp = $derived(pingLines.some((l) => l.method === 'tcp'));

	function stopPing(): void {
		pingRun++;
		pinging = false;
	}

	async function startPing(): Promise<void> {
		stopPing();
		const run = pingRun;
		pingLines = [];
		pingTimes = [];
		pingTarget = '';
		pinging = true;
		const ip = await resolveTarget();
		if (!ip || run !== pingRun) {
			if (run === pingRun) pinging = false;
			return;
		}
		pingTarget = ip === host.trim() ? ip : `${host.trim()} [${ip}]`;
		const count = Number(pingCount);
		const port = Number(tcpPort) || 443;
		for (let seq = 1; count === 0 || seq <= count; seq++) {
			const started = performance.now();
			let line: PingLine;
			try {
				const r = await netPing(ip, { method: pingMethod, port, timeoutMs: 2000, seq });
				line = { seq, ...r };
			} catch (err) {
				line = { seq, ok: false, ms: null, method: pingMethod, ttl: null, error: String(err) };
			}
			if (run !== pingRun) return;
			pingLines = [...pingLines.slice(-199), line];
			pingTimes = [...pingTimes, line.ok ? line.ms : null];
			if (count !== 0 && seq >= count) break;
			const wait = 1000 - (performance.now() - started);
			if (wait > 0) await new Promise((r) => setTimeout(r, wait));
			if (run !== pingRun) return;
		}
		pinging = false;
	}

	// ---- Port scan ----------------------------------------------------------
	let preset = $state<PortPreset>('common');
	let customPorts = $state('');
	let scanTimeout = $state('1000');
	let parallel = $state(isMobile() ? '32' : '128');
	let scanId = $state<string | null>(null);
	let scanDone = $state(0);
	let scanTotal = $state(0);
	let scanTarget = $state('');
	let openPorts = $state<{ port: number; ms: number }[]>([]);
	let scanSummary = $state<{ closed: number; filtered: number; ms: number; cancelled: boolean } | null>(null);
	let customError = $derived(preset === 'custom' ? parsePorts(customPorts).error : null);

	async function startScan(): Promise<void> {
		if (scanId) return;
		const ports = presetPorts(preset, customPorts);
		if (customError || ports.length === 0) {
			addToast(customError ? t('net.bad_ports', { part: customError }) : t('net.no_ports'), 'error');
			return;
		}
		const id = crypto.randomUUID();
		scanId = id;
		openPorts = [];
		scanSummary = null;
		scanDone = 0;
		scanTotal = ports.length;
		scanTarget = '';
		const ip = await resolveTarget();
		if (!ip || scanId !== id) {
			scanId = null;
			return;
		}
		scanTarget = ip === host.trim() ? ip : `${host.trim()} [${ip}]`;
		try {
			const s = await netScanPorts(id, ip, ports, { timeoutMs: Number(scanTimeout) || 1000, concurrency: Number(parallel) || undefined }, (e) => {
				if (e.type === 'open') openPorts = [...openPorts, { port: e.port, ms: e.ms }].sort((a, b) => a.port - b.port);
				else scanDone = e.done;
			});
			scanSummary = { closed: s.closed, filtered: s.filtered, ms: s.ms, cancelled: s.cancelled };
			scanDone = s.scanned;
		} catch (err) {
			addToast(String(err), 'error');
		} finally {
			scanId = null;
		}
	}

	function stopScan(): void {
		if (scanId) void netScanCancel(scanId);
	}

	// ---- Wake-on-LAN ------------------------------------------------------
	let mac = $state('');
	let broadcast = $state('');
	let wakePort = $state('');
	let waking = $state(false);
	let woke = $state(false);
	let macError = $derived(mac.trim() !== '' && !parseMac(mac));

	async function wake(): Promise<void> {
		if (!parseMac(mac)) {
			addToast(t('net.bad_mac'), 'error');
			return;
		}
		waking = true;
		woke = false;
		try {
			await netWake(mac, broadcast, Number(wakePort) || null);
			woke = true;
			addToast(t('net.wake_sent', { mac: formatMac(mac) }), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		} finally {
			waking = false;
		}
	}

	/** After a wake: ping until it answers. */
	function pingAfterWake(): void {
		tool = 'ping';
		pingCount = '0';
		void startPing();
	}

	const TOOLS: { id: NetTool; label: string }[] = [
		{ id: 'ping', label: 'net.ping' },
		{ id: 'scan', label: 'net.scan' },
		{ id: 'wake', label: 'net.wake' },
	];
</script>

<div class="net-tools">
	<div class="segmented" role="tablist">
		{#each TOOLS as x (x.id)}
			<button class="seg" class:selected={tool === x.id} role="tab" aria-selected={tool === x.id} type="button" onclick={() => (tool = x.id)}>{t(x.label)}</button>
		{/each}
	</div>

	<div class="target">
		<label class="field">
			<span>{t('net.session')}</span>
			<select value={sessionId} onchange={(e) => (e.currentTarget.value ? pickSession(e.currentTarget.value) : (sessionId = ''))}>
				<option value="">{t('net.no_session')}</option>
				{#each sessions as s (s.id)}
					<option value={s.id}>{s.name}</option>
				{/each}
			</select>
		</label>
		{#if addresses.length > 1}
			<label class="field">
				<span>{t('net.address')}</span>
				<select value={String(addressIndex)} onchange={(e) => pickAddress(Number(e.currentTarget.value))}>
					{#each addresses as a (a.index)}
						<option value={String(a.index)}>{a.label ? `${a.label}: ${a.host}` : a.host}</option>
					{/each}
				</select>
			</label>
			{#if address?.routed}
				<p class="hint">{t('net.routed_hint')}</p>
			{/if}
		{/if}
		{#if tool !== 'wake'}
			<label class="field">
				<span>{t('net.host')}</span>
				<input type="text" bind:value={host} placeholder="10.0.0.5" autocapitalize="off" autocomplete="off" spellcheck="false"
					onkeydown={(e) => { if (e.key === 'Enter') void (tool === 'ping' ? startPing() : startScan()); }} />
			</label>
		{/if}
	</div>

	{#if tool === 'ping'}
		<div class="row">
			<label class="field grow">
				<span>{t('net.count')}</span>
				<select bind:value={pingCount}>
					<option value="4">4</option>
					<option value="10">10</option>
					<option value="0">{t('net.until_stopped')}</option>
				</select>
			</label>
			<label class="field grow">
				<span>{t('net.method')}</span>
				<select bind:value={pingMethod} title={t('net.method_hint')}>
					<option value="auto">{t('net.method_auto')}</option>
					<option value="icmp">ICMP</option>
					<option value="tcp">TCP</option>
				</select>
			</label>
			{#if pingMethod !== 'icmp'}
				<label class="field port">
					<span>{t('net.tcp_port')}</span>
					<input type="number" min="1" max="65535" bind:value={tcpPort} />
				</label>
			{/if}
		</div>
		<div class="actions">
			{#if pinging}
				<button class="btn" type="button" onclick={stopPing}>{t('net.stop')}</button>
			{:else}
				<button class="btn primary" type="button" onclick={() => void startPing()}>{t('net.start_ping')}</button>
			{/if}
		</div>
		{#if pingTarget || pingLines.length}
			<div class="output" aria-live="polite">
				{#if pingTarget}<div class="out-head">{t('net.pinging', { target: pingTarget })}</div>{/if}
				{#each pingLines as l (l.seq)}
					<div class="out-line" class:bad={!l.ok}>
						<span class="seq">{l.seq}</span>
						{#if l.ok}
							<span>{formatMs(l.ms)}</span>
							<span class="dim">{l.method.toUpperCase()}{l.ttl != null ? ` · TTL ${l.ttl}` : ''}</span>
						{:else}
							<span>{l.error ?? t('net.timeout')}</span>
						{/if}
					</div>
				{/each}
				{#if stats.sent}
					<div class="out-stats">
						{t('net.ping_stats', { sent: String(stats.sent), received: String(stats.received), loss: String(stats.loss) })}
						{#if stats.received}<br />{t('net.ping_times', { min: formatMs(stats.min), avg: formatMs(stats.avg), max: formatMs(stats.max) })}{/if}
					</div>
				{/if}
				{#if usedTcp && pingMethod === 'auto'}<p class="hint">{t('net.tcp_fallback')}</p>{/if}
			</div>
		{/if}
	{:else if tool === 'scan'}
		<label class="field">
			<span>{t('net.ports')}</span>
			<select bind:value={preset}>
				<option value="common">{t('net.ports_common')}</option>
				<option value="well-known">{t('net.ports_well_known')}</option>
				<option value="all">{t('net.ports_all')}</option>
				<option value="custom">{t('net.ports_custom')}</option>
			</select>
		</label>
		{#if preset === 'custom'}
			<label class="field">
				<span>{t('net.ports_list')}</span>
				<input type="text" bind:value={customPorts} placeholder="22, 80, 443, 8000-8100" class:invalid={!!customError} spellcheck="false" />
			</label>
		{/if}
		<div class="row">
			<label class="field grow">
				<span>{t('net.timeout_ms')}</span>
				<input type="number" min="50" max="10000" step="50" bind:value={scanTimeout} />
			</label>
			<label class="field grow">
				<span>{t('net.parallel')}</span>
				<input type="number" min="1" max="512" bind:value={parallel} />
			</label>
		</div>
		<div class="actions">
			{#if scanId}
				<button class="btn" type="button" onclick={stopScan}>{t('net.stop')}</button>
			{:else}
				<button class="btn primary" type="button" onclick={() => void startScan()}>{t('net.start_scan')}</button>
			{/if}
		</div>
		{#if scanTarget || scanSummary}
			<div class="output">
				{#if scanTarget}<div class="out-head">{t('net.scanning', { target: scanTarget })}</div>{/if}
				{#if scanId || scanDone < scanTotal}
					<div class="progress" role="progressbar" aria-valuemin={0} aria-valuemax={scanTotal} aria-valuenow={scanDone}>
						<div class="bar" style:width="{scanTotal ? (scanDone / scanTotal) * 100 : 0}%"></div>
					</div>
					<div class="dim small">{scanDone} / {scanTotal}</div>
				{/if}
				{#each openPorts as p (p.port)}
					<div class="out-line">
						<span class="port-no">{p.port}</span>
						<span>{serviceName(p.port)}</span>
						<span class="dim">{formatMs(p.ms)}</span>
					</div>
				{/each}
				{#if scanSummary}
					<div class="out-stats">
						{#if scanSummary.cancelled}{t('net.scan_stopped')}<br />{/if}
						{t('net.scan_summary', { open: String(openPorts.length), closed: String(scanSummary.closed), filtered: String(scanSummary.filtered), seconds: (scanSummary.ms / 1000).toFixed(1) })}
					</div>
				{/if}
			</div>
		{/if}
	{:else}
		<label class="field">
			<span>{t('net.mac')}</span>
			<input type="text" bind:value={mac} placeholder="00:11:22:AA:BB:CC" class:invalid={macError} autocapitalize="off" spellcheck="false" />
		</label>
		<div class="row">
			<label class="field grow">
				<span>{t('net.broadcast')}</span>
				<input type="text" bind:value={broadcast} placeholder="255.255.255.255" autocapitalize="off" spellcheck="false" />
			</label>
			<label class="field port">
				<span>{t('net.udp_port')}</span>
				<input type="number" min="1" max="65535" bind:value={wakePort} placeholder="9" />
			</label>
		</div>
		<p class="hint">{t('net.broadcast_hint')}</p>
		<div class="actions">
			<button class="btn primary" type="button" disabled={waking || !parseMac(mac)} onclick={() => void wake()}>{t('net.send_wake')}</button>
			{#if woke && host.trim()}
				<button class="btn" type="button" onclick={pingAfterWake}>{t('net.ping_until_up')}</button>
			{/if}
		</div>
	{/if}
</div>

<style>
	.net-tools {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 4px 0;
		min-width: 0;
	}

	.segmented {
		display: flex;
		gap: 2px;
		padding: 2px;
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
	}

	.seg {
		flex: 1;
		min-width: 0;
		padding: 5px 6px;
		line-height: 1.2;
		font-family: var(--font-sans);
		font-size: 0.75rem;
		font-weight: 500;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.seg:hover {
		color: var(--color-text-primary);
	}

	.seg.selected {
		background-color: var(--color-bg-elevated);
		color: var(--color-accent);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25), inset 0 0 0 1px var(--color-border);
	}

	.target {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.row {
		display: flex;
		gap: 6px;
		min-width: 0;
	}

	.field {
		display: grid;
		gap: 4px;
		min-width: 0;
		color: var(--color-text-secondary);
		font-size: 0.75rem;
	}

	.field.grow {
		flex: 1;
	}

	.field.port {
		width: 76px;
		flex: none;
	}

	.field select,
	.field input {
		min-width: 0;
		width: 100%;
		box-sizing: border-box;
		padding: 6px 8px;
		font: inherit;
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
	}

	.field input:focus,
	.field select:focus {
		outline: none;
		border-color: var(--color-accent);
	}

	.field input.invalid {
		border-color: var(--color-danger);
	}

	.hint {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
		opacity: 0.85;
	}

	.actions {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
	}

	.btn {
		flex: 1;
		padding: 7px 10px;
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		font-weight: 500;
		border-radius: var(--radius-btn);
		cursor: pointer;
		color: var(--color-text-primary);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
	}

	.btn.primary {
		color: var(--color-accent);
		background: transparent;
		border-color: var(--color-accent);
	}

	.btn:hover:not(:disabled) {
		background-color: var(--color-surface-hover);
	}

	.btn:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.output {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 8px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card, 8px);
		background: var(--color-bg-primary);
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--color-text-primary);
		max-height: 50vh;
		overflow-y: auto;
		user-select: text;
	}

	.out-head {
		color: var(--color-text-secondary);
		margin-bottom: 4px;
		word-break: break-all;
	}

	.out-line {
		display: flex;
		gap: 8px;
		align-items: baseline;
	}

	.out-line.bad {
		color: var(--color-danger);
	}

	.seq,
	.port-no {
		min-width: 3.5em;
		color: var(--color-text-secondary);
		text-align: right;
	}

	.port-no {
		color: var(--color-success, var(--color-accent));
		font-weight: 600;
	}

	.dim {
		color: var(--color-text-secondary);
	}

	.small {
		font-size: 0.6875rem;
	}

	.out-stats {
		margin-top: 6px;
		padding-top: 6px;
		border-top: 1px solid var(--color-border);
		color: var(--color-text-secondary);
	}

	.progress {
		height: 4px;
		border-radius: 2px;
		background: var(--color-border);
		overflow: hidden;
		margin: 4px 0 2px;
	}

	.bar {
		height: 100%;
		background: var(--color-accent);
		transition: width 0.2s ease;
	}
</style>
