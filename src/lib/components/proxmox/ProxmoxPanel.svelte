<script lang="ts">
	/**
	 * A Proxmox tab: logs in to a Proxmox VE node or cluster with the
	 * session's API token or user login (asking for a password, a TOTP code
	 * or about a certificate when it must), then shows its pages. The login
	 * lives in the backend; closing the tab drops it (see closeTab).
	 *
	 * Consoles open in tabs of their own (see #lib/sessions/proxmox-console).
	 * Proxmox opens them for a user login only, so a session that logged in
	 * with a token is asked for its user's password (and code) the first time.
	 */
	import { onMount, tick, untrack } from 'svelte';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { setProxmoxConnection, setTabLink, type Tab } from '#lib/state/tabs.svelte.js';
	import {
		proxmoxConnect,
		proxmoxDisconnect,
		proxmoxTrustCert,
		proxmoxOverview,
		proxmoxResources,
		proxmoxConsoleReady,
		proxmoxConsoleLogin,
		proxmoxNodeAction,
		PVE_PASSWORD_NEEDED,
		PVE_TFA_NEEDED,
		PVE_LOGIN_REFUSED,
		PVE_UNTRUSTED_CERT,
		type ProxmoxOpened,
		type ProxmoxOverview,
		type ProxmoxResource,
		type ProxmoxConsole
	} from '#lib/ipc/proxmox.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import { isMobile } from '#lib/platform.js';
	import { openProxmoxConsole, consoleTitle, type ConsoleKind } from '#lib/sessions/proxmox-console.js';
	import ProxmoxGuest from './ProxmoxGuest.svelte';
	import ProxmoxTasks from './ProxmoxTasks.svelte';
	import { bytes, share, cpu, uptime, stateClass, stateLabel, proxmoxError } from './proxmox-format.js';

	interface Props {
		tab: Tab;
		active: boolean;
	}

	let { tab, active }: Props = $props();

	type Stage =
		| { kind: 'connecting' }
		| { kind: 'password'; error?: string }
		| { kind: 'tfa'; error?: string }
		| { kind: 'cert'; fingerprint: string }
		| { kind: 'error'; message: string }
		| { kind: 'up' };

	type Page = 'overview' | 'guests' | 'storage' | 'tasks';
	const PAGES: Page[] = ['overview', 'guests', 'storage', 'tasks'];
	/** The pages read again this often while they are shown. */
	const REFRESH_MS = 10_000;

	let stage = $state<Stage>({ kind: 'connecting' });
	let password = $state('');
	let otp = $state('');
	/** The password typed for this login, kept to send with the code. */
	let typedPassword = '';
	let inputEl: HTMLInputElement | undefined = $state();
	let seenReconnect = untrack(() => tab.reconnect);
	let opened = $state<ProxmoxOpened | null>(null);
	let page = $state<Page>('guests');

	let overview = $state<ProxmoxOverview | null>(null);
	let resources = $state<ProxmoxResource[] | null>(null);
	let loading = $state(false);
	let pageError = $state<string | null>(null);
	let refreshCount = $state(0);
	let filter = $state('');
	/** The guest whose page is open. */
	let guest = $state<{ node: string; kind: 'qemu' | 'lxc'; vmid: number; name: string | null } | null>(null);

	let params = $derived(tab.proxmoxConnectParams);
	let where = $derived(params ? `${params.username ? params.username + '@' : ''}${params.host}${params.port && params.port !== 8006 ? ':' + params.port : ''}` : '');
	let serverName = $derived(tab.sessionName ?? tab.title);
	let connectionId = $derived(tab.proxmoxConnectionId);
	let pbs = $derived(opened?.product === 'pbs');

	let nodes = $derived((resources ?? []).filter((r) => r.kind === 'node'));
	let storages = $derived((resources ?? []).filter((r) => r.kind === 'storage'));
	let guests = $derived.by(() => {
		const all = (resources ?? []).filter((r) => r.kind === 'qemu' || r.kind === 'lxc');
		const q = filter.trim().toLowerCase();
		if (!q) return all;
		return all.filter((g) => `${g.vmid} ${g.name ?? ''} ${g.node ?? ''} ${g.tags.join(' ')} ${g.status ?? ''}`.toLowerCase().includes(q));
	});
	let counts = $derived.by(() => {
		const list = (resources ?? []).filter((r) => (r.kind === 'qemu' || r.kind === 'lxc') && !r.template);
		const vms = list.filter((g) => g.kind === 'qemu');
		const cts = list.filter((g) => g.kind === 'lxc');
		return {
			vms: vms.length,
			vmsUp: vms.filter((g) => g.status === 'running').length,
			cts: cts.length,
			ctsUp: cts.filter((g) => g.status === 'running').length
		};
	});

	async function connect(typed?: string, code?: string): Promise<void> {
		const p = params;
		if (!p) return;
		stage = { kind: 'connecting' };
		setTabLink(tab.id, 'reconnecting');
		const old = tab.proxmoxConnectionId;
		if (old) {
			setProxmoxConnection(tab.id, undefined);
			await proxmoxDisconnect(old).catch(() => {});
		}
		const id = crypto.randomUUID();
		try {
			opened = await proxmoxConnect(id, p.sessionId, typed, code);
			setProxmoxConnection(tab.id, id);
			setTabLink(tab.id, undefined);
			password = otp = typedPassword = '';
			stage = { kind: 'up' };
			overview = null;
			resources = null;
			if (opened.product === 'pbs') page = 'overview';
			await load(page, true);
		} catch (err) {
			setTabLink(tab.id, 'down');
			const msg = String(err);
			if (msg === PVE_PASSWORD_NEEDED) {
				stage = { kind: 'password' };
			} else if (msg === PVE_TFA_NEEDED) {
				stage = code ? { kind: 'tfa', error: t('proxmox.code_refused') } : { kind: 'tfa' };
				otp = '';
			} else if (msg.startsWith(PVE_LOGIN_REFUSED)) {
				const detail = msg.slice(PVE_LOGIN_REFUSED.length);
				stage = code ? { kind: 'tfa', error: `${t('proxmox.login_refused')} ${detail}`.trim() } : { kind: 'password', error: `${t('proxmox.login_refused')} ${detail}`.trim() };
				otp = '';
			} else if (msg.startsWith(PVE_UNTRUSTED_CERT)) {
				stage = { kind: 'cert', fingerprint: msg.slice(PVE_UNTRUSTED_CERT.length) };
			} else {
				stage = { kind: 'error', message: msg };
			}
			if (stage.kind === 'password' || stage.kind === 'tfa') {
				await tick();
				inputEl?.focus();
			}
		}
	}

	async function trust(fingerprint: string): Promise<void> {
		const p = params;
		if (!p) return;
		try {
			await proxmoxTrustCert(p.sessionId, fingerprint);
			await connect();
		} catch (err) {
			stage = { kind: 'error', message: String(err) };
		}
	}

	/** Read a page; `force` reads it again even when it was read before. */
	async function load(which: Page, force = false): Promise<void> {
		const id = tab.proxmoxConnectionId;
		if (!id || pbs) return;
		pageError = null;
		if (which === 'tasks') {
			if (force) refreshCount++;
			return;
		}
		if (!force && ((which === 'overview' && overview) || (which !== 'overview' && resources))) return;
		loading = true;
		try {
			if (which === 'overview') {
				overview = await proxmoxOverview(id);
				resources = await proxmoxResources(id);
			} else {
				resources = await proxmoxResources(id);
			}
		} catch (err) {
			pageError = proxmoxError(err);
		} finally {
			loading = false;
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
		void openUrl(`https://${host}:${p.port || 8006}/`).catch((e) => addToast(String(e), 'error'));
	}

	onMount(() => {
		if (tab.proxmoxConnectionId) {
			stage = { kind: 'up' };
			void load(page);
		} else void connect();
	});

	// The tab menu's Reconnect.
	$effect(() => {
		const bump = tab.reconnect;
		if (bump === seenReconnect) return;
		seenReconnect = bump;
		void connect();
	});

	// Guests and nodes change: read them again while in view.
	$effect(() => {
		if (!active || stage.kind !== 'up' || page === 'tasks' || guest || pbs) return;
		const which = page;
		const timer = setInterval(() => {
			if (!loading && document.visibilityState === 'visible') void load(which, true);
		}, REFRESH_MS);
		return () => clearInterval(timer);
	});

	/* ---- Consoles ---- */

	/** The console waiting for the console login. */
	let pendingConsole = $state<{ kind: ConsoleKind; target: Omit<ProxmoxConsole, 'sessionId'>; name: string | null } | null>(null);
	let consoleLogin = $state<{ need: 'password' | 'tfa'; error?: string } | null>(null);
	let consolePassword = $state('');
	let consoleOtp = $state('');
	let consoleBusy = $state(false);

	function launch(): void {
		const p = params;
		const c = pendingConsole;
		if (!p || !c) return;
		pendingConsole = null;
		const target: ProxmoxConsole = { ...c.target, sessionId: p.sessionId };
		openProxmoxConsole(c.kind, target, { host: p.host, port: p.port || 8006 }, consoleTitle(target, c.name));
	}

	/** Open a console, after the user login it needs when there is none yet. */
	async function openConsole(kind: ConsoleKind, target: Omit<ProxmoxConsole, 'sessionId'>, name: string | null): Promise<void> {
		const id = tab.proxmoxConnectionId;
		if (!id) return;
		pendingConsole = { kind, target, name };
		try {
			if (await proxmoxConsoleReady(id)) return launch();
			await proxmoxConsoleLogin(id);
			launch();
		} catch (err) {
			consoleFailed(String(err), false);
		}
	}

	function consoleFailed(msg: string, withCode: boolean): void {
		consoleOtp = '';
		if (msg === PVE_PASSWORD_NEEDED) consoleLogin = { need: 'password' };
		else if (msg === PVE_TFA_NEEDED) consoleLogin = { need: 'tfa', error: withCode ? t('proxmox.code_refused') : undefined };
		else if (msg.startsWith(PVE_LOGIN_REFUSED)) {
			const error = `${t('proxmox.login_refused')} ${msg.slice(PVE_LOGIN_REFUSED.length)}`.trim();
			consoleLogin = { need: withCode ? 'tfa' : 'password', error };
		} else {
			pendingConsole = null;
			consoleLogin = null;
			addToast(proxmoxError(msg), 'error');
		}
	}

	async function submitConsoleLogin(): Promise<void> {
		const id = tab.proxmoxConnectionId;
		const need = consoleLogin?.need;
		if (!id || !need || consoleBusy) return;
		consoleBusy = true;
		try {
			await proxmoxConsoleLogin(id, consolePassword || undefined, need === 'tfa' ? consoleOtp : undefined);
			consoleLogin = null;
			consolePassword = consoleOtp = '';
			launch();
		} catch (err) {
			consoleFailed(String(err), need === 'tfa');
		} finally {
			consoleBusy = false;
		}
	}

	function cancelConsoleLogin(): void {
		consoleLogin = null;
		pendingConsole = null;
		consolePassword = consoleOtp = '';
	}

	/* ---- Node actions ---- */

	let nodeConfirm = $state<{ node: string; command: 'reboot' | 'shutdown'; typed: string } | null>(null);

	async function runNodeAction(): Promise<void> {
		const c = nodeConfirm;
		const id = tab.proxmoxConnectionId;
		if (!c || !id || c.typed.trim() !== c.node) return;
		nodeConfirm = null;
		try {
			await proxmoxNodeAction(id, c.node, c.command);
			addToast(t(c.command === 'reboot' ? 'proxmox.node_rebooting' : 'proxmox.node_shutting_down', { node: c.node }), 'success');
		} catch (err) {
			addToast(proxmoxError(err), 'error');
		}
	}

	function nodeResource(name: string): ProxmoxResource | undefined {
		return nodes.find((n) => n.node === name);
	}
</script>

{#snippet dot(status: string | null | undefined)}
	<span class="dot {stateClass(status)}" title={stateLabel(status)}></span>
{/snippet}

{#snippet bar(used: number | null | undefined, total: number | null | undefined)}
	{@const pct = share(used, total)}
	<span class="bar" title={pct === null ? '' : `${pct.toFixed(0)} %`}>
		<span class="fill" class:high={pct !== null && pct >= 90} style:width={`${pct ?? 0}%`}></span>
	</span>
{/snippet}

{#if stage.kind === 'up' && connectionId}
	<div class="pve">
		<header class="top">
			<div class="who">
				<svg class="logo" viewBox="0 0 24 24" aria-hidden="true"><path d="M4.928 1.825c-1.09.553-1.09.64-.07 1.78 5.655 6.295 7.004 7.782 7.107 7.782.139.017 7.971-8.542 8.058-8.801.034-.07-.208-.312-.519-.536-.415-.312-.864-.433-1.712-.467-1.59-.104-2.144.242-4.115 2.455-.899 1.003-1.66 1.833-1.66 1.833-.017 0-.76-.813-1.642-1.798S8.473 2.1 8.127 1.91c-.796-.45-2.421-.484-3.2-.086zM1.297 4.367C.45 4.695 0 5.007 0 5.248c0 .121 1.331 1.678 2.94 3.459 1.625 1.78 2.939 3.268 2.939 3.302 0 .035-1.331 1.522-2.94 3.303C1.314 17.11.017 18.683.035 18.822c.086.467 1.504 1.055 2.541 1.055 1.678-.018 2.058-.312 5.603-4.202 1.78-1.954 3.233-3.614 3.233-3.666 0-.069-1.435-1.694-3.199-3.63-2.3-2.508-3.423-3.632-3.96-3.874-.812-.398-2.126-.467-2.956-.138zm18.467.12c-.502.26-1.764 1.505-3.943 3.891-1.763 1.937-3.199 3.562-3.199 3.631 0 .07 1.453 1.712 3.234 3.666 3.544 3.89 3.925 4.184 5.602 4.202 1.038 0 2.455-.588 2.542-1.055.017-.156-1.28-1.712-2.905-3.493-1.608-1.78-2.94-3.285-2.94-3.32 0-.034 1.332-1.539 2.94-3.32C22.72 6.91 24.017 5.352 24 5.214c-.087-.45-1.366-.968-2.473-1.038-.795-.034-1.21.035-1.763.312zM7.954 16.973c-2.144 2.369-3.908 4.374-3.943 4.46-.034.07.208.312.52.537.414.311.864.432 1.711.467 1.574.103 2.161-.26 4.15-2.508.864-.968 1.608-1.78 1.625-1.78s.761.812 1.643 1.798c2.023 2.248 2.559 2.576 4.132 2.49.848-.035 1.297-.156 1.712-.467.311-.225.553-.467.519-.536-.087-.26-7.92-8.819-8.058-8.801-.069 0-1.867 1.954-4.011 4.34z" /></svg>
				<div class="who-text">
					<span class="name">{serverName}</span>
					<span class="sub">{[pbs ? 'Proxmox Backup Server' : 'Proxmox VE', opened?.version, overview?.clusterName ? t('proxmox.cluster_named', { name: overview.clusterName }) : null, where].filter(Boolean).join(' · ')}</span>
				</div>
			</div>
			<div class="top-actions">
				{#if opened?.token}
					<span class="pill" title={t('proxmox.token_hint', { user: opened.user })}>{t('proxmox.token_pill')}</span>
				{/if}
				{#if !isMobile()}
					<Button variant="ghost" size="sm" onclick={openWeb}>{t('proxmox.open_web')}</Button>
				{/if}
				<Button variant="secondary" size="sm" disabled={loading} onclick={() => void load(page, true)}>{t('proxmox.refresh')}</Button>
			</div>
		</header>
		{#if pbs}
			<div class="body">
				<p class="note">{t('proxmox.pbs_later')}</p>
				<div><Button variant="secondary" size="sm" onclick={openWeb}>{t('proxmox.open_web')}</Button></div>
			</div>
		{:else}
			<div class="pages" role="tablist">
				{#each PAGES as p (p)}
					<button type="button" role="tab" class="page-tab" class:active={page === p} aria-selected={page === p} onclick={() => show(p)}>{t(`proxmox.page_${p}`)}</button>
				{/each}
			</div>
			<div class="body">
				{#if pageError}
					<p class="note error">{pageError}</p>
				{/if}
				{#if loading && !resources}
					<p class="note">{t('proxmox.loading')}</p>
				{/if}

				{#if page === 'overview' && overview}
					<div class="tiles">
						<div class="tile"><span class="tile-n">{counts.vmsUp}<small>/{counts.vms}</small></span><span class="tile-l">{t('proxmox.vms_running')}</span></div>
						<div class="tile"><span class="tile-n">{counts.ctsUp}<small>/{counts.cts}</small></span><span class="tile-l">{t('proxmox.cts_running')}</span></div>
						<div class="tile"><span class="tile-n">{overview.nodes.filter((n) => n.online).length}<small>/{overview.nodes.length}</small></span><span class="tile-l">{t('proxmox.nodes_online')}</span></div>
						{#if overview.quorate !== null}
							<div class="tile"><span class="tile-n" class:bad={!overview.quorate}>{overview.quorate ? t('proxmox.yes') : t('proxmox.no')}</span><span class="tile-l">{t('proxmox.quorate')}</span></div>
						{/if}
					</div>
					{#each overview.nodes as n (n.name)}
						{@const r = nodeResource(n.name)}
						<section class="node">
							<div class="node-head">
								{@render dot(n.online ? 'online' : 'offline')}
								<span class="node-name">{n.name}</span>
								<span class="dim">{[n.ip, n.pveVersion ? `PVE ${n.pveVersion}` : null, n.kernel].filter(Boolean).join(' · ')}</span>
								<span class="spacer"></span>
								{#if n.online}
									<Button variant="secondary" size="sm" onclick={() => void openConsole('term', { node: n.name, kind: 'node' }, null)}>{t('proxmox.shell')}</Button>
									<Button variant="ghost" size="sm" onclick={() => (nodeConfirm = { node: n.name, command: 'reboot', typed: '' })}>{t('proxmox.reboot')}</Button>
									<Button variant="ghost" size="sm" onclick={() => (nodeConfirm = { node: n.name, command: 'shutdown', typed: '' })}>{t('proxmox.shutdown')}</Button>
								{/if}
							</div>
							{#if n.online}
								<dl class="facts">
									<dt>{t('proxmox.cpu')}</dt>
									<dd>{@render bar((n.cpu ?? r?.cpu ?? 0) * 100, 100)} {cpu(n.cpu ?? r?.cpu)} · {n.cpus ?? r?.maxcpu ?? '—'} × {n.cpuModel ?? ''}</dd>
									<dt>{t('proxmox.load')}</dt>
									<dd class="mono">{n.load.length ? n.load.map((l) => l.toFixed(2)).join(' ') : '—'}</dd>
									<dt>{t('proxmox.memory')}</dt>
									<dd>{@render bar(n.memUsed, n.memTotal)} {bytes(n.memUsed)} / {bytes(n.memTotal)}</dd>
									{#if n.swapTotal}
										<dt>{t('proxmox.swap')}</dt>
										<dd>{@render bar(n.swapUsed, n.swapTotal)} {bytes(n.swapUsed)} / {bytes(n.swapTotal)}</dd>
									{/if}
									<dt>{t('proxmox.root_disk')}</dt>
									<dd>{@render bar(n.rootUsed, n.rootTotal)} {bytes(n.rootUsed)} / {bytes(n.rootTotal)}</dd>
									<dt>{t('proxmox.uptime')}</dt>
									<dd>{uptime(n.uptime)}</dd>
								</dl>
							{/if}
						</section>
					{/each}
				{:else if page === 'guests' && resources}
					<div class="toolbar">
						<input class="filter" type="search" bind:value={filter} placeholder={t('proxmox.filter')} autocomplete="off" spellcheck="false" />
						<span class="dim">{t('proxmox.guest_count', { count: String(guests.length) })}</span>
					</div>
					{#if guests.length === 0}
						<p class="note">{t('proxmox.no_guests')}</p>
					{:else}
						<div class="table-box">
							<table class="pve-table guests">
								<thead>
									<tr>
										<th></th>
										<th class="r">{t('proxmox.col_id')}</th>
										<th>{t('proxmox.col_name')}</th>
										<th class="hide-narrow">{t('proxmox.col_node')}</th>
										<th class="r">{t('proxmox.cpu')}</th>
										<th class="r hide-narrow">{t('proxmox.memory')}</th>
										<th class="r hide-narrow">{t('proxmox.uptime')}</th>
										<th class="hide-narrow">{t('proxmox.col_tags')}</th>
									</tr>
								</thead>
								<tbody>
									{#each guests as g (g.id)}
										<tr class="row" onclick={() => (guest = { node: g.node ?? '', kind: g.kind as 'qemu' | 'lxc', vmid: g.vmid ?? 0, name: g.name })}>
											<td class="dot-cell">{@render dot(g.template ? null : g.status)}</td>
											<td class="r mono">{g.vmid}</td>
											<td class="name-cell">
												<span class="kind-tag">{g.template ? t('proxmox.template') : g.kind === 'qemu' ? 'VM' : 'CT'}</span>
												{g.name ?? '—'}
												{#if g.lock}<span class="lock" title={t('proxmox.locked', { lock: g.lock })}>🔒</span>{/if}
											</td>
											<td class="hide-narrow">{g.node}</td>
											<td class="r mono">{g.status === 'running' ? cpu(g.cpu) : ''}</td>
											<td class="r mono hide-narrow">{g.status === 'running' ? `${bytes(g.mem)} / ${bytes(g.maxmem)}` : bytes(g.maxmem)}</td>
											<td class="r hide-narrow">{g.status === 'running' ? uptime(g.uptime) : ''}</td>
											<td class="hide-narrow">{#each g.tags as tag (tag)}<span class="tag">{tag}</span>{/each}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				{:else if page === 'storage' && resources}
					{#if storages.length === 0}
						<p class="note">{t('proxmox.no_storage')}</p>
					{:else}
						<div class="table-box">
							<table class="pve-table">
								<thead>
									<tr>
										<th></th>
										<th>{t('proxmox.col_storage')}</th>
										<th class="hide-narrow">{t('proxmox.col_node')}</th>
										<th class="hide-narrow">{t('proxmox.col_type')}</th>
										<th class="hide-narrow">{t('proxmox.col_content')}</th>
										<th>{t('proxmox.col_usage')}</th>
									</tr>
								</thead>
								<tbody>
									{#each storages as s (s.id)}
										<tr>
											<td class="dot-cell">{@render dot(s.status)}</td>
											<td>{s.storage}{#if s.shared} <span class="tag">{t('proxmox.shared')}</span>{/if}</td>
											<td class="hide-narrow">{s.node}</td>
											<td class="hide-narrow">{s.plugintype ?? '—'}</td>
											<td class="hide-narrow dim">{(s.content ?? '').split(',').join(', ')}</td>
											<td class="usage">{@render bar(s.disk, s.maxdisk)} <span class="mono">{bytes(s.disk)} / {bytes(s.maxdisk)}</span></td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				{:else if page === 'tasks'}
					<ProxmoxTasks {connectionId} refresh={refreshCount} {active} />
				{/if}
			</div>
		{/if}
	</div>
{:else}
	<div class="connect">
		<div class="card">
			<div class="title">{serverName}</div>
			<div class="where">{where}</div>
			{#if stage.kind === 'connecting'}
				<p class="note">{t('proxmox.connecting')}</p>
			{:else if stage.kind === 'password'}
				<form
					onsubmit={(e) => {
						e.preventDefault();
						if (password) {
							typedPassword = password;
							void connect(password);
						}
					}}
				>
					<label class="field">
						<span>{t('session.password')}</span>
						<input bind:this={inputEl} type="password" bind:value={password} autocomplete="off" />
					</label>
					{#if stage.error}<p class="note error">{stage.error}</p>{/if}
					<div class="actions">
						<Button type="submit" disabled={!password}>{t('proxmox.connect')}</Button>
					</div>
				</form>
			{:else if stage.kind === 'tfa'}
				<form
					onsubmit={(e) => {
						e.preventDefault();
						if (otp.trim()) void connect(typedPassword || undefined, otp.trim());
					}}
				>
					<label class="field">
						<span>{t('proxmox.code')}</span>
						<input bind:this={inputEl} type="text" inputmode="numeric" autocomplete="one-time-code" bind:value={otp} spellcheck="false" />
					</label>
					<p class="note small">{t('proxmox.code_hint')}</p>
					{#if stage.error}<p class="note error">{stage.error}</p>{/if}
					<div class="actions">
						<Button type="submit" disabled={!otp.trim()}>{t('proxmox.connect')}</Button>
					</div>
				</form>
			{:else if stage.kind === 'cert'}
				<p class="note">{t('proxmox.cert_untrusted')}</p>
				<code class="fp">{stage.fingerprint}</code>
				<p class="note small">{t('proxmox.cert_hint')}</p>
				<div class="actions">
					<Button variant="secondary" onclick={() => (stage = { kind: 'error', message: t('proxmox.cert_refused') })}>{t('common.cancel')}</Button>
					<Button onclick={() => void trust((stage as { fingerprint: string }).fingerprint)}>{t('proxmox.cert_trust')}</Button>
				</div>
			{:else if stage.kind === 'error'}
				<p class="note error">{stage.message}</p>
				<div class="actions">
					{#if !isMobile()}<Button variant="ghost" onclick={openWeb}>{t('proxmox.open_web')}</Button>{/if}
					<Button onclick={() => void connect()}>{t('proxmox.retry')}</Button>
				</div>
			{/if}
		</div>
	</div>
{/if}

{#if guest && connectionId}
	<ProxmoxGuest
		{connectionId}
		node={guest.node}
		kind={guest.kind}
		vmid={guest.vmid}
		onclose={() => {
			guest = null;
			void load(page, true);
		}}
		onconsole={(kind, target, name) => void openConsole(kind, target, name)}
	/>
{/if}

<Modal open={consoleLogin !== null} onclose={cancelConsoleLogin} title={t('proxmox.console_login_title')} maxWidth="420px">
	{#if consoleLogin}
		<form class="dialog-form" onsubmit={(e) => { e.preventDefault(); void submitConsoleLogin(); }}>
			<p class="note">{t('proxmox.console_login_intro')}</p>
			{#if consoleLogin.need === 'password'}
				<label class="field">
					<span>{t('session.password')}</span>
					<input type="password" bind:value={consolePassword} autocomplete="off" />
				</label>
			{:else}
				<label class="field">
					<span>{t('proxmox.code')}</span>
					<input type="text" inputmode="numeric" autocomplete="one-time-code" bind:value={consoleOtp} spellcheck="false" />
				</label>
				<p class="note small">{t('proxmox.code_hint')}</p>
			{/if}
			{#if consoleLogin.error}<p class="note error">{consoleLogin.error}</p>{/if}
			<div class="actions">
				<Button variant="ghost" size="sm" type="button" onclick={cancelConsoleLogin}>{t('common.cancel')}</Button>
				<Button size="sm" type="submit" disabled={consoleBusy || (consoleLogin.need === 'password' ? !consolePassword : !consoleOtp.trim())}>{t('proxmox.console_open')}</Button>
			</div>
		</form>
	{/if}
</Modal>

<Modal open={nodeConfirm !== null} onclose={() => (nodeConfirm = null)} title={nodeConfirm?.command === 'reboot' ? t('proxmox.node_reboot_title') : t('proxmox.node_shutdown_title')} maxWidth="440px">
	{#if nodeConfirm}
		<form class="dialog-form" onsubmit={(e) => { e.preventDefault(); void runNodeAction(); }}>
			<p class="note">{nodeConfirm.command === 'reboot' ? t('proxmox.node_reboot_intro', { node: nodeConfirm.node }) : t('proxmox.node_shutdown_intro', { node: nodeConfirm.node })}</p>
			<label class="field">
				<span>{t('proxmox.type_node_name', { node: nodeConfirm.node })}</span>
				<input type="text" bind:value={nodeConfirm.typed} autocomplete="off" spellcheck="false" />
			</label>
			<div class="actions">
				<Button variant="ghost" size="sm" type="button" onclick={() => (nodeConfirm = null)}>{t('common.cancel')}</Button>
				<Button variant="danger" size="sm" type="submit" disabled={nodeConfirm.typed.trim() !== nodeConfirm.node}>{nodeConfirm.command === 'reboot' ? t('proxmox.reboot') : t('proxmox.shutdown')}</Button>
			</div>
		</form>
	{/if}
</Modal>

<style>
	.pve {
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
	.logo {
		width: 22px;
		height: 22px;
		fill: #e57000;
		flex-shrink: 0;
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
	.top-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.pill {
		font-size: 11px;
		padding: 2px 8px;
		border-radius: 999px;
		border: 1px solid var(--color-border);
		color: var(--color-text-secondary);
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
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
		gap: 10px;
		max-width: 760px;
	}
	.tile {
		border: 1px solid var(--color-border);
		border-radius: 8px;
		padding: 10px 12px;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.tile-n {
		font-size: 20px;
		font-weight: 600;
	}
	.tile-n small {
		font-size: 13px;
		font-weight: 400;
		color: var(--color-text-tertiary);
	}
	.tile-n.bad {
		color: var(--color-danger);
	}
	.tile-l {
		font-size: 12px;
		color: var(--color-text-tertiary);
	}
	.node {
		border: 1px solid var(--color-border);
		border-radius: 8px;
		padding: 10px 12px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		max-width: 760px;
	}
	.node-head {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		font-size: 13px;
	}
	.node-name {
		font-weight: 600;
	}
	.spacer {
		flex: 1;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 6px 18px;
		margin: 0;
		font-size: 13px;
	}
	.facts dt {
		color: var(--color-text-tertiary);
	}
	.facts dd {
		margin: 0;
		display: flex;
		align-items: center;
		gap: 8px;
		word-break: break-word;
		flex-wrap: wrap;
	}
	.bar {
		display: inline-block;
		width: 90px;
		height: 6px;
		border-radius: 3px;
		background: var(--color-border);
		overflow: hidden;
		flex-shrink: 0;
	}
	.fill {
		display: block;
		height: 100%;
		background: var(--color-accent);
	}
	.fill.high {
		background: var(--color-danger, #d64545);
	}
	.toolbar {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
	}
	.filter {
		flex: 1;
		max-width: 320px;
		min-width: 160px;
		padding: 6px 9px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.table-box {
		border: 1px solid var(--color-border);
		border-radius: 8px;
		overflow-x: auto;
	}
	.pve-table {
		width: 100%;
		border-collapse: collapse;
		font-size: 13px;
	}
	.pve-table th {
		text-align: left;
		font-weight: 500;
		font-size: 12px;
		color: var(--color-text-tertiary);
		padding: 6px 10px;
		border-bottom: 1px solid var(--color-border);
		white-space: nowrap;
	}
	.pve-table td {
		padding: 6px 10px;
		border-top: 1px solid var(--color-border);
		white-space: nowrap;
	}
	.pve-table tr.row {
		cursor: pointer;
	}
	.pve-table tr.row:hover td {
		background: var(--color-bg-hover, rgba(127, 127, 127, 0.08));
	}
	.name-cell {
		width: 100%;
		white-space: normal !important;
	}
	.usage {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.kind-tag {
		display: inline-block;
		font-size: 10px;
		padding: 0 5px;
		margin-right: 6px;
		border-radius: 4px;
		border: 1px solid var(--color-border);
		color: var(--color-text-tertiary);
	}
	.tag {
		display: inline-block;
		font-size: 11px;
		padding: 0 6px;
		margin-right: 4px;
		border-radius: 999px;
		background: var(--color-surface-sunken, rgba(127, 127, 127, 0.12));
		color: var(--color-text-secondary);
	}
	.lock {
		font-size: 11px;
		margin-left: 4px;
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
		font-size: 12px;
	}
	.dot-cell {
		width: 14px;
	}
	.dot {
		display: inline-block;
		width: 9px;
		height: 9px;
		border-radius: 50%;
		flex-shrink: 0;
		background: transparent;
		border: 1px solid var(--color-text-tertiary);
	}
	.dot.ok {
		background: var(--color-success, #2e9d5b);
		border-color: transparent;
	}
	.dot.warn {
		background: var(--color-warning, #d99a1e);
		border-color: transparent;
	}
	.dot.crit {
		background: var(--color-danger, #d64545);
		border-color: transparent;
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
	@media (max-width: 640px) {
		.hide-narrow {
			display: none;
		}
		.facts {
			grid-template-columns: 1fr;
			gap: 2px;
		}
		.facts dd {
			margin-bottom: 6px;
		}
	}
</style>
