<script lang="ts">
	/**
	 * Picks the login the current person uses for a session (or a folder of
	 * sessions) in a shared vault that asks each person for their own. The
	 * choice is kept in their own vault and never written to the shared one:
	 * one of their own saved credentials (My sessions), or the session's user
	 * name with their default SSH key (Settings > Security).
	 *
	 * Shown on the first connect, and from the session's menu (My login…) to
	 * change it.
	 */
	import Modal from '#lib/components/shared/Modal.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { DEFAULT_KEY, credentialsFor, type SavedCredential } from '#lib/ipc/savedCredentials.js';

	interface Props {
		/** What is asked for; null: closed. `passwordOnly` for kinds that
		 *  log in with a password (RDP, Telnet…): no keys offered. */
		asked: { sessionName: string; username: string; current: string | null; passwordOnly: boolean } | null;
		/** Every saved credential; only the person's own are offered. */
		credentials: SavedCredential[];
		busy?: boolean;
		error?: string;
		onpick: (credentialId: string) => void;
		/** Forget the choice, so the next connect asks again. */
		onforget?: () => void;
		oncancel: () => void;
	}

	let { asked, credentials, busy = false, error, onpick, onforget, oncancel }: Props = $props();

	let mine = $derived(
		credentialsFor(credentials, null).filter((c) => !asked?.passwordOnly || c.auth_method.type === 'Password')
	);
	let picked = $state('');

	$effect(() => {
		picked = asked?.current ?? '';
	});

	function method(c: SavedCredential): string {
		const a = c.auth_method;
		if (a.type === 'Key') return t('session.auth_key');
		if (a.type === 'Agent') return t('session.auth_agent');
		return a.password_ref ? t('session.auth_manager') : t('session.auth_password');
	}
</script>

<Modal open={!!asked} onclose={oncancel} title={t('credential.personal_pick_title')} maxWidth="480px" zIndex={300}>
	{#if asked}
		<div class="body">
			<p class="note">{t('credential.personal_pick_desc', { name: asked.sessionName })}</p>
			<div class="choices" role="radiogroup">
				{#if !asked.passwordOnly}
					<label class="choice">
						<input type="radio" name="personal-login" value={DEFAULT_KEY} bind:group={picked} disabled={busy} />
						<span class="main">{t('credential.personal_default_key')}</span>
						<span class="sub">{asked.username ? t('credential.personal_default_key_as', { username: asked.username }) : t('credential.personal_default_key_desc')}</span>
					</label>
				{/if}
				{#each mine as c (c.id)}
					<label class="choice">
						<input type="radio" name="personal-login" value={c.id} bind:group={picked} disabled={busy} />
						<span class="main">{c.name}</span>
						<span class="sub">{c.username} · {method(c)}</span>
					</label>
				{/each}
			</div>
			{#if mine.length === 0}
				<p class="note">{t('credential.personal_pick_none')}</p>
			{/if}
			{#if error}
				<p class="error" role="alert">{error}</p>
			{/if}
		</div>
	{/if}
	{#snippet actions()}
		{#if asked?.current && onforget}
			<Button variant="ghost" onclick={onforget} disabled={busy}>{t('credential.personal_forget')}</Button>
		{/if}
		<Button variant="ghost" onclick={oncancel} disabled={busy}>{t('common.cancel')}</Button>
		<Button variant="primary" onclick={() => onpick(picked)} disabled={busy || !picked}>{t('credential.personal_use')}</Button>
	{/snippet}
</Modal>

<style>
	.body {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.note {
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
		overflow-wrap: anywhere;
	}

	.choices {
		display: flex;
		flex-direction: column;
		gap: 6px;
		max-height: 320px;
		overflow-y: auto;
	}

	.choice {
		display: grid;
		grid-template-columns: auto 1fr;
		column-gap: 10px;
		align-items: center;
		padding: 8px 10px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	.choice:has(input:checked) {
		border-color: var(--color-accent);
		background: color-mix(in srgb, var(--color-accent) 8%, transparent);
	}

	.choice input {
		grid-row: span 2;
	}

	.main {
		font-size: 0.875rem;
		color: var(--color-text-primary);
		overflow-wrap: anywhere;
	}

	.sub {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		overflow-wrap: anywhere;
	}

	.error {
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-danger);
		overflow-wrap: anywhere;
	}
</style>
