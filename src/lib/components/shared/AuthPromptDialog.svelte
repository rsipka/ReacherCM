<script lang="ts">
	import PasswordField from '#lib/components/shared/PasswordField.svelte';
	/**
	 * A question a login asks while it runs: a password, a key's passphrase,
	 * the server's keyboard-interactive questions (one-time codes and the
	 * like), or a yes/no. One at a time, oldest first; withdrawn when the
	 * login no longer needs it.
	 */
	import { onMount, onDestroy } from 'svelte';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { invoke } from '@tauri-apps/api/core';
	import Modal from '#lib/components/shared/Modal.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { addToast, dismissToast } from '#lib/state/toasts.svelte.js';
	import { listedMeta } from '#lib/terminal/tmux-tabs.js';

	interface AuthPrompt {
		promptId: string;
		host: string;
		port: number;
		/** `unlock`: a credential key's passphrase asked ahead of any login
		 *  (Settings → Security). `host` holds the credentials' names, `title`
		 *  the key, and `instructions` is "retry" after a wrong passphrase. */
		kind: 'password' | 'passphrase' | 'unlock' | 'keyboard' | 'confirm' | 'tmux';
		title: string;
		instructions: string;
		fields: { text: string; echo: boolean }[];
	}

	let queue = $state<AuthPrompt[]>([]);
	let current = $derived(queue[0]);
	let answers = $state<string[]>([]);

	$effect(() => {
		// tmux: one answer, the name of a new session, filled in.
		answers = current ? (current.kind === 'tmux' ? [current.title] : current.fields.map(() => '')) : [];
	});

	/** tmux: the server's sessions, as `name\twindows\tattached\tactivity\texited`
	 *  (Zellij: no windows, clients or activity). */
	let tmuxSessions = $derived(
		current?.kind === 'tmux'
			? current.fields.map((f) => {
					const [name, windows, attached, activity, exited] = f.text.split('\t');
					return { name, windows: Number(windows) || 0, attached: Number(attached) || 0, activity: Number(activity) || 0, exited: exited === '1' };
				})
			: []
	);
	/** The picker's answer for "no tmux" (ssh::tmux::NO_TMUX_ANSWER). */
	const NO_TMUX = '\u0001none';

	async function pick(answer: string): Promise<void> {
		answers = [answer];
		await respond(true);
	}

	function ago(unixSeconds: number): string {
		const mins = Math.max(0, Math.round((Date.now() / 1000 - unixSeconds) / 60));
		if (mins < 1) return t('tmux.used_now');
		if (mins < 60) return t('tmux.used_minutes', { count: String(mins) });
		const hours = Math.round(mins / 60);
		if (hours < 48) return t('tmux.used_hours', { count: String(hours) });
		return t('tmux.used_days', { count: String(Math.round(hours / 24)) });
	}

	/** A notice with no answer while a login waits ("Confirm user presence for key …"). */
	interface AuthNotice {
		host: string;
		port: number;
		text: string | null;
		done: string | null;
	}
	let noticeId: string | undefined;

	let unlisten: UnlistenFn | undefined;
	let unlistenClosed: UnlistenFn | undefined;
	let unlistenNotice: UnlistenFn | undefined;
	onMount(async () => {
		unlistenNotice = await listen<AuthNotice>('ssh-auth-notice', (e) => {
			if (noticeId) dismissToast(noticeId);
			noticeId = undefined;
			if (e.payload.text) {
				noticeId = addToast(`${e.payload.host}: ${e.payload.text}`, 'info', 0).id;
			} else if (e.payload.done) {
				addToast(e.payload.done, 'success', 2000);
			}
		});
		unlisten = await listen<AuthPrompt>('ssh-auth-prompt', (e) => {
			queue = [...queue, e.payload];
		});
		unlistenClosed = await listen<string>('ssh-auth-prompt-closed', (e) => {
			queue = queue.filter((p) => p.promptId !== e.payload);
		});
	});
	onDestroy(() => {
		unlisten?.();
		unlistenClosed?.();
		unlistenNotice?.();
	});

	async function respond(ok: boolean): Promise<void> {
		const p = current;
		if (!p) return;
		const given = ok ? [...answers] : null;
		queue = queue.slice(1);
		answers = [];
		try {
			await invoke('ssh_auth_prompt_response', { promptId: p.promptId, answers: given });
		} catch (err) {
			console.error('Login prompt response failed:', err);
		}
	}

	let heading = $derived(current ? t(`authprompt.title_${current.kind}`, { host: current.host }) : '');
	// The backend's own sentence ("Enter passphrase for key …") under the heading.
	let detail = $derived(
		!current || current.kind === 'keyboard'
			? ''
			: current.kind === 'tmux'
				? (current.fields.length ? t('tmux.pick_detail') : t('tmux.pick_none'))
			: current.kind === 'unlock'
				? t('authprompt.unlock_detail', { key: current.title, credentials: current.host })
				: current.title
	);
	let instructions = $derived(
		current?.kind === 'unlock' ? (current.instructions ? t('authprompt.unlock_retry') : '') : (current?.instructions ?? '')
	);
</script>

{#if current}
	<Modal open={true} onclose={() => respond(false)} zIndex={1000} title={heading} maxWidth="480px">
		<form class="form" onsubmit={(e) => { e.preventDefault(); respond(true); }}>
			{#if current.kind === 'keyboard' && current.title}
				<p class="msg strong">{current.title}</p>
			{/if}
			{#if detail}
				<p class="msg">{detail}</p>
			{/if}
			{#if instructions}
				<p class="msg pre">{instructions}</p>
			{/if}
			{#if current.kind === 'tmux'}
				{#if tmuxSessions.length}
					<div class="tmux-list">
						{#each tmuxSessions as s (s.name)}
							<button type="button" class="tmux-row" onclick={() => pick(s.name)}>
								<span class="tmux-name">{s.name}</span>
								<span class="tmux-meta">
									{[listedMeta(s), s.activity > 0 ? ago(s.activity) : ''].filter(Boolean).join(' · ')}
								</span>
							</button>
						{/each}
					</div>
				{/if}
				<label class="field">
					<span class="label">{t('tmux.new_session')}</span>
					<!-- svelte-ignore a11y_autofocus -->
					<input class="input" type="text" bind:value={answers[0]} autocomplete="off" autocapitalize="off" spellcheck="false" autofocus={!tmuxSessions.length} />
				</label>
			{:else if current.kind !== 'confirm'}
				{#each current.fields as f, i (i)}
					<label class="field">
						<span class="label">{current.kind === 'keyboard' ? f.text : t(`authprompt.field_${current.kind}`)}</span>
						<PasswordField secret={!f.echo}>
							{#snippet children(type)}
								<!-- svelte-ignore a11y_autofocus -->
								<input
									class="input"
									{type}
									bind:value={answers[i]}
									autocomplete="off"
									autocapitalize="off"
									spellcheck="false"
									autofocus={i === 0}
								/>
							{/snippet}
						</PasswordField>
					</label>
				{/each}
			{/if}
			<p class="hint">
				{current.kind === 'unlock' ? t('authprompt.unlock_hint') : t('authprompt.host_hint', { host: `${current.host}:${current.port}` })}
			</p>
		</form>

		{#snippet actions()}
			{#if current.kind === 'tmux'}
				<Button variant="secondary" onclick={() => pick(NO_TMUX)}>{t('tmux.no_tmux')}</Button>
			{/if}
			<Button variant="secondary" onclick={() => respond(false)}>
				{current.kind === 'confirm' ? t('authprompt.no') : t('common.cancel')}
			</Button>
			<Button variant="primary" onclick={() => respond(true)}>
				{current.kind === 'confirm' ? t('authprompt.yes') : current.kind === 'tmux' ? t('tmux.start_new') : t('authprompt.continue')}
			</Button>
		{/snippet}
	</Modal>
{/if}

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.msg {
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
		overflow-wrap: anywhere;
	}

	.msg.strong {
		color: var(--color-text-primary);
		font-weight: 600;
	}

	.msg.pre {
		white-space: pre-wrap;
	}

	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.label {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.input {
		min-height: 34px;
		padding: 6px 10px;
		font: inherit;
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
	}

	.input:focus {
		outline: none;
		border-color: var(--color-accent);
	}

	.tmux-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
		max-height: 260px;
		overflow-y: auto;
	}

	.tmux-row {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 2px;
		padding: 8px 10px;
		text-align: left;
		font: inherit;
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
		cursor: pointer;
	}

	.tmux-row:hover,
	.tmux-row:focus-visible {
		border-color: var(--color-accent);
		outline: none;
	}

	.tmux-name {
		font-family: var(--font-mono, monospace);
		font-size: 0.8125rem;
	}

	.tmux-meta {
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.hint {
		margin: 0;
		font-size: 0.6875rem;
		color: var(--color-text-tertiary, var(--color-text-secondary));
	}

	@media (max-width: 700px) {
		.input {
			min-height: 44px;
		}
	}
</style>
