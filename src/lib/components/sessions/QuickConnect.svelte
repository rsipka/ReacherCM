<script lang="ts">
	import { untrack } from 'svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import KeyPathInput from '#lib/components/sessions/KeyPathInput.svelte';
	import KeyPicker from '#lib/components/sessions/KeyPicker.svelte';
	import ConnectSteps from '#lib/components/sessions/ConnectSteps.svelte';
	import { sshConnect, type JumpHostConnectParams } from '#lib/ipc/ssh.js';
	import { DEFAULT_TELNET_OPTIONS } from '#lib/ipc/telnet.js';
	import { DEFAULT_SERIAL_OPTIONS, SERIAL_BAUD_RATES, serialListPorts, type SerialPortInfo } from '#lib/ipc/serial.js';
	import { isMobile, isWindows } from '#lib/platform.js';
	import { setActivePage } from '#lib/state/navigation.svelte.js';
	import { createTab } from '#lib/state/tabs.svelte.js';
	import { getSettings } from '#lib/state/settings.svelte.js';
	import { validatedSessionLogConfig } from '#lib/state/log-folder.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';

	/** What the sessions search field parsed out of `user@host:port`. */
	export interface QuickConnectPrefill {
		host: string;
		username?: string;
		port?: number;
	}

	interface Props {
		open: boolean;
		prefill?: QuickConnectPrefill | null;
	}

	let { open = $bindable(), prefill = null }: Props = $props();

	type Protocol = 'ssh' | 'rdp' | 'telnet' | 'serial';
	const DEFAULT_PORT: Record<Exclude<Protocol, 'serial'>, string> = { ssh: '22', rdp: '3389', telnet: '23' };
	/** Serial ports open only in the desktop app. */
	const protocols: Protocol[] = isMobile() ? ['ssh', 'rdp', 'telnet'] : ['ssh', 'rdp', 'telnet', 'serial'];
	/** Short names for the switch; the editor's long "Remote desktop (RDP)" does not fit. */
	const PROTOCOL_LABEL: Record<Protocol, string> = { ssh: 'SSH', rdp: 'RDP', telnet: 'Telnet', serial: '' };

	let protocol = $state<Protocol>('ssh');
	let serialPort = $state('');
	let serialPorts = $state<SerialPortInfo[]>([]);
	let serialPortsLoading = $state(false);
	let serialPortTyped = $state(false);
	let baudRate = $state(DEFAULT_SERIAL_OPTIONS.baud_rate);
	let domain = $state('');

	let host = $state('');
	let portStr = $state('22');
	let username = $state('root');
	let authMethod = $state<'password' | 'key'>('password');
	let password = $state('');
	let keyPath = $state('');
	let keyId = $state('');
	let keyPassphrase = $state('');
	let shell = $state('');
	let jumpEnabled = $state(false);
	let jumpHost = $state('');
	let jumpPortStr = $state('22');
	let jumpUsername = $state('root');
	let jumpAuthMethod = $state<'password' | 'key'>('password');
	let jumpPassword = $state('');
	let jumpKeyPath = $state('');
	let jumpKeyPassphrase = $state('');
	let proxyEnabled = $state(false);
	let proxyType = $state<'socks5' | 'socks4' | 'http'>('socks5');
	let proxyHost = $state('127.0.0.1');
	let proxyPort = $state('9050');
	let proxyUsername = $state('');
	let proxyPassword = $state('');
	let connecting = $state(false);
	let error = $state<string | undefined>();
	/** The connection being made, for its steps (ConnectSteps). */
	let attemptId = $state<string | undefined>();

	let port = $derived(parseInt(portStr, 10) || (protocol === 'serial' ? 22 : parseInt(DEFAULT_PORT[protocol], 10)));

	function pickProtocol(next: Protocol): void {
		if (next === protocol) return;
		// Follow the protocol's usual port unless one was typed.
		if (next !== 'serial' && (portStr === '' || Object.values(DEFAULT_PORT).includes(portStr))) {
			portStr = DEFAULT_PORT[next];
		}
		// SSH's default user means nothing to a switch's login prompt.
		if (next !== 'ssh' && username === 'root') username = '';
		if (next === 'ssh' && username === '') username = 'root';
		protocol = next;
		error = undefined;
		if (next === 'serial' && serialPorts.length === 0) void loadSerialPorts();
	}

	async function loadSerialPorts(): Promise<void> {
		serialPortsLoading = true;
		try {
			serialPorts = await serialListPorts();
		} catch {
			serialPorts = [];
		} finally {
			serialPortsLoading = false;
		}
		untrack(() => {
			if (!serialPort.trim() && serialPorts.length > 0) serialPort = serialPorts[0].name;
			serialPortTyped = serialPorts.length === 0 || (!!serialPort.trim() && !serialPorts.some((p) => p.name === serialPort));
		});
	}

	// Opening with an address already typed elsewhere: take it, leave the rest.
	$effect(() => {
		if (!open || !prefill) return;
		const p = prefill;
		untrack(() => {
			protocol = 'ssh';
			host = p.host;
			if (p.username) username = p.username;
			if (p.port) portStr = String(p.port);
		});
	});
	let canConnect = $derived(
		!connecting &&
		(protocol === 'serial'
			? serialPort.trim().length > 0
			: host.trim().length > 0 && (protocol === 'telnet' || username.trim().length > 0))
	);

	/**
	 * Telnet and serial tabs connect by themselves once their terminal
	 * listens; an RDP tab once its panel knows its size.
	 */
	function openTerminalTab(): void {
		setActivePage('terminal');
		if (protocol === 'rdp') {
			const tab = createTab('rdp', `${username.trim()}@${host.trim()}`);
			tab.rdpConnectParams = {
				id: tab.id,
				host: host.trim(),
				port,
				username: username.trim(),
				password,
				domain: domain.trim() || undefined,
				// Filled in by the panel from its own size.
				width: 0,
				height: 0,
			};
		} else if (protocol === 'telnet') {
			const tab = createTab('telnet', host.trim());
			tab.telnetConnectParams = {
				host: host.trim(),
				port,
				options: { ...DEFAULT_TELNET_OPTIONS },
				username: username.trim() || null,
				password: password || null,
			};
		} else {
			const tab = createTab('serial', serialPort.trim());
			tab.serialConnectParams = {
				port: serialPort.trim(),
				options: { ...DEFAULT_SERIAL_OPTIONS, baud_rate: baudRate },
			};
		}
		host = '';
		password = '';
		error = undefined;
		open = false;
	}

	async function handleConnect(): Promise<void> {
		if (!canConnect) return;
		if (protocol !== 'ssh') {
			openTerminalTab();
			return;
		}
		connecting = true;
		error = undefined;

		const id = crypto.randomUUID();
		attemptId = id;

		try {
			const jumpChain: JumpHostConnectParams[] | undefined = jumpEnabled && jumpHost.trim()
				? [{
					host: jumpHost.trim(),
					port: parseInt(jumpPortStr, 10) || 22,
					username: jumpUsername.trim(),
					authMethod: jumpAuthMethod,
					password: jumpAuthMethod === 'password' ? jumpPassword : undefined,
					keyPath: jumpAuthMethod === 'key' ? jumpKeyPath : undefined,
					keyPassphrase: jumpAuthMethod === 'key' && jumpKeyPassphrase ? jumpKeyPassphrase : undefined,
				}]
				: undefined;

			const connectParams = {
				id,
				host: host.trim(),
				port,
				username: username.trim(),
				authMethod,
				password: authMethod === 'password' ? password : undefined,
				keyPath: authMethod === 'key' && !keyId ? keyPath.trim() : undefined,
				keyId: authMethod === 'key' && keyId ? keyId : undefined,
				keyPassphrase: authMethod === 'key' && keyPassphrase ? keyPassphrase : undefined,
				cols: 80,
				rows: 24,
				shell: shell.trim() || undefined,
				injectColors: getSettings().typeShellSetup,
				sessionLog: await validatedSessionLogConfig(),
				showLoginMessage: getSettings().showLoginMessage,
				jumpChain,
				proxy: proxyEnabled ? {
					proxy_type: proxyType,
					host: proxyHost.trim(),
					port: parseInt(proxyPort, 10) || 9050,
					username: proxyUsername.trim() || undefined,
					password: proxyPassword || undefined,
				} : undefined,
			};
			await sshConnect(connectParams);

			const tab = createTab('ssh', `${username.trim()}@${host.trim()}`, id);
			tab.sshConnectParams = connectParams;

			// Reset form
			host = '';
			portStr = '22';
			username = 'root';
			password = '';
			keyPath = '';
			keyId = '';
			keyPassphrase = '';
			shell = '';
			jumpEnabled = false;
			jumpHost = '';
			jumpPortStr = '22';
			jumpUsername = 'root';
			jumpPassword = '';
			jumpKeyPath = '';
			jumpKeyPassphrase = '';
			proxyEnabled = false;
			proxyType = 'socks5';
			proxyHost = '127.0.0.1';
			proxyPort = '9050';
			proxyUsername = '';
			proxyPassword = '';
			error = undefined;
			open = false;
		} catch (err) {
			error = String(err);
		} finally {
			connecting = false;
		}
	}

	function handleClose(): void {
		if (!connecting) {
			open = false;
		}
	}
</script>

<Modal {open} onclose={handleClose} title={t('session.quick_connect')}>
	<form class="form" onsubmit={(e) => { e.preventDefault(); handleConnect(); }}>
		<div class="auth-section">
			<span class="auth-label">{t('session.protocol')}</span>
			<div class="auth-toggle" role="radiogroup" aria-label={t('session.protocol')}>
				{#each protocols as p (p)}
					<button
						type="button"
						role="radio"
						aria-checked={protocol === p}
						class="auth-btn"
						class:active={protocol === p}
						disabled={connecting}
						onclick={() => pickProtocol(p)}
					>
						{p === 'serial' ? t('session.protocol_serial') : PROTOCOL_LABEL[p]}
					</button>
				{/each}
			</div>
		</div>

		{#if protocol === 'serial'}
			<div class="shell-field">
				<label class="auth-label" for="qc-serial-port">{t('serial.port')}</label>
				<div class="port-row">
					<div class="field-host">
						{#if serialPortTyped}
							<Input bind:value={serialPort} placeholder={isWindows() ? 'COM3' : '/dev/ttyUSB0'} />
						{:else}
							<select id="qc-serial-port" class="qc-select" bind:value={serialPort}>
								{#each serialPorts as p (p.name)}
									<option value={p.name}>{p.description ? `${p.name} · ${p.description}` : p.name}</option>
								{/each}
							</select>
						{/if}
					</div>
					<Button variant="secondary" size="sm" onclick={loadSerialPorts} disabled={serialPortsLoading}>
						{t('serial.refresh')}
					</Button>
				</div>
				<p class="shell-hint">
					{#if serialPorts.length === 0 && !serialPortsLoading}
						{t('serial.no_ports')}
					{:else if serialPorts.length > 0}
						<button type="button" class="link-btn" onclick={() => (serialPortTyped = !serialPortTyped)}>
							{serialPortTyped ? t('serial.pick_port') : t('serial.type_port')}
						</button>
					{/if}
				</p>
			</div>
			<div class="shell-field">
				<label class="auth-label" for="qc-serial-baud">{t('serial.speed')}</label>
				<select id="qc-serial-baud" class="qc-select" bind:value={baudRate}>
					{#each SERIAL_BAUD_RATES as rate (rate)}
						<option value={rate}>{rate}</option>
					{/each}
				</select>
				<p class="shell-hint">{t('serial.hint')}</p>
			</div>
		{:else}
		<div class="row">
			<div class="field-host">
				<Input label={t('session.host')} bind:value={host} placeholder="192.168.1.1" disabled={connecting} />
			</div>
			<div class="field-port">
				<Input label={t('session.port')} bind:value={portStr} type="number" placeholder={DEFAULT_PORT[protocol]} disabled={connecting} />
			</div>
		</div>

		{#if protocol === 'rdp'}
			<Input label={t('session.username')} bind:value={username} placeholder="Administrator" />
			<Input label={t('session.password')} bind:value={password} type="password" />
			<Input label={t('session.rdp_domain')} bind:value={domain} />
		{:else if protocol === 'telnet'}
			<Input label={t('session.username_optional')} bind:value={username} placeholder="admin" />
			<Input label={t('session.password_optional')} bind:value={password} type="password" />
			<p class="shell-hint">{t('quick_connect.telnet_hint')}</p>
		{:else}
		<Input label={t('session.username')} bind:value={username} placeholder="root" disabled={connecting} />

		<div class="auth-section">
			<span class="auth-label">{t('session.auth_method')}</span>
			<div class="auth-toggle">
				<button
					type="button"
					class="auth-btn"
					class:active={authMethod === 'password'}
					disabled={connecting}
					onclick={() => (authMethod = 'password')}
				>
					{t('session.auth_password')}
				</button>
				<button
					type="button"
					class="auth-btn"
					class:active={authMethod === 'key'}
					disabled={connecting}
					onclick={() => (authMethod = 'key')}
				>
					{t('session.auth_key')}
				</button>
			</div>
		</div>

		{#if authMethod === 'password'}
			<Input label={t('session.password')} bind:value={password} type="password" disabled={connecting} />
		{:else}
			<KeyPicker bind:path={keyPath} bind:keyId disabled={connecting} />
			{#if !keyId}
				<Input label={t('session.passphrase_optional')} bind:value={keyPassphrase} type="password" disabled={connecting} />
			{/if}
		{/if}

		<div class="shell-field">
			<Input label={t('session.login_shell_optional')} bind:value={shell} placeholder="fish -l" disabled={connecting} />
			<p class="shell-hint">{t('session.login_shell_hint')}</p>
		</div>

		<div class="jump-section">
			<label class="jump-toggle">
				<input type="checkbox" bind:checked={jumpEnabled} disabled={connecting} />
				<span class="jump-toggle-text">{t('session.jump_host_enable')}</span>
			</label>

			{#if jumpEnabled}
				<p class="jump-hint">{t('session.jump_host_hint')}</p>
				<div class="jump-fields">
					<div class="row">
						<div class="field-host">
							<Input label={t('session.jump_host')} bind:value={jumpHost} placeholder="bastion.example.com" disabled={connecting} />
						</div>
						<div class="field-port">
							<Input label={t('session.port')} bind:value={jumpPortStr} type="number" placeholder="22" disabled={connecting} />
						</div>
					</div>
					<Input label={t('session.username')} bind:value={jumpUsername} placeholder="root" disabled={connecting} />

					<div class="auth-section">
						<span class="auth-label">{t('session.auth_method')}</span>
						<div class="auth-toggle">
							<button type="button" class="auth-btn" class:active={jumpAuthMethod === 'password'} disabled={connecting} onclick={() => (jumpAuthMethod = 'password')}>
								{t('session.auth_password')}
							</button>
							<button type="button" class="auth-btn" class:active={jumpAuthMethod === 'key'} disabled={connecting} onclick={() => (jumpAuthMethod = 'key')}>
								{t('session.auth_key')}
							</button>
						</div>
					</div>

					{#if jumpAuthMethod === 'password'}
						<Input label={t('session.password')} bind:value={jumpPassword} type="password" disabled={connecting} />
					{:else}
						<KeyPathInput label={t('session.key_path')} bind:value={jumpKeyPath} placeholder="~/.ssh/id_rsa" disabled={connecting} />
						<Input label={t('session.passphrase_optional')} bind:value={jumpKeyPassphrase} type="password" disabled={connecting} />
					{/if}
				</div>
			{/if}
		</div>

		<div class="proxy-section">
			<label class="proxy-toggle">
				<input type="checkbox" bind:checked={proxyEnabled} disabled={connecting} />
				<span class="proxy-toggle-text">{t('proxy.connect_via')}</span>
			</label>

			{#if proxyEnabled}
				<div class="proxy-fields">
					<div class="proxy-type-row">
						<button type="button" class="proxy-type-btn" class:active={proxyType === 'socks5'} onclick={() => (proxyType = 'socks5')} disabled={connecting}>SOCKS5</button>
						<button type="button" class="proxy-type-btn" class:active={proxyType === 'socks4'} onclick={() => (proxyType = 'socks4')} disabled={connecting}>SOCKS4</button>
						<button type="button" class="proxy-type-btn" class:active={proxyType === 'http'} onclick={() => (proxyType = 'http')} disabled={connecting}>HTTP</button>
					</div>
					<div class="row">
						<div class="field-host">
							<Input label={t('proxy.host')} bind:value={proxyHost} placeholder="127.0.0.1" disabled={connecting} />
						</div>
						<div class="field-port">
							<Input label={t('session.port')} bind:value={proxyPort} type="number" placeholder="9050" disabled={connecting} />
						</div>
					</div>
					<div class="row">
						<div class="field-host">
							<Input label={t('proxy.username_optional')} bind:value={proxyUsername} disabled={connecting} />
						</div>
						<div class="field-host">
							<Input label={t('session.password_optional')} bind:value={proxyPassword} type="password" disabled={connecting} />
						</div>
					</div>
					<p class="proxy-hint">
						{proxyType === 'socks5' ? t('proxy.hint_socks5') : proxyType === 'http' ? t('proxy.hint_http') : t('proxy.hint_socks4')}
					</p>
				</div>
			{/if}
		</div>
		{/if}
		{/if}

		<ConnectSteps id={attemptId} active={connecting} failed={!connecting && !!error} />

		{#if error}
			<div class="error-message">{error}</div>
		{/if}
	</form>

	{#snippet actions()}
		<Button variant="secondary" onclick={handleClose} disabled={connecting}>
			{t('common.cancel')}
		</Button>
		<Button variant="primary" onclick={handleConnect} disabled={!canConnect}>
			{#if connecting}
				<span class="spinner"></span>
				{t('session.connecting')}
			{:else}
				{t('session.connect')}
			{/if}
		</Button>
	{/snippet}
</Modal>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.row {
		display: flex;
		gap: 10px;
		align-items: flex-start;
	}

	.field-host {
		flex: 1;
		min-width: 0;
	}

	.field-port {
		width: 80px;
		flex-shrink: 0;
	}

	.auth-section {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.auth-label {
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--color-text-secondary);
	}

	.shell-field {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.shell-hint {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.shell-hint:empty {
		display: none;
	}

	.port-row {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.qc-select {
		width: 100%;
		padding: 10px 12px;
		font-family: var(--font-sans);
		font-size: 0.875rem;
		color: var(--color-text-primary);
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		outline: none;
		box-sizing: border-box;
	}

	.qc-select:focus {
		border-color: var(--color-accent);
	}

	.link-btn {
		background: none;
		border: none;
		padding: 0;
		color: var(--color-accent);
		font: inherit;
		cursor: pointer;
	}

	.link-btn:hover {
		text-decoration: underline;
	}

	/* The same segmented control as the session editor's. */
	.auth-toggle {
		display: flex;
		gap: 2px;
		padding: 3px;
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
	}

	.auth-btn {
		flex: 1 1 auto;
		min-width: 0;
		padding: 6px 10px;
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition:
			background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default);
	}

	.auth-btn:hover:not(:disabled):not(.active) {
		color: var(--color-text-primary);
	}

	.auth-btn.active {
		background-color: var(--color-bg-elevated);
		color: var(--color-accent);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25), inset 0 0 0 1px var(--color-border);
	}

	.auth-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.error-message {
		padding: 8px 12px;
		font-size: 0.8125rem;
		color: var(--color-danger);
		background-color: rgba(255, 69, 58, 0.08);
		border: 1px solid rgba(255, 69, 58, 0.2);
		border-radius: var(--radius-btn);
	}

	.spinner {
		display: inline-block;
		width: 14px;
		height: 14px;
		border: 2px solid var(--color-border);
		border-top-color: #fff;
		border-radius: 50%;
		animation: spin 0.6s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	.jump-section {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding-top: 4px;
	}

	.jump-toggle {
		display: flex;
		align-items: center;
		gap: 8px;
		cursor: pointer;
	}

	.jump-toggle input {
		width: 14px;
		height: 14px;
		accent-color: var(--color-accent);
	}

	.jump-toggle-text {
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-text-primary);
	}


	.jump-hint {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.jump-fields {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 10px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		background-color: var(--color-surface-hover);
	}

	.proxy-section {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding-top: 4px;
	}

	.proxy-toggle {
		display: flex;
		align-items: center;
		gap: 8px;
		cursor: pointer;
	}

	.proxy-toggle input[type="checkbox"] {
		width: 14px;
		height: 14px;
		accent-color: var(--color-accent);
		cursor: pointer;
	}

	.proxy-toggle-text {
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-text-primary);
	}

	.proxy-fields {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 10px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		background-color: var(--color-surface-hover);
	}

	.proxy-type-row {
		display: flex;
		gap: 0;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		overflow: hidden;
	}

	.proxy-type-btn {
		flex: 1;
		padding: 5px 8px;
		font-family: var(--font-sans);
		font-size: 0.6875rem;
		font-weight: 500;
		border: none;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default), color var(--duration-default) var(--ease-default);
	}

	.proxy-type-btn.active {
		background-color: var(--color-accent-fill);
		color: #fff;
	}

	.proxy-type-btn:not(.active):hover {
		background-color: var(--color-surface-hover);
	}

	.proxy-hint {
		margin: 0;
		font-size: 0.625rem;
		color: var(--color-text-secondary);
		opacity: 0.7;
	}
</style>
