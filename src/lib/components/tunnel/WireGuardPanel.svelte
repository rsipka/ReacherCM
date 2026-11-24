<script lang="ts">
	/**
	 * WireGuard profiles, at the top of the Tunnels panel: import a wg-quick
	 * file, turn a tunnel on or off, see whether its server answers. A
	 * session picks a profile as its route (session editor), and connecting
	 * turns the tunnel on after asking.
	 */
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { wireguardCheck, wireguardDelete, wireguardDown, wireguardGenerateKey, wireguardGet, wireguardSave, wireguardUp, type WgProfileInfo } from '#lib/ipc/wireguard.js';
	import { refreshWireguard, wireguardProfiles } from '#lib/state/wireguard.svelte.js';
	import { ask } from '#lib/state/ask.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import Input from '#lib/components/shared/Input.svelte';
	import Button from '#lib/components/shared/Button.svelte';

	const list = $derived(wireguardProfiles());
	const busy = new SvelteSet<string>();

	let editing = $state<{ id: string | null; name: string; conf: string } | null>(null);
	let formError = $state('');
	let saving = $state(false);
	let fileInput = $state<HTMLInputElement | undefined>();

	onMount(() => {
		void refreshWireguard();
		// Handshakes and byte counts move while a tunnel is on.
		const timer = window.setInterval(() => {
			if (list.some((p) => p.state.running)) void refreshWireguard();
		}, 3000);
		return () => window.clearInterval(timer);
	});

	function template(privateKey = ''): string {
		return `[Interface]\nPrivateKey = ${privateKey}\nAddress = 10.0.0.2/32\nDNS = 10.0.0.1\n\n[Peer]\nPublicKey = \nEndpoint = vpn.example.com:51820\nAllowedIPs = 10.0.0.0/24\nPersistentKeepalive = 25\n`;
	}

	function startNew(): void {
		formError = '';
		editing = { id: null, name: '', conf: '' };
	}

	async function startEdit(p: WgProfileInfo): Promise<void> {
		formError = '';
		try {
			const full = await wireguardGet(p.id);
			editing = { id: full.id, name: full.name, conf: full.conf };
		} catch (e) {
			addToast(String(e), 'error');
		}
	}

	async function newKeys(): Promise<void> {
		if (!editing) return;
		const [priv, pub] = await wireguardGenerateKey();
		editing.conf = editing.conf.trim() ? editing.conf.replace(/^(\s*PrivateKey\s*=).*$/im, `$1 ${priv}`) : template(priv);
		if (!/PrivateKey\s*=/i.test(editing.conf)) editing.conf = template(priv);
		addToast(t('wireguard.new_key_toast', { key: pub }), 'info');
	}

	async function pickFile(e: Event): Promise<void> {
		const file = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!file || !editing) return;
		editing.conf = await file.text();
		if (!editing.name.trim()) editing.name = file.name.replace(/\.conf$/i, '');
		(e.currentTarget as HTMLInputElement).value = '';
		formError = '';
	}

	async function save(): Promise<void> {
		if (!editing || saving) return;
		formError = '';
		saving = true;
		try {
			await wireguardCheck(editing.conf);
			await wireguardSave(editing.id, editing.name.trim() || t('wireguard.default_name'), editing.conf);
			editing = null;
			await refreshWireguard();
		} catch (e) {
			formError = String(e);
		} finally {
			saving = false;
		}
	}

	async function toggle(p: WgProfileInfo): Promise<void> {
		if (busy.has(p.id)) return;
		busy.add(p.id);
		try {
			if (p.state.running) await wireguardDown(p.id);
			else await wireguardUp(p.id);
		} catch (e) {
			addToast(String(e), 'error');
		} finally {
			busy.delete(p.id);
			await refreshWireguard();
		}
	}

	async function remove(p: WgProfileInfo): Promise<void> {
		if (!(await ask(t('wireguard.delete_confirm', { name: p.name }), { confirmLabel: t('common.delete') }))) return;
		try {
			await wireguardDelete(p.id);
		} catch (e) {
			addToast(String(e), 'error');
		}
		await refreshWireguard();
	}

	function bytes(n: number): string {
		if (n < 1024) return `${n} B`;
		if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
		return `${(n / 1024 / 1024).toFixed(1)} MB`;
	}

	function status(p: WgProfileInfo): string {
		if (p.invalid) return p.invalid;
		if (!p.state.running) return t('wireguard.off');
		if (p.state.error) return p.state.error;
		const h = p.state.handshake_secs;
		return t('wireguard.on_status', { ago: h == null ? '—' : `${h}s`, rx: bytes(p.state.rx_bytes), tx: bytes(p.state.tx_bytes) });
	}
</script>

<section class="wg">
	<div class="head">
		<span class="title">WireGuard</span>
		<button type="button" class="link" onclick={startNew} disabled={!!editing}>+ {t('wireguard.add')}</button>
	</div>

	{#if editing}
		<div class="form">
			<Input label={t('wireguard.name')} bind:value={editing.name} placeholder={t('wireguard.default_name')} disabled={saving} />
			<label class="conf">
				<span>{t('wireguard.conf')}</span>
				<textarea bind:value={editing.conf} rows="9" spellcheck="false" autocomplete="off" placeholder={template('…')} disabled={saving}></textarea>
			</label>
			<p class="hint">{t('wireguard.conf_hint')}</p>
			<div class="row">
				<button type="button" class="link" onclick={() => fileInput?.click()} disabled={saving}>{t('wireguard.import_file')}</button>
				<button type="button" class="link" onclick={() => void newKeys()} disabled={saving}>{t('wireguard.generate_key')}</button>
				<input bind:this={fileInput} type="file" accept=".conf,text/plain" hidden onchange={pickFile} />
			</div>
			{#if formError}<p class="error" role="alert">{formError}</p>{/if}
			<div class="actions">
				<Button variant="ghost" size="sm" onclick={() => (editing = null)} disabled={saving}>{t('common.cancel')}</Button>
				<Button variant="primary" size="sm" onclick={() => void save()} disabled={saving || !editing.conf.trim()}>{t('common.save')}</Button>
			</div>
		</div>
	{/if}

	{#if list.length === 0 && !editing}
		<p class="empty">{t('wireguard.empty')}</p>
	{/if}
	{#each list as p (p.id)}
		<div class="card">
			<span class="dot" class:on={p.state.running && !p.state.error} class:bad={!!p.invalid || !!p.state.error} aria-hidden="true"></span>
			<div class="info">
				<span class="name">{p.name}</span>
				<span class="sub" title={p.allowed_ips.join(', ')}>{p.endpoint ?? '—'} · {p.addresses.join(', ')}</span>
				<span class="sub" role="status">{status(p)}</span>
			</div>
			<div class="btns">
				<button type="button" class="btn" onclick={() => void toggle(p)} disabled={busy.has(p.id) || !!p.invalid}>
					{busy.has(p.id) ? t('session.connecting') : p.state.running ? t('wireguard.turn_off') : t('wireguard.turn_on')}
				</button>
				<button type="button" class="btn" onclick={() => void startEdit(p)} disabled={!!editing}>{t('common.edit')}</button>
				<button type="button" class="btn danger" onclick={() => void remove(p)} aria-label={t('common.delete')} title={t('common.delete')}>✕</button>
			</div>
		</div>
	{/each}
</section>

<style>
	.wg {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding-bottom: 8px;
		border-bottom: 1px solid var(--color-border);
	}

	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.title {
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.link {
		padding: 2px 0;
		border: none;
		background: none;
		font: inherit;
		font-size: 0.75rem;
		color: var(--color-accent);
		cursor: pointer;
	}

	.link:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.form {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 10px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card, 8px);
	}

	.conf {
		display: grid;
		gap: 4px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	textarea {
		width: 100%;
		min-width: 0;
		box-sizing: border-box;
		padding: 6px 8px;
		font-family: var(--font-mono);
		font-size: 0.75rem;
		color: var(--color-text-primary);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		resize: vertical;
	}

	.hint,
	.empty {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.empty {
		opacity: 0.8;
	}

	.error {
		margin: 0;
		font-size: 0.75rem;
		color: var(--color-danger);
	}

	.row,
	.actions {
		display: flex;
		gap: 12px;
		flex-wrap: wrap;
	}

	.actions {
		justify-content: flex-end;
		gap: 6px;
	}

	.card {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		padding: 6px 4px;
		border-radius: 6px;
	}

	.card:hover {
		background: var(--color-surface-hover);
	}

	.dot {
		flex-shrink: 0;
		width: 8px;
		height: 8px;
		margin-top: 5px;
		border-radius: 50%;
		background: var(--color-text-secondary);
		opacity: 0.5;
	}

	.dot.on {
		background: var(--color-success);
		opacity: 1;
	}

	.dot.bad {
		background: var(--color-danger);
		opacity: 1;
	}

	.info {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1;
	}

	.name {
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.sub {
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
		overflow-wrap: anywhere;
	}

	.btns {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: 4px;
	}

	.btn {
		padding: 3px 8px;
		font: inherit;
		font-size: 0.6875rem;
		color: var(--color-text-primary);
		background: transparent;
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	.btn:hover:not(:disabled) {
		background: var(--color-surface-active);
	}

	.btn:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.btn.danger:hover:not(:disabled) {
		color: var(--color-danger);
	}
</style>
