<script lang="ts">
	import { open as openDialog } from '@tauri-apps/plugin-dialog';
	import { writeText } from '@tauri-apps/plugin-clipboard-manager';
	import {
		SERVER_LABELS,
		generatePassword,
		isPrivateIp,
		localServerCheckPort,
		localServerClearLog,
		localServerStart,
		localServerStop,
		serverUrl,
		type NetInterface,
		type PortCheck,
		type ServerConfig,
		type ServerStatus
	} from '#lib/ipc/localServers.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Toggle from '#lib/components/shared/Toggle.svelte';

	interface Props {
		config: ServerConfig;
		status?: ServerStatus;
		interfaces: NetInterface[];
		expanded: boolean;
		ontoggle: () => void;
		onchange: () => void;
		onstatus: (status: ServerStatus) => void;
	}

	let { config = $bindable(), status, interfaces, expanded, ontoggle, onchange, onstatus }: Props = $props();

	const kind = $derived(config.kind);
	const label = $derived(SERVER_LABELS[kind]);
	const running = $derived(status?.running ?? false);
	const hasLogins = $derived(kind !== 'tftp');
	const canAnonymous = $derived(kind === 'ftp' || kind === 'http');

	let busy = $state(false);
	let portCheck = $state<PortCheck | null>(null);
	let revealed = $state<Record<number, boolean>>({});

	const descriptions = {
		tftp: 'servers.tftp_desc',
		ftp: 'servers.ftp_desc',
		sftp: 'servers.sftp_desc',
		http: 'servers.http_desc'
	} as const;

	/* Checked whenever the address or port changes while stopped, so a
	   taken port is known, with a free one offered, before Start. */
	let checkRequest = 0;
	$effect(() => {
		const { bind, port } = config;
		if (running || !expanded) return;
		const request = ++checkRequest;
		const timer = window.setTimeout(async () => {
			if (!Number.isInteger(port) || port < 1 || port > 65535) {
				portCheck = null;
				return;
			}
			try {
				const result = await localServerCheckPort(kind, bind, port);
				if (request === checkRequest) portCheck = result;
			} catch {
				if (request === checkRequest) portCheck = null;
			}
		}, 250);
		return () => window.clearTimeout(timer);
	});

	function changed(): void {
		onchange();
	}

	function portProblem(check: PortCheck): string {
		const port = config.port;
		switch (check.problem) {
			case 'in_use':
				return t('servers.port_in_use', { port });
			case 'privileged':
				return t('servers.port_privileged', { port });
			case 'no_address':
				return t('servers.port_no_address');
			default:
				return t('servers.port_other', { port });
		}
	}

	function usePort(port: number): void {
		config.port = port;
		changed();
	}

	async function chooseFolder(): Promise<void> {
		const picked = await openDialog({
			directory: true,
			multiple: false,
			defaultPath: config.root || undefined,
			title: t('servers.pick_folder')
		}).catch(() => null);
		if (typeof picked === 'string') {
			config.root = picked;
			changed();
		}
	}

	function addUser(): void {
		const n = config.users.length + 1;
		config.users = [...config.users, { username: n === 1 ? 'reacher' : `user${n}`, password: generatePassword() }];
		changed();
	}

	function removeUser(index: number): void {
		config.users = config.users.filter((_, i) => i !== index);
		changed();
	}

	function newPassword(index: number): void {
		config.users[index].password = generatePassword();
		revealed[index] = true;
		changed();
	}

	async function copy(text: string): Promise<void> {
		try {
			await writeText(text);
			addToast(t('servers.copied'), 'success');
		} catch (err) {
			addToast(t('toast.copy_failed_err', { error: String(err) }), 'error');
		}
	}

	async function start(): Promise<void> {
		if (busy) return;
		busy = true;
		try {
			const result = await localServerStart($state.snapshot(config));
			onstatus(result);
			addToast(t('servers.started_toast', { name: label, port: result.port }), 'success');
		} catch (err) {
			addToast(t('servers.start_failed', { name: label, error: String(err) }), 'error');
		} finally {
			busy = false;
		}
	}

	async function stop(): Promise<void> {
		if (busy) return;
		busy = true;
		try {
			onstatus(await localServerStop(kind));
		} catch (err) {
			addToast(String(err), 'error');
		} finally {
			busy = false;
		}
	}

	async function clearLog(): Promise<void> {
		await localServerClearLog(kind).catch(() => {});
		if (status) onstatus({ ...status, log: [] });
	}

	const validPort = $derived(Number.isInteger(config.port) && config.port >= 1 && config.port <= 65535);
	const loginMissing = $derived(hasLogins && !(canAnonymous && config.anonymous) && !config.users.some((u) => u.username.trim()));
	const exposure = $derived.by(() => {
		if (config.bind === '0.0.0.0') return t('servers.warn_all');
		if (config.bind !== '127.0.0.1' && !isPrivateIp(config.bind)) return t('servers.warn_public', { ip: config.bind });
		return '';
	});
	const firstUser = $derived(config.users.find((u) => u.username.trim())?.username);
	const urls = $derived(
		running && status ? status.addresses.map((ip) => serverUrl(kind, ip, status.port, firstUser)) : []
	);
	const ifaceKnown = $derived(
		config.bind === '0.0.0.0' || config.bind === '127.0.0.1' || interfaces.some((i) => i.ip === config.bind)
	);

	let logBox = $state<HTMLDivElement>();
	$effect(() => {
		void status?.log.length;
		if (logBox) logBox.scrollTop = logBox.scrollHeight;
	});
</script>

<div class="server-card" class:running>
	<div class="header">
		<button class="title" onclick={ontoggle} aria-expanded={expanded}>
			<svg class="chevron" class:open={expanded} width="10" height="10" viewBox="0 0 16 16" fill="none">
				<path d="M6 3l5 5-5 5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
			</svg>
			<span class="dot" class:on={running} aria-hidden="true"></span>
			<span class="name">{label}</span>
			<span class="state">
				{running && status ? `:${status.port}` : t('servers.stopped')}
			</span>
		</button>
		{#if running}
			<Button variant="ghost" size="sm" onclick={stop} disabled={busy}>{t('servers.stop')}</Button>
		{:else}
			<Button variant="primary" size="sm" onclick={start} disabled={busy || !config.root || loginMissing || !validPort}>
				{busy ? t('servers.starting') : t('servers.start')}
			</Button>
		{/if}
	</div>

	{#if expanded}
		<div class="body">
			<p class="desc">{t(descriptions[kind])}</p>

			{#if running && status}
				<div class="section">
					<span class="section-label">{t('servers.addresses')}</span>
					{#each urls as url (url)}
						<div class="copy-row">
							<code>{url}</code>
							<button class="icon-btn" onclick={() => copy(url)} title={t('servers.copy')} aria-label={t('servers.copy')}>
								<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" /><path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" /></svg>
							</button>
						</div>
					{/each}
					{#if status.fingerprint}
						<span class="section-label">{t('servers.fingerprint')}</span>
						<div class="copy-row">
							<code class="small">{status.fingerprint}</code>
							<button class="icon-btn" onclick={() => copy(status.fingerprint ?? '')} title={t('servers.copy')} aria-label={t('servers.copy')}>
								<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" /><path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" /></svg>
							</button>
						</div>
					{/if}
				</div>
			{/if}

			<div class="settings">
				<label class="field">
					<span>{t('servers.folder')}</span>
					<div class="inline">
						<input type="text" bind:value={config.root} onchange={changed} spellcheck="false" disabled={running} />
						<button class="small-btn" onclick={chooseFolder} disabled={running}>{t('servers.choose_folder')}</button>
					</div>
				</label>

				<div class="inline two">
					<label class="field grow">
						<span>{t('servers.listen_on')}</span>
						<select bind:value={config.bind} onchange={changed} disabled={running}>
							{#each interfaces.filter((i) => !i.loopback) as iface (iface.ip)}
								<option value={iface.ip}>{iface.ip} ({iface.name})</option>
							{/each}
							<option value="127.0.0.1">{t('servers.this_computer')}</option>
							<option value="0.0.0.0">{t('servers.all_interfaces')}</option>
							{#if !ifaceKnown}
								<option value={config.bind}>{config.bind}</option>
							{/if}
						</select>
					</label>
					<label class="field port">
						<span>{t('servers.port')}</span>
						<input type="number" min="1" max="65535" bind:value={config.port} onchange={changed} disabled={running} />
					</label>
				</div>
				{#if !running && portCheck && !portCheck.free}
					<div class="notice warn">
						<span>{portProblem(portCheck)}</span>
						{#if portCheck.suggestion}
							<button class="small-btn accent" onclick={() => usePort(portCheck?.suggestion ?? config.port)}>
								{t('servers.use_port', { port: portCheck.suggestion })}
							</button>
						{/if}
					</div>
				{/if}
				{#if exposure}
					<p class="notice warn">{exposure}</p>
				{/if}

				{#if hasLogins}
					<div class="section">
						<span class="section-label">{t('servers.users')}</span>
						{#each config.users as user, i (i)}
							<div class="user-row">
								<input
									class="user-name"
									type="text"
									bind:value={user.username}
									onchange={changed}
									disabled={running}
									placeholder={t('servers.username')}
									aria-label={t('servers.username')}
									spellcheck="false"
									autocomplete="off"
								/>
								<input
									class="user-pass"
									type={revealed[i] ? 'text' : 'password'}
									bind:value={user.password}
									onchange={changed}
									disabled={running}
									placeholder={t('servers.password')}
									aria-label={t('servers.password')}
									spellcheck="false"
									autocomplete="off"
								/>
							</div>
							<div class="user-actions">
								<button class="small-btn" onclick={() => (revealed[i] = !revealed[i])}>
									{revealed[i] ? t('servers.hide') : t('servers.show')}
								</button>
								<button class="small-btn" onclick={() => copy(user.password)}>{t('servers.copy_password')}</button>
								<button class="small-btn" onclick={() => newPassword(i)} disabled={running}>{t('servers.new_password')}</button>
								<button class="small-btn danger" onclick={() => removeUser(i)} disabled={running}>{t('servers.remove_user')}</button>
							</div>
						{/each}
						<button class="small-btn add" onclick={addUser} disabled={running}>+ {t('servers.add_user')}</button>
					</div>
					{#if canAnonymous}
						<Toggle bind:checked={config.anonymous} label={t('servers.anonymous')} onchange={changed} disabled={running} />
					{/if}
					{#if loginMissing}
						<p class="notice">{t('servers.no_users')}</p>
					{/if}
				{:else}
					<p class="notice">{t('servers.tftp_no_login')}</p>
				{/if}

				<Toggle bind:checked={config.allowWrite} label={t('servers.allow_write')} onchange={changed} disabled={running} />
				{#if kind === 'ftp' || kind === 'http'}
					<p class="notice">{t('servers.warn_plain', { name: label })}</p>
				{/if}
			</div>

			<div class="section">
				<div class="log-head">
					<span class="section-label">{t('servers.log')}</span>
					{#if status?.log.length}
						<button class="small-btn" onclick={clearLog}>{t('servers.clear_log')}</button>
					{/if}
				</div>
				<div class="log" bind:this={logBox}>
					{#if !status?.log.length}
						<span class="muted">{t('servers.log_empty')}</span>
					{:else}
						{#each status.log as line, i (i)}
							<div class="log-line">
								<span class="muted">{line.time}</span>
								{#if line.peer}<span class="peer">{line.peer}</span>{/if}
								<span>{line.message}</span>
							</div>
						{/each}
					{/if}
				</div>
			</div>
		</div>
	{/if}
</div>

<style>
	.server-card {
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card, 8px);
		background: var(--color-bg-elevated);
		overflow: hidden;
	}

	.header {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 6px 8px 6px 4px;
	}

	.title {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 2px 4px;
		border: none;
		background: transparent;
		color: var(--color-text-primary);
		font: inherit;
		cursor: pointer;
		text-align: left;
	}

	.chevron {
		flex-shrink: 0;
		color: var(--color-text-secondary);
		transition: transform var(--duration-default) var(--ease-default);
	}

	.chevron.open {
		transform: rotate(90deg);
	}

	.dot {
		flex-shrink: 0;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--color-text-tertiary, var(--color-border-strong));
	}

	.dot.on {
		background: var(--color-success, #34c759);
		box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-success, #34c759) 25%, transparent);
	}

	.name {
		font-size: 0.8125rem;
		font-weight: 600;
	}

	.state {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.body {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 2px 10px 10px;
		border-top: 1px solid var(--color-border);
	}

	.desc {
		margin: 6px 0 0;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		line-height: 1.4;
	}

	.settings {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}

	input:disabled,
	select:disabled,
	.small-btn:disabled {
		opacity: 0.6;
		cursor: default;
	}

	.field {
		display: grid;
		gap: 3px;
		min-width: 0;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.inline {
		display: flex;
		gap: 4px;
		min-width: 0;
	}

	.two {
		align-items: end;
	}

	.grow {
		flex: 1;
	}

	.port {
		width: 76px;
		flex-shrink: 0;
	}

	input,
	select {
		min-width: 0;
		width: 100%;
		padding: 5px 7px;
		font: inherit;
		font-size: 0.75rem;
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
	}

	input:focus,
	select:focus {
		outline: none;
		border-color: var(--color-accent);
	}

	.section {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.section-label {
		font-size: 0.6875rem;
		font-weight: 600;
		color: var(--color-text-secondary);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.copy-row {
		display: flex;
		align-items: center;
		gap: 4px;
		min-width: 0;
	}

	code {
		flex: 1;
		min-width: 0;
		padding: 3px 6px;
		font-family: var(--font-mono);
		font-size: 0.75rem;
		background: var(--color-bg-primary);
		border-radius: 4px;
		overflow-wrap: anywhere;
		user-select: text;
	}

	code.small {
		font-size: 0.6875rem;
	}

	.icon-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		flex-shrink: 0;
		padding: 0;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.icon-btn:hover {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.small-btn {
		flex-shrink: 0;
		padding: 3px 8px;
		font: inherit;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
		background: transparent;
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		cursor: pointer;
		white-space: nowrap;
	}

	.small-btn:hover:not(:disabled) {
		color: var(--color-text-primary);
		background: var(--color-surface-hover);
	}

	.small-btn.accent {
		color: var(--color-accent);
		border-color: var(--color-accent);
	}

	.small-btn.danger:hover:not(:disabled) {
		color: var(--color-danger, #ff453a);
		border-color: var(--color-danger, #ff453a);
	}

	.small-btn.add {
		align-self: flex-start;
	}

	.user-row {
		display: grid;
		grid-template-columns: minmax(0, 2fr) minmax(0, 3fr);
		gap: 4px;
	}

	.user-actions {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin-bottom: 4px;
	}

	.notice {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: 0;
		font-size: 0.6875rem;
		line-height: 1.4;
		color: var(--color-text-secondary);
	}

	.notice.warn {
		padding: 5px 7px;
		color: var(--color-warning, #ff9f0a);
		background: color-mix(in srgb, var(--color-warning, #ff9f0a) 10%, transparent);
		border-radius: 6px;
	}

	.log-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.log {
		max-height: 180px;
		overflow-y: auto;
		padding: 5px 6px;
		font-family: var(--font-mono);
		font-size: 0.6875rem;
		line-height: 1.45;
		background: var(--color-bg-primary);
		border-radius: 6px;
		user-select: text;
	}

	.log-line {
		overflow-wrap: anywhere;
	}

	.log-line span + span {
		margin-left: 6px;
	}

	.muted {
		color: var(--color-text-secondary);
	}

	.peer {
		color: var(--color-accent);
	}
</style>
