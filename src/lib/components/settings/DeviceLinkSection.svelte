<script lang="ts">
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import { linkDevice } from '#lib/state/vault.svelte.js';
	import { exportDeviceLink, qrSvg } from '#lib/ipc/vault.js';
	import { canScanLinkCode, scanLinkCode } from '#lib/state/link-scan.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';

	/** Link another device to this one's vaults, or this one to another's.
	 *  Works the same whichever Sync location is in use. */
	let { onJoined }: { onJoined?: () => void } = $props();

	let error = $state('');
	let linkCode = $state('');
	let linkQr = $state('');
	let joinCode = $state('');
	let joinArmed = $state(false);
	let joining = $state(false);
	let accountLink = $derived(joinCode.trim().startsWith('reachercm-account-link1:'));

	async function handleLinkDevice() {
		error = '';
		try {
			linkCode = await exportDeviceLink();
			linkQr = await qrSvg(linkCode).catch(() => '');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function handleScan() {
		error = '';
		try {
			const code = await scanLinkCode();
			if (code) {
				joinCode = code;
				joinArmed = false;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function handleJoin() {
		if (!joinArmed) {
			joinArmed = true;
			return;
		}
		joining = true;
		error = '';
		try {
			await linkDevice(joinCode.trim());
			joinCode = '';
			joinArmed = false;
			addToast(t('sync.joined_toast'), 'success');
			onJoined?.();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			joining = false;
		}
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
		addToast(t('sync.copied_toast'), 'success');
	}
</script>

<div class="section device">
	<h3 class="section-title">{t('sync.link_device')}</h3>
	<p class="section-desc">{t('sync.link_device_desc')}</p>
	<div class="action-row">
		<Button variant="secondary" size="sm" onclick={handleLinkDevice}>
			{t('sync.show_link_code')}
		</Button>
	</div>
	{#if linkCode}
		<div class="export-box">
			<p class="export-warning">{t('sync.link_code_warning')}</p>
			{#if linkQr}
				<p class="section-desc">{t('sync.link_qr_hint')}</p>
				<!-- SVG drawn by the backend from the code -->
				<div class="link-qr">{@html linkQr}</div>
			{/if}
			<div class="export-key">
				<code>{linkCode}</code>
				<button class="copy-btn" onclick={() => copyToClipboard(linkCode)}>{t('sync.copy')}</button>
			</div>
			<Button variant="ghost" size="sm" onclick={() => { linkCode = ''; linkQr = ''; }}>
				{t('common.close')}
			</Button>
		</div>
	{/if}

	<h3 class="section-title">{t('sync.join_device')}</h3>
	<p class="section-desc">{t('sync.join_device_desc')}</p>
	<div class="form-field">
		<Input
			label={t('sync.link_code')}
			type="password"
			placeholder="reachercm-link1:..."
			bind:value={joinCode}
			disabled={joining}
			oninput={() => (joinArmed = false)}
		/>
	</div>
	{#if joinArmed}
		<p class="export-warning">{t(accountLink ? 'sync_account.vault_link_confirm' : 'sync.join_confirm')}</p>
	{/if}
	<div class="action-row">
		{#if canScanLinkCode()}
			<Button variant="secondary" size="sm" onclick={handleScan} disabled={joining}>
				{t('sync.scan_qr')}
			</Button>
		{/if}
		<Button variant={joinArmed ? 'danger' : 'secondary'} size="sm" onclick={handleJoin} disabled={joining || !joinCode.trim()}>
			{joining ? t('sync.joining') : joinArmed ? t('sync.join_confirm_button') : t('sync.join')}
		</Button>
	</div>
	{#if error}
		<div class="form-error">{error}</div>
	{/if}
</div>

<style>
	.device { padding: 12px 0; }
	.section-desc { margin: 0; font-size: 0.75rem; color: var(--color-text-secondary); }
	.form-field { width: 100%; }
	.form-error { font-size: 0.75rem; color: var(--color-danger); padding: 4px 0; }
	.copy-btn {
		padding: 4px 8px;
		font-size: 0.6875rem;
		font-weight: 500;
		color: var(--color-accent);
		background: transparent;
		border: 1px solid var(--color-accent);
		border-radius: 4px;
		cursor: pointer;
	}
	.copy-btn:hover { background: color-mix(in srgb, var(--color-accent) 10%, transparent); }
	.link-qr {
		align-self: center;
		width: min(320px, 100%);
		padding: 8px;
		background: #ffffff;
		border-radius: 8px;
	}
	.link-qr :global(svg) { display: block; width: 100%; height: auto; }
	.export-box {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 12px;
		background: rgba(255, 214, 10, 0.08);
		border: 1px solid rgba(255, 214, 10, 0.3);
		border-radius: 8px;
	}
	.export-warning { margin: 0; font-size: 0.75rem; font-weight: 500; color: var(--color-warning); }
	.export-key {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px;
		background: var(--color-bg-primary);
		border-radius: 4px;
		overflow: hidden;
	}
	.export-key code {
		flex: 1;
		font-size: 0.6875rem;
		font-family: var(--font-mono);
		color: var(--color-text-primary);
		word-break: break-all;
	}
</style>
