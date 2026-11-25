<script lang="ts">
	/**
	 * Asks for a server management session's VNC server (an iDRAC's): port,
	 * VNC password and SSL encryption, and whether to save them with the
	 * session. Shown by the session list when no password is saved, or when
	 * the saved one was refused.
	 */
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import Button from '#lib/components/shared/Button.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import type { SessionConfig } from '#lib/ipc/sessions.js';
	import { bmcSaveVnc } from '#lib/ipc/bmc.js';
	import { BMC_VNC_PORT } from '#lib/sessions/bmc-console.js';

	interface Props {
		/** The session to ask for; `refused`: its saved password was refused. */
		ask: { session: SessionConfig; refused: boolean } | null;
		onclose: () => void;
		/** Open the desktop with these, once saved (or not). */
		onopen: (port: number, password: string, tls: boolean) => void;
	}
	let { ask, onclose, onopen }: Props = $props();

	let port = $state('');
	let password = $state('');
	let tls = $state(false);
	let save = $state(true);
	let busy = $state(false);

	$effect(() => {
		const saved = ask?.session.bmc?.vnc;
		if (!ask) return;
		port = String(saved?.port ?? BMC_VNC_PORT);
		// A refused password is not offered again.
		password = ask.refused ? '' : (saved?.password ?? '');
		tls = saved?.tls ?? false;
		save = true;
		busy = false;
	});

	async function submit(): Promise<void> {
		const a = ask;
		if (!a || busy) return;
		const n = Number.parseInt(port, 10);
		if (!(n > 0 && n < 65536)) {
			addToast(t('bmc.console_bad_port'), 'error');
			return;
		}
		busy = true;
		try {
			// Port and encryption are always kept; the password only if asked.
			await bmcSaveVnc(a.session.id, { port: n, tls, password: save && password ? password : null });
		} catch (err) {
			addToast(String(err), 'error');
		}
		onopen(n, password, tls);
	}
</script>

<Modal open={ask !== null} {onclose} title={t('bmc.vnc_title')} maxWidth="440px">
	{#if ask}
		<form class="vnc-form" onsubmit={(e) => { e.preventDefault(); void submit(); }}>
			{#if ask.refused}
				<p class="note error">{t('bmc.vnc_refused')}</p>
			{:else}
				<p class="note">{t('bmc.vnc_intro')}</p>
			{/if}
			<label>
				<span>{t('bmc.vnc_port')}</span>
				<input type="text" inputmode="numeric" bind:value={port} />
			</label>
			<label>
				<span>{t('bmc.vnc_password')}</span>
				<!-- svelte-ignore a11y_autofocus -->
				<input type="password" bind:value={password} autocomplete="off" maxlength="8" autofocus />
			</label>
			<label class="check">
				<input type="checkbox" bind:checked={tls} />
				<span>{t('bmc.vnc_tls')}</span>
			</label>
			<label class="check">
				<input type="checkbox" bind:checked={save} />
				<span>{t('bmc.vnc_save')}</span>
			</label>
			<p class="note dim">{t('bmc.vnc_hint')}</p>
			<div class="actions">
				<Button variant="ghost" size="sm" type="button" onclick={onclose}>{t('common.cancel')}</Button>
				<Button variant="primary" size="sm" type="submit" disabled={busy}>{t('bmc.console_open')}</Button>
			</div>
		</form>
	{/if}
</Modal>

<style>
	.vnc-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.vnc-form label {
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: 12px;
		color: var(--color-text-secondary);
	}
	.vnc-form label.check {
		flex-direction: row;
		align-items: center;
		gap: 8px;
		font-size: 13px;
		color: var(--color-text-primary);
	}
	.vnc-form input:not([type='checkbox']) {
		padding: 6px 8px;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 13px;
	}
	.note {
		margin: 0;
		font-size: 13px;
		color: var(--color-text-secondary);
	}
	.note.dim {
		font-size: 12px;
		color: var(--color-text-tertiary);
	}
	.note.error {
		color: var(--color-danger);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
	}
</style>
