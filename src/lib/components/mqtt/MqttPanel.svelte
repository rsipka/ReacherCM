<script lang="ts">
	/**
	 * An MQTT tab: a broker explorer. Connects its saved session (asking for
	 * a password or about a certificate when needed), subscribes to its
	 * topic filters and shows every topic that has had a message as a tree,
	 * each with its last message, a short history and a payload view (text,
	 * JSON or hex). Publishes, and subscribes or unsubscribes while open.
	 * The connection lives in the backend; closing the tab ends it.
	 */
	import { onDestroy, onMount, tick, untrack } from 'svelte';
	import type { UnlistenFn } from '@tauri-apps/api/event';
	import { t } from '#lib/state/i18n.svelte.js';
	import { setMqttConnection, setTabLink, type Tab } from '#lib/state/tabs.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import {
		base64ToBytes,
		mqttConnect,
		mqttDisconnect,
		mqttPublish,
		mqttSubscribe,
		mqttTrustCert,
		mqttUnsubscribe,
		onMqttEvent,
		MQTT_LOGIN_REFUSED,
		MQTT_PASSWORD_NEEDED,
		MQTT_UNTRUSTED_CERT,
		type MqttMessage
	} from '#lib/ipc/mqtt.js';
	import { TopicTree, visibleRows, validFilter, validTopic, type Row } from '#lib/mqtt/topics.js';
	import { encode, preview, render, size, type PayloadView } from '#lib/mqtt/payload.js';
	import Button from '#lib/components/shared/Button.svelte';
	import VirtualList from '#lib/components/shared/VirtualList.svelte';

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

	let stage = $state<Stage>({ kind: 'connecting' });
	let password = $state('');
	let passwordEl: HTMLInputElement | undefined = $state();
	let seenReconnect = untrack(() => tab.reconnect);
	let unlisten: UnlistenFn | undefined;

	let where = $derived(tab.mqttConnectParams ? `${tab.mqttConnectParams.username ? tab.mqttConnectParams.username + '@' : ''}${tab.mqttConnectParams.host}:${tab.mqttConnectParams.port}` : '');

	// The tree is not reactive; `version` redraws it.
	const tree = new TopicTree();
	let version = $state(0);
	let paused = $state(false);
	let clientId = $state('');
	let filters = $state<string[]>([]);
	let newFilter = $state('');
	let search = $state('');
	let open = $state(new Set<string>());
	let selected = $state<string | null>(null);
	/** A message picked from the history; the last one when unset. */
	let picked = $state<MqttMessage | null>(null);
	let view = $state<PayloadView>('auto');
	let received = $state(0);

	let pubTopic = $state('');
	let pubPayload = $state('');
	let pubFormat = $state<'text' | 'hex'>('text');
	let pubQos = $state(0);
	let pubRetain = $state(false);
	let publishing = $state(false);

	let rows = $derived.by<Row[]>(() => {
		void version;
		return visibleRows(tree, open, search);
	});
	let node = $derived.by(() => {
		void version;
		return selected ? tree.find(selected) : undefined;
	});
	let shown = $derived(picked ?? node?.last ?? null);
	let shownBytes = $derived(shown ? base64ToBytes(shown.payload) : null);
	let rendered = $derived(shownBytes ? render(shownBytes, view) : null);

	function onEvent(id: string, e: Parameters<Parameters<typeof onMqttEvent>[1]>[0]): void {
		if (tab.mqttConnectionId !== id) return;
		if (e.type === 'messages') {
			received += e.messages.length;
			if (paused) return;
			tree.add(e.messages);
			version = tree.version;
		} else {
			setMqttConnection(tab.id, undefined);
			setTabLink(tab.id, 'down');
			stage = { kind: 'error', message: e.error ?? t('mqtt.closed') };
		}
	}

	async function connect(typed?: string): Promise<void> {
		const params = tab.mqttConnectParams;
		if (!params) return;
		stage = { kind: 'connecting' };
		setTabLink(tab.id, 'reconnecting');
		const old = tab.mqttConnectionId;
		if (old) {
			setMqttConnection(tab.id, undefined);
			await mqttDisconnect(old).catch(() => {});
		}
		unlisten?.();
		const id = crypto.randomUUID();
		// Hear the connection before it starts: retained messages come at once.
		unlisten = await onMqttEvent(id, (e) => onEvent(id, e));
		setMqttConnection(tab.id, id);
		try {
			const opened = await mqttConnect(id, params.sessionId, typed);
			clientId = opened.clientId;
			filters = opened.filters;
			setTabLink(tab.id, undefined);
			password = '';
			stage = { kind: 'up' };
		} catch (err) {
			setMqttConnection(tab.id, undefined);
			setTabLink(tab.id, 'down');
			const msg = String(err);
			if (msg === MQTT_PASSWORD_NEEDED) {
				stage = { kind: 'password' };
				await tick();
				passwordEl?.focus();
			} else if (msg.startsWith(MQTT_LOGIN_REFUSED)) {
				stage = { kind: 'password', error: msg.slice(MQTT_LOGIN_REFUSED.length) };
				await tick();
				passwordEl?.focus();
			} else if (msg.startsWith(MQTT_UNTRUSTED_CERT)) {
				stage = { kind: 'cert', fingerprint: msg.slice(MQTT_UNTRUSTED_CERT.length) };
			} else {
				stage = { kind: 'error', message: msg };
			}
		}
	}

	async function trust(fingerprint: string): Promise<void> {
		const params = tab.mqttConnectParams;
		if (!params) return;
		try {
			await mqttTrustCert(params.sessionId, fingerprint);
			await connect();
		} catch (err) {
			stage = { kind: 'error', message: String(err) };
		}
	}

	async function subscribe(): Promise<void> {
		const f = newFilter.trim();
		const id = tab.mqttConnectionId;
		if (!f || !id) return;
		if (!validFilter(f)) {
			addToast(t('mqtt.filter_invalid'), 'error');
			return;
		}
		if (filters.includes(f)) {
			newFilter = '';
			return;
		}
		try {
			const [granted] = await mqttSubscribe(id, [f]);
			if (granted === 128) {
				addToast(t('mqtt.subscribe_refused', { filter: f }), 'error');
				return;
			}
			filters = [...filters, f];
			newFilter = '';
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	async function unsubscribe(f: string): Promise<void> {
		const id = tab.mqttConnectionId;
		if (!id) return;
		try {
			await mqttUnsubscribe(id, [f]);
			filters = filters.filter((x) => x !== f);
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	async function publish(): Promise<void> {
		const id = tab.mqttConnectionId;
		const topic = pubTopic.trim();
		if (!id || !topic) return;
		if (!validTopic(topic)) {
			addToast(t('mqtt.topic_invalid'), 'error');
			return;
		}
		let bytes: Uint8Array;
		try {
			bytes = encode(pubPayload, pubFormat);
		} catch {
			addToast(t('mqtt.hex_invalid'), 'error');
			return;
		}
		publishing = true;
		try {
			await mqttPublish(id, topic, bytes, pubQos, pubRetain);
			addToast(t('mqtt.published', { topic }), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		} finally {
			publishing = false;
		}
	}

	/** An empty retained message removes a topic's retained one. */
	async function clearRetained(topic: string): Promise<void> {
		const id = tab.mqttConnectionId;
		if (!id) return;
		try {
			await mqttPublish(id, topic, new Uint8Array(), 1, true);
			addToast(t('mqtt.retained_cleared', { topic }), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	function select(row: Row): void {
		selected = row.node.path;
		picked = null;
		if (row.parent) toggle(row.node.path);
	}

	function toggle(path: string): void {
		const next = new Set(open);
		if (next.has(path)) next.delete(path);
		else next.add(path);
		open = next;
	}

	function useInPublish(m: MqttMessage | null, topic: string): void {
		pubTopic = topic;
		if (m) {
			const r = render(base64ToBytes(m.payload), 'auto');
			pubFormat = r.view === 'hex' ? 'hex' : 'text';
			pubPayload = r.view === 'hex' ? [...base64ToBytes(m.payload)].map((b) => b.toString(16).padStart(2, '0')).join(' ') : new TextDecoder().decode(base64ToBytes(m.payload));
			pubRetain = m.retain;
			pubQos = Math.min(m.qos, 2);
		}
	}

	async function copy(text: string): Promise<void> {
		try {
			await navigator.clipboard.writeText(text);
			addToast(t('mqtt.copied'), 'success');
		} catch {
			// No clipboard here.
		}
	}

	function clearTree(): void {
		tree.clear();
		version = tree.version;
		received = 0;
		picked = null;
	}

	function time(ms: number): string {
		return new Date(ms).toLocaleTimeString();
	}

	onMount(() => {
		void connect();
	});

	onDestroy(() => {
		unlisten?.();
	});

	// The tab menu's Reconnect.
	$effect(() => {
		const bump = tab.reconnect;
		if (bump === seenReconnect) return;
		seenReconnect = bump;
		void connect();
	});
</script>

{#if stage.kind === 'up'}
	<div class="mqtt" class:inactive={!active}>
		<div class="bar">
			<span class="dot" title={t('mqtt.connected')}></span>
			<span class="where" title={clientId}>{where}</span>
			<div class="filters">
				{#each filters as f (f)}
					<span class="chip">
						<code>{f}</code>
						<button type="button" title={t('mqtt.unsubscribe')} aria-label={t('mqtt.unsubscribe')} onclick={() => void unsubscribe(f)}>×</button>
					</span>
				{/each}
				<form
					class="add-filter"
					onsubmit={(e) => {
						e.preventDefault();
						void subscribe();
					}}
				>
					<input bind:value={newFilter} placeholder={t('mqtt.subscribe_placeholder')} aria-label={t('mqtt.subscribe')} spellcheck="false" autocapitalize="off" />
					<Button type="submit" size="sm" variant="secondary" disabled={!newFilter.trim()}>{t('mqtt.subscribe')}</Button>
				</form>
			</div>
			<span class="count" title={t('mqtt.received_hint')}>{received.toLocaleString()}</span>
			<Button size="sm" variant="ghost" onclick={() => (paused = !paused)}>{paused ? t('mqtt.resume') : t('mqtt.pause')}</Button>
			<Button size="sm" variant="ghost" onclick={clearTree}>{t('mqtt.clear')}</Button>
		</div>

		<div class="body">
			<div class="tree">
				<input class="search" bind:value={search} placeholder={t('mqtt.search')} spellcheck="false" autocapitalize="off" />
				{#if rows.length === 0}
					<p class="empty">{search ? t('mqtt.no_match') : t('mqtt.waiting')}</p>
				{:else}
					<div class="rows">
						<VirtualList items={rows} itemHeight={26}>
							{#snippet children(row: Row)}
								<button type="button" class="row" class:sel={row.node.path === selected} style="padding-left: {8 + row.depth * 14}px" onclick={() => select(row)} title={row.node.path}>
									<span class="caret">{row.parent ? (row.open ? '▾' : '▸') : ''}</span>
									<span class="name">{row.node.name === '' ? '∅' : row.node.name}</span>
									{#if row.node.last}
										{#if row.node.last.retain}<span class="ret" title={t('mqtt.retained')}>R</span>{/if}
										<span class="val">{preview(base64ToBytes(row.node.last.payload), 40)}</span>
									{/if}
									<span class="num" title={t('mqtt.count_hint')}>{row.parent ? `${row.node.topics}·${row.node.total}` : row.node.count}</span>
								</button>
							{/snippet}
						</VirtualList>
					</div>
				{/if}
				{#if tree.dropped > 0}
					<p class="note small">{t('mqtt.tree_full')}</p>
				{/if}
			</div>

			<div class="side">
				{#if selected}
					<div class="topic-head">
						<code class="topic">{selected}</code>
						<Button size="sm" variant="ghost" onclick={() => void copy(selected!)}>{t('mqtt.copy_topic')}</Button>
					</div>
					{#if shown && shownBytes && rendered}
						<div class="meta">
							<span>{time(shown.time)}</span>
							<span>QoS {shown.qos}</span>
							{#if shown.retain}<span class="ret">{t('mqtt.retained')}</span>{/if}
							<span>{size(shown.size)}{shown.size > shownBytes.length ? ` (${t('mqtt.truncated')})` : ''}</span>
							<span class="views">
								{#each ['auto', 'text', 'json', 'hex'] as v (v)}
									<button type="button" class:on={view === v} onclick={() => (view = v as PayloadView)}>{t(`mqtt.view_${v}`)}</button>
								{/each}
							</span>
						</div>
						<pre class="payload">{rendered.body || t('mqtt.empty_payload')}</pre>
						<div class="actions-row">
							<Button size="sm" variant="secondary" onclick={() => void copy(rendered!.body)}>{t('mqtt.copy_payload')}</Button>
							<Button size="sm" variant="secondary" onclick={() => useInPublish(shown, selected!)}>{t('mqtt.use_in_publish')}</Button>
							{#if shown.retain && shown.size > 0}
								<Button size="sm" variant="ghost" onclick={() => void clearRetained(selected!)}>{t('mqtt.clear_retained')}</Button>
							{/if}
						</div>
						{#if node && node.history.length > 1}
							<div class="history">
								<div class="label">{t('mqtt.history', { count: node.count })}</div>
								{#each node.history as m, i (i)}
									<button type="button" class="hist" class:sel={m === shown} onclick={() => (picked = m)}>
										<span class="t">{time(m.time)}</span>
										<span class="v">{preview(base64ToBytes(m.payload), 60)}</span>
									</button>
								{/each}
							</div>
						{/if}
					{:else}
						<p class="note">{t('mqtt.no_message')}</p>
						<div class="actions-row">
							<Button size="sm" variant="secondary" onclick={() => useInPublish(null, selected!)}>{t('mqtt.use_in_publish')}</Button>
						</div>
					{/if}
				{:else}
					<p class="note">{t('mqtt.pick_topic')}</p>
				{/if}

				<form
					class="publish"
					onsubmit={(e) => {
						e.preventDefault();
						void publish();
					}}
				>
					<div class="label">{t('mqtt.publish')}</div>
					<input bind:value={pubTopic} placeholder={t('mqtt.topic')} aria-label={t('mqtt.topic')} spellcheck="false" autocapitalize="off" />
					<textarea bind:value={pubPayload} rows="4" placeholder={pubFormat === 'hex' ? '7b 22 6f 6e 22 7d' : '{"on": true}'} aria-label={t('mqtt.payload')} spellcheck="false"></textarea>
					<div class="pub-opts">
						<select bind:value={pubFormat} aria-label={t('mqtt.payload_format')}>
							<option value="text">{t('mqtt.format_text')}</option>
							<option value="hex">{t('mqtt.format_hex')}</option>
						</select>
						<select bind:value={pubQos} aria-label="QoS">
							<option value={0}>QoS 0</option>
							<option value={1}>QoS 1</option>
							<option value={2}>QoS 2</option>
						</select>
						<label class="check"><input type="checkbox" bind:checked={pubRetain} /> {t('mqtt.retain')}</label>
						<span class="grow"></span>
						<Button type="submit" size="sm" disabled={publishing || !pubTopic.trim()}>{t('mqtt.publish_button')}</Button>
					</div>
				</form>
			</div>
		</div>
	</div>
{:else}
	<div class="connect">
		<div class="card">
			<div class="title">{tab.sessionName ?? tab.title}</div>
			<div class="where">{where}</div>
			{#if stage.kind === 'connecting'}
				<p class="note">{t('mqtt.connecting')}</p>
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
						<Button type="submit" disabled={!password}>{t('mqtt.connect')}</Button>
					</div>
				</form>
			{:else if stage.kind === 'cert'}
				<p class="note">{t('mqtt.cert_untrusted')}</p>
				<code class="fp">{stage.fingerprint}</code>
				<p class="note small">{t('ftp.cert_hint')}</p>
				<div class="actions">
					<Button variant="secondary" onclick={() => (stage = { kind: 'error', message: t('ftp.cert_refused') })}>{t('common.cancel')}</Button>
					<Button onclick={() => void trust((stage as { fingerprint: string }).fingerprint)}>{t('ftp.cert_trust')}</Button>
				</div>
			{:else if stage.kind === 'error'}
				<p class="note error">{stage.message}</p>
				<div class="actions">
					<Button onclick={() => void connect()}>{t('ftp.retry')}</Button>
				</div>
			{/if}
		</div>
	</div>
{/if}

<style>
	.mqtt {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.bar {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 10px;
		border-bottom: 1px solid var(--color-border);
		flex-wrap: wrap;
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--color-success, #3fb950);
		flex: none;
	}
	.bar .where {
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.filters {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1;
		flex-wrap: wrap;
		min-width: 0;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 2px 4px 2px 8px;
		border-radius: 10px;
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		font-size: 12px;
	}
	.chip button {
		border: none;
		background: none;
		color: var(--color-text-tertiary);
		cursor: pointer;
		font-size: 14px;
		line-height: 1;
		padding: 0 4px;
	}
	.chip button:hover {
		color: var(--color-danger);
	}
	.add-filter {
		display: flex;
		gap: 4px;
	}
	input,
	textarea,
	select {
		padding: 5px 8px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 12px;
	}
	.add-filter input {
		width: 160px;
		font-family: var(--font-mono, monospace);
	}
	.count {
		font-size: 12px;
		color: var(--color-text-tertiary);
		font-variant-numeric: tabular-nums;
	}
	.body {
		flex: 1;
		display: flex;
		min-height: 0;
	}
	.tree {
		width: 45%;
		min-width: 240px;
		display: flex;
		flex-direction: column;
		border-right: 1px solid var(--color-border);
		min-height: 0;
	}
	.search {
		margin: 8px;
	}
	.rows {
		flex: 1;
		min-height: 0;
		position: relative;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 6px;
		width: 100%;
		height: 26px;
		border: none;
		background: none;
		color: inherit;
		text-align: left;
		cursor: pointer;
		padding-right: 8px;
		font-size: 12px;
		white-space: nowrap;
	}
	.row:hover {
		background: var(--color-bg-hover, var(--color-bg-elevated));
	}
	.row.sel {
		background: var(--color-accent-subtle, var(--color-bg-elevated));
	}
	.caret {
		width: 10px;
		color: var(--color-text-tertiary);
		flex: none;
	}
	.name {
		font-weight: 500;
		flex: none;
		max-width: 50%;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.val {
		color: var(--color-text-secondary);
		overflow: hidden;
		text-overflow: ellipsis;
		flex: 1;
		min-width: 0;
		font-family: var(--font-mono, monospace);
		font-size: 11px;
	}
	.num {
		margin-left: auto;
		color: var(--color-text-tertiary);
		font-size: 11px;
		font-variant-numeric: tabular-nums;
		flex: none;
	}
	.ret {
		font-size: 10px;
		padding: 0 4px;
		border-radius: 4px;
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		color: var(--color-text-secondary);
		flex: none;
	}
	.empty {
		padding: 16px;
		color: var(--color-text-tertiary);
		font-size: 12px;
	}
	.side {
		flex: 1;
		min-width: 0;
		overflow: auto;
		padding: 10px 12px;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.topic-head {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.topic {
		font-size: 13px;
		word-break: break-all;
		flex: 1;
	}
	.meta {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.views {
		margin-left: auto;
		display: inline-flex;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		overflow: hidden;
	}
	.views button {
		border: none;
		background: none;
		color: var(--color-text-secondary);
		padding: 2px 8px;
		font-size: 11px;
		cursor: pointer;
	}
	.views button.on {
		background: var(--color-bg-elevated);
		color: var(--color-text-primary);
	}
	.payload {
		margin: 0;
		padding: 8px;
		border-radius: 6px;
		background: var(--color-surface-sunken, var(--color-bg-elevated));
		font-size: 12px;
		max-height: 40vh;
		overflow: auto;
		white-space: pre-wrap;
		word-break: break-all;
	}
	.actions-row {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
	}
	.history {
		display: flex;
		flex-direction: column;
	}
	.label {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--color-text-tertiary);
		margin-bottom: 4px;
	}
	.hist {
		display: flex;
		gap: 10px;
		border: none;
		background: none;
		color: inherit;
		text-align: left;
		padding: 3px 4px;
		font-size: 12px;
		cursor: pointer;
		border-radius: 4px;
	}
	.hist:hover,
	.hist.sel {
		background: var(--color-bg-elevated);
	}
	.hist .t {
		color: var(--color-text-tertiary);
		flex: none;
		font-variant-numeric: tabular-nums;
	}
	.hist .v {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: var(--font-mono, monospace);
		font-size: 11px;
	}
	.publish {
		margin-top: auto;
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding-top: 10px;
		border-top: 1px solid var(--color-border);
	}
	.publish input,
	.publish textarea {
		font-family: var(--font-mono, monospace);
	}
	.pub-opts {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.check {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		font-size: 12px;
	}
	.grow {
		flex: 1;
	}
	/* A phone or a narrow window: the tree above, the topic below. */
	@media (max-width: 720px) {
		.body {
			flex-direction: column;
		}
		.tree {
			width: auto;
			min-width: 0;
			height: 45%;
			border-right: none;
			border-bottom: 1px solid var(--color-border);
		}
		.add-filter input {
			width: 120px;
		}
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
	.card .where {
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
		padding: 0 8px 8px;
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
	.add-filter {
		flex-direction: row;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}
</style>
