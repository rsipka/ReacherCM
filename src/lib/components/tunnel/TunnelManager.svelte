<script lang="ts">
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { tunnelCreate, tunnelStart, tunnelStop, tunnelRemove, tunnelList, type TunnelConfig } from '#lib/ipc/tunnel.js';
	import { sessionList, sessionKind, sessionApproveSecretRefs, untrustedRefsFromError, type UntrustedRefs } from '#lib/ipc/sessions.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import UntrustedRefsDialog from '#lib/components/pwmanager/UntrustedRefsDialog.svelte';
	import TunnelCard from './TunnelCard.svelte';
	import WireGuardPanel from './WireGuardPanel.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import Button from '#lib/components/shared/Button.svelte';

	const TUNNEL_TYPES = [
		{ type: 'Local', label: 'tunnel.local_forward' },
		{ type: 'Dynamic', label: 'tunnel.dynamic' }
	] as const;

	let tunnels = $state<TunnelConfig[]>([]);
	let sessions = $state<{ id: string; label: string }[]>([]);
	let sessionsError = $state('');
	let loading = $state(true);
	let showForm = $state(false);
	let formSessionId = $state('');
	let formType = $state<TunnelConfig['tunnel_type']>('Local');
	let formLocalPort = $state('8080');
	let formRemoteHost = $state('localhost');
	let formRemotePort = $state('80');
	let creating = $state(false);
	const busy = new SvelteSet<string>();

	let approval = $state<{ tunnel: TunnelConfig; asked: UntrustedRefs } | null>(null);
	let approving = $state(false);
	let approvalError = $state<string | undefined>();
	let listRequest = 0;

	async function loadTunnels(): Promise<void> {
		const request = ++listRequest;
		try {
			const list = await tunnelList();
			if (request === listRequest) tunnels = list;
		} catch (err) {
			console.error('Failed to load tunnels:', err);
		} finally {
			if (request === listRequest) loading = false;
		}
	}

	async function loadSessions(): Promise<void> {
		sessionsError = '';
		try {
			sessions = (await sessionList())
				.filter((session) => sessionKind(session) === 'ssh')
				.map((session) => ({
					id: session.id,
					label: `${session.name} (${session.username}@${session.host}:${session.port})`
				}))
				.sort((a, b) => a.label.localeCompare(b.label));
		} catch (err) {
			sessionsError = String(err);
		}
	}

	onMount(() => {
		void loadTunnels();
		void loadSessions();
		const timer = window.setInterval(() => {
			if (busy.size || tunnels.some((tunnel) => tunnel.status !== 'stopped')) void loadTunnels();
		}, 2000);
		const reload = () => void loadSessions();
		window.addEventListener('reach:sessions-changed', reload);
		return () => {
			window.clearInterval(timer);
			window.removeEventListener('reach:sessions-changed', reload);
		};
	});

	function handleNewTunnel(): void {
		formSessionId = sessions.length === 1 ? sessions[0].id : '';
		formType = 'Local';
		formLocalPort = '8080';
		formRemoteHost = 'localhost';
		formRemotePort = '80';
		showForm = true;
	}

	async function handleCreateTunnel(): Promise<void> {
		if (!formSessionId || creating) return;
		const localPort = Number(formLocalPort);
		const remotePort = Number(formRemotePort);
		const dynamic = formType === 'Dynamic';

		if (!Number.isInteger(localPort) || localPort < 1 || localPort > 65535) {
			addToast(t('tunnel.invalid_local_port'), 'error');
			return;
		}
		if (!dynamic && (!Number.isInteger(remotePort) || remotePort < 1 || remotePort > 65535)) {
			addToast(t('tunnel.invalid_remote_port'), 'error');
			return;
		}
		if (!dynamic && !formRemoteHost.trim()) {
			addToast(t('tunnel.remote_host_required'), 'error');
			return;
		}

		try {
			creating = true;
			await tunnelCreate(formType, localPort, formRemoteHost.trim(), dynamic ? 0 : remotePort, formSessionId);
			showForm = false;
			addToast(t('tunnel.created_toast', { port: localPort }), 'success');
		} catch (err) {
			addToast(t('toast.tunnel_create_failed', { error: String(err) }), 'error');
		} finally {
			await loadTunnels();
			creating = false;
		}
	}

	async function handleStart(tunnel: TunnelConfig): Promise<void> {
		if (busy.has(tunnel.id) || approval) return;
		busy.add(tunnel.id);
		try {
			await tunnelStart(tunnel.id);
			addToast(t('tunnel.started_toast', { port: tunnel.local_port }), 'success');
		} catch (err) {
			const asked = untrustedRefsFromError(err);
			if (asked && !approval) {
				approvalError = undefined;
				approval = { tunnel, asked };
			} else {
				addToast(t('toast.tunnel_start_failed', { error: String(err) }), 'error');
			}
		} finally {
			busy.delete(tunnel.id);
			void loadTunnels();
		}
	}

	async function allowRefs(): Promise<void> {
		if (!approval || approving) return;
		const { tunnel, asked } = approval;
		approving = true;
		approvalError = undefined;
		try {
			await sessionApproveSecretRefs(asked);
			approval = null;
		} catch (err) {
			approvalError = String(err);
			return;
		} finally {
			approving = false;
		}
		await handleStart(tunnel);
	}

	async function handleStop(tunnel: TunnelConfig, remove = false): Promise<void> {
		if (busy.has(tunnel.id)) return;
		busy.add(tunnel.id);
		try {
			if (remove) {
				await tunnelRemove(tunnel.id);
				addToast(t('tunnel.removed_toast'), 'info');
			} else {
				await tunnelStop(tunnel.id);
				addToast(t('tunnel.stopped_toast', { port: tunnel.local_port }), 'info');
			}
		} catch (err) {
			addToast(t(remove ? 'toast.tunnel_remove_failed' : 'toast.tunnel_stop_failed', { error: String(err) }), 'error');
		} finally {
			await loadTunnels();
			busy.delete(tunnel.id);
		}
	}
</script>

<div class="tunnel-manager">
	<WireGuardPanel />

	<div class="actions-row">
		<button class="new-tunnel-btn" onclick={handleNewTunnel}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none">
				<path
					d="M12 5v14M5 12h14"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
				/>
			</svg>
			{t('tunnel.new')}
		</button>
	</div>

	{#if showForm}
		<div class="create-form">
			<div class="form-header">
				<span class="form-title">{t('tunnel.create_tunnel')}</span>
				<button class="form-close" onclick={() => (showForm = false)} disabled={creating} aria-label={t('common.close')}>
					<svg width="12" height="12" viewBox="0 0 14 14" fill="none">
						<path d="M1 1L13 13M13 1L1 13" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
					</svg>
				</button>
			</div>

			<div class="form-body">
				<label class="session-field">
					<span>{t('tunnel.ssh_session')}</span>
					<select bind:value={formSessionId} disabled={creating || !sessions.length}>
						<option value="" disabled>{t('tunnel.pick_ssh_session')}</option>
						{#each sessions as session (session.id)}
							<option value={session.id}>{session.label}</option>
						{/each}
					</select>
				</label>
				{#if sessionsError}
					<p class="empty-state">{sessionsError}</p>
				{:else if !sessions.length}
					<p class="empty-state">{t('tunnel.no_ssh_sessions')}</p>
				{/if}
				<div class="type-selector">
					{#each TUNNEL_TYPES as tt (tt.type)}
						<button
							class="type-option"
							class:selected={formType === tt.type}
							onclick={() => (formType = tt.type)}
						>
							{t(tt.label)}
						</button>
					{/each}
				</div>

				<Input label={t('tunnel.local_port')} bind:value={formLocalPort} type="number" placeholder="8080" />
				{#if formType !== 'Dynamic'}
					<Input label={t('tunnel.remote_host')} bind:value={formRemoteHost} placeholder="localhost" />
					<Input label={t('tunnel.remote_port')} bind:value={formRemotePort} type="number" placeholder="80" />
				{/if}

				<div class="form-actions">
					<Button variant="ghost" size="sm" onclick={() => (showForm = false)} disabled={creating}>{t('common.cancel')}</Button>
					<Button variant="primary" size="sm" onclick={handleCreateTunnel} disabled={creating || !formSessionId}>
						{creating ? t('tunnel.creating') : t('tunnel.create')}
					</Button>
				</div>
			</div>
		</div>
	{/if}

	{#if loading}
		<div class="loading-state">
			<span class="spinner"></span>
			<span class="loading-text">{t('tunnel.loading')}</span>
		</div>
	{:else if tunnels.length === 0 && !showForm}
		<p class="empty-state">{t('tunnel.no_tunnels')}</p>
	{:else}
		{#if tunnels.length > 0}
			<div class="divider"></div>
			<div class="tunnels-scroll">
				{#each tunnels as tunnel (tunnel.id)}
					<div class="tunnel-row">
						<TunnelCard
							{tunnel}
							sessionLabel={sessions.find((session) => session.id === tunnel.session_id)?.label ?? t('tunnel.ssh_session_unavailable')}
							busy={busy.has(tunnel.id)}
							onstart={() => handleStart(tunnel)}
							onstop={() => handleStop(tunnel)}
						/>
						<button
							class="delete-btn"
							onclick={() => handleStop(tunnel, true)}
							disabled={busy.has(tunnel.id) || tunnel.status === 'starting'}
							title={t('tunnel.remove')}
							aria-label={t('tunnel.remove')}
						>
							<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M3 6h18" />
								<path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
								<path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
							</svg>
						</button>
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>

<UntrustedRefsDialog
	asked={approval?.asked ?? null}
	busy={approving}
	error={approvalError}
	onallow={() => void allowRefs()}
	oncancel={() => { if (!approving) approval = null; }}
/>

<style>
	.tunnel-manager {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 4px 0;
	}

	.actions-row {
		display: flex;
		gap: 6px;
	}

	.new-tunnel-btn {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1;
		padding: 8px 10px;
		font-family: var(--font-sans);
		font-size: 0.8125rem;
		font-weight: 500;
		border-radius: var(--radius-btn);
		cursor: pointer;
		color: var(--color-accent);
		background: transparent;
		border: 1px solid var(--color-accent);
		transition:
			background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default);
	}

	.new-tunnel-btn:hover {
		background-color: rgba(0, 122, 255, 0.1);
	}

	.new-tunnel-btn:active {
		transform: scale(0.98);
	}

	.create-form {
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card, 8px);
		overflow: hidden;
	}

	.form-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 8px 10px;
		border-bottom: 1px solid var(--color-border);
		background-color: var(--color-surface-hover);
	}

	.form-title {
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.form-close {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 20px;
		height: 20px;
		padding: 0;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.form-close:hover {
		background-color: var(--color-surface-active);
		color: var(--color-text-primary);
	}

	.form-body {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 10px;
	}

	.session-field {
		display: grid;
		gap: 4px;
		min-width: 0;
		color: var(--color-text-secondary);
		font-size: 0.75rem;
	}

	.session-field select {
		min-width: 0;
		width: 100%;
		padding: 6px 8px;
		font: inherit;
		color: var(--color-text-primary);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
	}

	/* The app's segmented control: a recessed track, the choice raised out
	   of it. Labels wrap rather than being cut in a narrow sidebar. */
	.type-selector {
		display: flex;
		gap: 2px;
		padding: 2px;
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: var(--radius-btn);
	}

	.type-option {
		flex: 1;
		min-width: 0;
		padding: 5px 6px;
		line-height: 1.2;
		text-wrap: balance;
		font-family: var(--font-sans);
		font-size: 0.75rem;
		font-weight: 500;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		transition:
			background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default);
	}

	.type-option:hover {
		color: var(--color-text-primary);
	}

	.type-option.selected {
		background-color: var(--color-bg-elevated);
		color: var(--color-accent);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25), inset 0 0 0 1px var(--color-border);
	}

	.form-actions {
		display: flex;
		justify-content: flex-end;
		gap: 6px;
		padding-top: 4px;
	}

	.divider {
		height: 1px;
		background-color: var(--color-border);
		opacity: 0.5;
		margin: 2px 0;
	}

	.tunnels-scroll {
		display: flex;
		flex-direction: column;
		gap: 2px;
		overflow-y: auto;
	}

	.tunnel-row {
		display: flex;
		align-items: center;
		gap: 2px;
	}

	.tunnel-row :global(.tunnel-card) {
		flex: 1;
	}

	.delete-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		padding: 0;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		opacity: 0;
		transition:
			background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default),
			opacity var(--duration-default) var(--ease-default);
	}

	.tunnel-row:hover .delete-btn {
		opacity: 1;
	}

	.delete-btn:hover {
		background-color: var(--color-surface-active);
		color: var(--color-danger);
	}

	.delete-btn:active {
		transform: scale(0.92);
	}

	.loading-state {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		padding: 16px 0;
	}

	.loading-text {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.empty-state {
		margin: 0;
		padding: 12px 4px;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
		opacity: 0.7;
		text-align: center;
	}

	.spinner {
		display: inline-block;
		width: 14px;
		height: 14px;
		border: 2px solid var(--color-border);
		border-top-color: var(--color-accent);
		border-radius: 50%;
		animation: spin 0.6s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
