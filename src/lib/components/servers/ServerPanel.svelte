<script lang="ts">
	import { onMount } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import { downloadDir } from '@tauri-apps/api/path';
	import * as settings from '#lib/ipc/settings.js';
	import {
		SERVER_KINDS,
		DEFAULT_PORTS,
		generatePassword,
		localServerInterfaces,
		localServerStatus,
		type LogEntry,
		type NetInterface,
		type ServerConfig,
		type ServerKind,
		type ServerStatus
	} from '#lib/ipc/localServers.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import ServerCard from './ServerCard.svelte';

	/** The settings key the four servers' settings are kept under (in the vault). */
	const SETTINGS_KEY = 'local_servers';
	const LOG_LINES = 500;

	let configs = $state<Record<ServerKind, ServerConfig> | null>(null);
	let statuses = $state<Partial<Record<ServerKind, ServerStatus>>>({});
	let interfaces = $state<NetInterface[]>([]);
	let open = $state<ServerKind | null>(null);

	function defaultBind(list: NetInterface[]): string {
		return list.find((i) => i.private && !i.loopback)?.ip ?? '127.0.0.1';
	}

	function defaults(kind: ServerKind, root: string, bind: string): ServerConfig {
		return {
			kind,
			root,
			bind,
			port: DEFAULT_PORTS[kind],
			users: kind === 'tftp' ? [] : [{ username: 'reacher', password: generatePassword() }],
			anonymous: false,
			allowWrite: kind === 'tftp'
		};
	}

	async function load(): Promise<void> {
		interfaces = await localServerInterfaces().catch(() => []);
		const bind = defaultBind(interfaces);
		const root = await downloadDir().catch(() => '');
		let saved: Partial<Record<ServerKind, Partial<ServerConfig>>> = {};
		try {
			const text = await settings.get(SETTINGS_KEY);
			if (text) saved = JSON.parse(text);
		} catch {
			// Nothing saved, or the vault is locked: start from the defaults.
		}
		const known = new Set(['0.0.0.0', '127.0.0.1', ...interfaces.map((i) => i.ip)]);
		const next = {} as Record<ServerKind, ServerConfig>;
		for (const kind of SERVER_KINDS) {
			const cfg = { ...defaults(kind, root, bind), ...(saved[kind] ?? {}), kind };
			// An address of a network this computer has left (another Wi-Fi):
			// the default one of the network it is on now.
			if (!known.has(cfg.bind)) cfg.bind = bind;
			next[kind] = cfg;
		}
		configs = next;
		await refresh();
	}

	async function refresh(): Promise<void> {
		try {
			const list = await localServerStatus();
			statuses = Object.fromEntries(list.map((s) => [s.kind, s]));
		} catch (err) {
			console.error('Local servers:', err);
		}
	}

	let saveTimer: number | undefined;
	function save(): void {
		window.clearTimeout(saveTimer);
		saveTimer = window.setTimeout(() => {
			if (configs) void settings.set(SETTINGS_KEY, JSON.stringify(configs)).catch(() => {});
		}, 400);
	}

	function onStatus(status: ServerStatus): void {
		statuses = { ...statuses, [status.kind]: status };
	}

	onMount(() => {
		void load();
		const unlisten = listen<LogEntry>('local-server-log', (event) => {
			const entry = event.payload;
			const current = statuses[entry.kind];
			if (!current) return;
			const log = [...current.log, entry].slice(-LOG_LINES);
			statuses = { ...statuses, [entry.kind]: { ...current, log } };
		});
		// A server whose socket failed stops on its own; this notices.
		const timer = window.setInterval(() => {
			if (Object.values(statuses).some((s) => s?.running)) void refresh();
		}, 5000);
		return () => {
			void unlisten.then((fn) => fn());
			window.clearInterval(timer);
			window.clearTimeout(saveTimer);
			if (configs) void settings.set(SETTINGS_KEY, JSON.stringify(configs)).catch(() => {});
		};
	});
</script>

<div class="server-panel">
	<p class="intro">{t('servers.intro')}</p>
	{#if configs}
		{#each SERVER_KINDS as kind (kind)}
			<ServerCard
				bind:config={configs[kind]}
				status={statuses[kind]}
				{interfaces}
				expanded={open === kind}
				ontoggle={() => (open = open === kind ? null : kind)}
				onchange={save}
				onstatus={onStatus}
			/>
		{/each}
	{/if}
</div>

<style>
	.server-panel {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: 4px 0 8px;
	}

	.intro {
		margin: 0 2px 2px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		line-height: 1.4;
	}
</style>
