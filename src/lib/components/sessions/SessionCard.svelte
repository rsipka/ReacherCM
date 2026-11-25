<script lang="ts">
	import type { SessionConfig } from '#lib/ipc/sessions.js';
	import type { VaultInfo } from '#lib/state/vault.svelte.js';
	import DistroIcon from './DistroIcon.svelte';
	import { getDistroIcon, defaultSessionOs } from '#lib/data/distro-icons.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { sessionAddress } from '#lib/utils/address.js';
	import { addressLabelled, hasAltAddresses, sessionAddresses } from '#lib/sessions/addresses.js';
	import { isMobile, isWindows } from '#lib/platform.js';
	import { getSettings } from '#lib/state/settings.svelte.js';
	import { tooltip } from '#lib/utils/tooltip.js';
	import { longPress } from '#lib/utils/longPress.js';
	import { formatHops, formatProxyAddress, ROUTE_ICON } from '#lib/utils/route.js';
	import { wireguardName, wireguardProfiles } from '#lib/state/wireguard.svelte.js';

	const settings = getSettings();

	interface Props {
		session: SessionConfig;
		/** Named on the card when the list spans vaults, so a session says
		 *  where it lives instead of the person having to remember. */
		vault?: VaultInfo | null;
		onconnect: () => void;
		onedit: () => void;
		ondelete: () => void;
		oncontextmenu?: (e: MouseEvent) => void;
		ondragstart?: (e: PointerEvent) => void;
		ondragend?: () => void;
		/** Selecting several: a click selects instead of connecting. */
		selecting?: boolean;
		selected?: boolean;
		/** Ctrl/Cmd or Shift click, or any click while selecting. */
		onselect?: (e: MouseEvent) => void;
		/** VNC: the name of the saved SSH session it is reached through. */
		viaName?: string | null;
		/** It has a tab open in this window: a slight tint. */
		open?: boolean;
		/** Its tab is the one on screen: shown as picked. */
		current?: boolean;
		/** One of its tabs is connected: a green dot by the name. */
		connected?: boolean;
	}

	let { session, vault = null, onconnect, onedit, ondelete, oncontextmenu, ondragstart, ondragend, selecting = false, selected = false, onselect, viaName = null, open = false, current = false, connected = false }: Props = $props();
	let unavailable = $derived((session.kind === 'wsl' && !isWindows()) || ((session.kind === 'serial' || session.kind === 'winbox' || session.kind === 'ftp' || !!session.files || !!session.x11_desktop) && isMobile()));
	/** A session that opens in the file manager: which protocol, for its badge. */
	let filesBadge = $derived(session.kind === 'ftp' ? (session.ftp?.tls === 'none' ? 'FTP' : 'FTPS') : session.files === 'scp' ? 'SCP' : session.files === 'sftp' ? 'SFTP' : '');
	/** WinBox only ever manages a MikroTik, a Server management session a
	 *  BMC: their logo until another is found or picked. */
	let osId = $derived(session.detected_os ?? defaultSessionOs(session.kind, session.web?.login));
	/** Double-click to open (Settings > General): a mouse click only focuses
	 *  the row. Enter or Space (a click with no detail) and a tap still
	 *  connect, as there is no double press for either. */
	let needsDouble = $derived(settings.doubleClickToOpen && !selecting);

	function onMain(e: MouseEvent): void {
		if (unavailable && !selecting) return;
		if (onselect && (selecting || e.ctrlKey || e.metaKey || e.shiftKey)) {
			e.preventDefault();
			onselect(e);
		} else if (needsDouble && e.detail > 0 && (e as PointerEvent).pointerType !== 'touch') {
			// The dblclick event connects. Focus the row so it shows as picked:
			// WebKit does not focus a button on click.
			(e.currentTarget as HTMLElement).focus();
		} else {
			onconnect();
		}
	}

	let mainTitle = $derived.by(() => {
		if (unavailable) return t('session.wsl_windows_only');
		if (selecting) return t('session.select_name', { name: session.name });
		return t(needsDouble ? 'session.double_click_connect_to' : 'session.connect_to', { name: session.name });
	});

	function onMainDouble(e: MouseEvent): void {
		if (!needsDouble || unavailable) return;
		if (e.ctrlKey || e.metaKey || e.shiftKey) return;
		onconnect();
	}

	/** A lock for a password, a key for a key, a link for the agent. */
	let authIcon = $derived(
		session.auth_method.type === 'Password'
			? 'M6 11h12v10H6zM8 11V8a4 4 0 0 1 8 0v3'
			: session.auth_method.type === 'Key'
				? 'M15 7a4 4 0 1 1-3.9 4.9L4 19v2h3v-2h2v-2h2l1.1-1.1A4 4 0 0 1 15 7zM16 9.5h.01'
				: 'M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7'
	);

	/** How the session gets to its server when not directly: through jump
	 *  hosts or a saved SSH session (VNC), or through a proxy. Remote
	 *  desktops keep no jump chain or proxy, so stale ones are not shown. */
	let desktopKind = $derived(session.kind === 'rdp' || session.kind === 'vnc' || session.kind === 'spice');
	let jumpTip = $derived.by(() => {
		if (session.kind === 'vnc' || session.kind === 'spice') return session.via_session_id ? t('session.via_session', { name: viaName || '?' }) : '';
		if (desktopKind || !session.jump_chain?.length) return '';
		return t('session.via_jump', { hops: formatHops(session.jump_chain) });
	});
	let proxyTip = $derived(
		!desktopKind && session.proxy?.host
			? t('session.via_proxy', { type: session.proxy.proxy_type.toUpperCase(), address: formatProxyAddress(session.proxy) })
			: ''
	);

	/** A WireGuard tunnel the session (or one of its other addresses) goes
	 *  through, by profile name. */
	let wireguardTip = $derived.by(() => {
		const ids = [session.wireguard_id, ...(session.alt_addresses ?? []).map((a) => a.via_wireguard)].filter((id): id is string => !!id);
		if (!ids.length) return '';
		wireguardProfiles();
		return t('session.via_wireguard', { name: [...new Set(ids)].map(wireguardName).join(', ') });
	});

	/** The session's notes and fields, for a tooltip: hidden values stay masked. */
	let docsTooltip = $derived.by(() => {
		const notes = session.notes?.trim() ?? '';
		const fields = (session.custom_fields ?? []).map((f) => `${f.name || '—'}: ${f.hidden ? '••••••' : f.value}`);
		const text = [notes.length > 600 ? `${notes.slice(0, 600)}…` : notes, fields.join('\n')].filter(Boolean).join('\n\n');
		return text;
	});

	/** Compact layout shows the name alone; on desktop everything else the
	 *  full row shows is in the row's tooltip instead. */
	let compact = $derived(settings.compactLayout);
	let showDetails = $derived(compact && !isMobile());
	/** The session's other addresses, for the tooltips. */
	let otherAddresses = $derived(
		hasAltAddresses(session)
			? t('session.more_addresses', { addresses: sessionAddresses(session).slice(1).map((a) => addressLabelled(a, session.port)).join(', ') })
			: ''
	);
	let detailsTooltip = $derived.by(() => {
		const kind = session.kind ?? 'ssh';
		const lines: string[] = [session.favorite ? `${session.name} ★` : session.name];
		lines.push(kind === 'wsl'
			? `${session.username ? `${session.username} · ` : ''}${session.wsl_distro || 'WSL'}`
			: sessionAddress(session));
		if (otherAddresses) lines.push(otherAddresses);
		const type = t(`session.protocol_${session.files ?? kind}`);
		lines.push(kind === 'wsl' || kind === 'serial' ? type : `${type} · ${t('session.auth_type', { type: session.auth_method.type })}`);
		if (kind !== 'wsl' && session.tags.length > 0) lines.push(t('session.tooltip_tags', { tags: session.tags.join(', ') }));
		if (jumpTip) lines.push(jumpTip);
		if (proxyTip) lines.push(proxyTip);
		if (vault) lines.push(t('session.tooltip_vault', { name: vault.name }));
		if (session.credential) {
			lines.push(session.credential.missing ? t('credential.missing') : t('credential.chip', { name: session.credential.name }));
		}
		const parts = [lines.join('\n')];
		if (docsTooltip) parts.push(docsTooltip);
		parts.push(mainTitle);
		return parts.join('\n\n');
	});

	let authLabel = $derived(
		session.kind === 'wsl' ? 'WSL' :
		session.auth_method.type === 'Password' ? t('session.auth_pw_label') :
		session.auth_method.type === 'Key' ? t('session.auth_key') : t('session.auth_agent')
	);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="session-card" class:selected class:open class:current class:unavailable class:dbl={needsDouble} class:names-only={compact} oncontextmenu={oncontextmenu} use:longPress={!!oncontextmenu} role="group">
	{#if selecting}
		<input
			class="select-box"
			type="checkbox"
			checked={selected}
			aria-label={t('session.select_name', { name: session.name })}
			onclick={(e) => { e.stopPropagation(); onselect?.(e); }}
		/>
	{/if}
	<span
		class="drag-handle"
		role="img"
		aria-label={t('common.drag_to_reorder')}
		onpointerdown={(e) => { if (ondragstart) { e.stopPropagation(); ondragstart(e); } }}
	>
		<svg width="8" height="12" viewBox="0 0 8 12" fill="currentColor">
			<circle cx="2" cy="2" r="1.2"/><circle cx="6" cy="2" r="1.2"/>
			<circle cx="2" cy="6" r="1.2"/><circle cx="6" cy="6" r="1.2"/>
			<circle cx="2" cy="10" r="1.2"/><circle cx="6" cy="10" r="1.2"/>
		</svg>
	</span>
	<button class="card-main" onclick={onMain} ondblclick={onMainDouble} disabled={unavailable && !selecting} aria-pressed={selecting ? selected : undefined} title={showDetails ? undefined : mainTitle} use:tooltip={showDetails ? detailsTooltip : ''} >
		<!-- Every row has the icon's place, so names line up whether or not the
		     OS is known yet: its logo once detected, until then a quiet mark for
		     the kind of session. -->
		<span class="os-slot" aria-hidden="true">
			{#if getDistroIcon(osId)}
				<DistroIcon {osId} size={18} />
			{:else}
				<svg class="kind-mark" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
					{#if session.kind === 'rdp' || session.kind === 'vnc' || session.kind === 'spice'}
						<rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" />
					{:else if session.kind === 'wsl'}
						<path d="M5 8l4 4-4 4M11 16h7" />
					{:else if session.kind === 'telnet'}
						<!-- A console: a network device's prompt rather than a server. -->
						<rect x="3" y="5" width="18" height="14" rx="2" /><path d="M7 10l3 2-3 2M12 14h5" />
					{:else if session.kind === 'web'}
						<!-- A globe: a device's web page. -->
						<circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9c-2.5-2.6-3.8-5.6-3.8-9S9.5 5.6 12 3z" />
					{:else if session.kind === 'bmc'}
						<!-- A rack server with a pulse: its management controller. -->
						<path d="M4 4h16v6H4zM4 14h16v6H4zM7 7h.01M7 17h.01M11 17h2l1-2 2 4 1-2h1" />
					{:else if session.kind === 'ftp'}
						<!-- A folder: files on a server. -->
						<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
					{:else if session.kind === 'serial'}
						<!-- A serial (DB9) connector. -->
						<path d="M3 7h18l-2.5 10h-13z" /><path d="M8 10h.01M12 10h.01M16 10h.01M10 14h.01M14 14h.01" />
					{:else}
						<rect x="3" y="4" width="18" height="7" rx="1.5" /><rect x="3" y="13" width="18" height="7" rx="1.5" /><path d="M7 7.5h.01M7 16.5h.01" />
					{/if}
				</svg>
			{/if}
		</span>
		<div class="session-info">
			<span class="session-name-row">
				<span class="session-name">{session.name}</span>
				{#if connected}
					<!-- Connected: a green dot right by the name, so it stays in
					     Compact layout and on phones. -->
					<span class="mark-tip" title={t('session.connected_mark')}><span class="live-mark" role="img" aria-label={t('session.connected_mark')}></span></span>
				{:else if open}
					<span class="visually-hidden">{t('session.open_mark')}</span>
				{/if}
				{#if session.favorite}
					<span class="mark-tip" title={t('session.favorite')}><svg class="fav-mark" width="11" height="11" viewBox="0 0 24 24" fill="currentColor" role="img" aria-label={t('session.favorite')}>
						<path d="M12 3.5l2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z" />
					</svg></span>
				{/if}
				<!-- Not a direct connection: a route for jump hosts, a globe for a
				     proxy. Small marks by the name, so they stay in Compact layout
				     and on phones, where the other chips are hidden. -->
				{#if jumpTip}
					<span class="mark-tip route-mark" title={jumpTip}><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" role="img" aria-label={jumpTip}>
						<path d={ROUTE_ICON.jump} />
					</svg></span>
				{/if}
				{#if proxyTip}
					<span class="mark-tip route-mark" title={proxyTip}><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" role="img" aria-label={proxyTip}>
						<path d={ROUTE_ICON.proxy} />
					</svg></span>
				{/if}
				{#if wireguardTip}
					<span class="mark-tip route-mark" title={wireguardTip}><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" role="img" aria-label={wireguardTip}>
						<path d={ROUTE_ICON.wireguard} />
					</svg></span>
				{/if}
				<!-- Beside the name, which is short, so the address below has the
				     row's width to itself. -->
				{#if session.kind !== 'wsl' && session.tags.length > 0}
					<!-- One line that clips rather than wraps: a second line of tags
					     made one row taller than its neighbours. All are in the tooltip. -->
					<span class="tags" title={session.tags.join(', ')}>
						{#each session.tags.slice(0, 2) as tag (tag)}
							<span class="tag-chip">{tag}</span>
						{/each}
						{#if session.tags.length > 2}
							<span class="tag-chip more">+{session.tags.length - 2}</span>
						{/if}
					</span>
				{/if}
				{#if vault}
					<span class="vault-chip" class:shared={vault.vaultType === 'shared'} title={vault.name}>
						{#if vault.vaultType === 'shared'}
							<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>
							</svg>
						{:else}
							<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
								<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>
							</svg>
						{/if}
						<span class="vault-chip-name">{vault.name}</span>
					</span>
				{/if}
				{#if session.credential}
					<!-- The login comes from a saved credential: which one, or that it is missing. -->
					<span
						class="vault-chip cred-chip"
						class:missing={session.credential.missing}
						title={session.credential.missing ? t('credential.missing_hint') : session.credential.folder_id ? t('credential.chip_folder', { name: session.credential.name }) : t('credential.chip', { name: session.credential.name })}
					>
						<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
							<circle cx="7.5" cy="15.5" r="4.5"/><path d="M10.7 12.3 21 2M16 7l3 3M18 5l3 3"/>
						</svg>
						<span class="vault-chip-name">{session.credential.missing ? t('credential.missing') : session.credential.name}</span>
					</span>
				{/if}
			</span>
			{#if session.kind === 'wsl'}
				<span class="session-detail">
					{session.username ? `${session.username} · ` : ''}{session.wsl_distro || 'WSL'}
					{#if unavailable}
						· <span class="unavailable-text">({t('session.wsl_windows_only')})</span>
					{/if}
				</span>
			{:else}
				<span class="session-meta">
					<!-- The port only when it is not ssh's own: ":22" on every row
					     said nothing. -->
					<span class="session-detail">{sessionAddress(session)}</span>
					{#if otherAddresses}
						<span class="more-addresses" title={otherAddresses}>+{(session.alt_addresses ?? []).length}</span>
					{/if}
				</span>
			{/if}
		</div>
		{#if docsTooltip}
			<!-- Notes or fields: read them by hovering, edit them in the editor. -->
			<span class="auth-mark docs-mark" title={docsTooltip} aria-label={t('session.docs')}>
				<svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
					<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6M8 13h8M8 17h5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
				</svg>
			</span>
		{/if}
		<!-- How it logs in, as a quiet mark rather than a label on every row. -->
		{#if session.kind === 'wsl'}
			<span class="auth-badge">WSL</span>
		{:else if session.kind === 'serial'}
			<span class="auth-badge">{unavailable ? t('serial.desktop_only_short') : t('session.protocol_serial')}</span>
		{:else if session.x11_desktop}
			<!-- A whole desktop, in a window of its own. -->
			<span class="auth-badge" title={t(`x11desktop.${session.x11_desktop.desktop}`)}>{unavailable ? t('serial.desktop_only_short') : 'X11'}</span>
		{:else if filesBadge}
			<!-- Opens in the file manager, not a terminal. -->
			<span class="auth-badge" title={authLabel}>{unavailable ? t('serial.desktop_only_short') : filesBadge}</span>
		{:else if session.kind === 'bmc'}
			<!-- Talks Redfish to the server's management controller. -->
			<span class="auth-badge" title={t('bmc.badge_hint')}>{t('bmc.badge')}</span>
		{:else if session.kind === 'rdp' && session.rdp?.remote_app && session.rdp?.program}
			<!-- Opens one program's windows, not a desktop. -->
			<span class="auth-badge" title={`RemoteApp: ${session.rdp.program}`}>App</span>
		{:else if session.kind === 'winbox'}
			<!-- Opens in WinBox, not in a tab here. -->
			<span class="auth-badge">{unavailable ? t('serial.desktop_only_short') : t('session.protocol_winbox')}</span>
		{:else}
			<span class="auth-mark" title={`${t('session.auth_type', { type: session.auth_method.type })} · ${authLabel}`} aria-label={authLabel}>
				<svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true">
					<path d={authIcon} stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
				</svg>
			</span>
		{/if}
	</button>

	<div class="session-actions">
		<button class="action-btn connect-btn" disabled={unavailable} onclick={onconnect} title={t('session.connect')} aria-label={t('session.connect_to', { name: session.name })}>
			<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
				<path d="M8 5v14l11-7z"/>
			</svg>
		</button>
		<button class="action-btn edit-btn" onclick={onedit} title={t('session.edit')} aria-label={t('session.edit_name', { name: session.name })}>
			<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M17 3a2.85 2.85 0 0 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/>
			</svg>
		</button>
		<button class="action-btn delete-btn" onclick={ondelete} title={t('session.delete')} aria-label={t('session.delete_name', { name: session.name })}>
			<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M3 6h18"/>
				<path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/>
				<path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>
			</svg>
		</button>
	</div>
</div>

<style>
	.drag-handle {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 16px;
		height: 24px;
		flex-shrink: 0;
		color: var(--color-text-secondary);
		opacity: 0.15;
		cursor: grab;
		touch-action: none;
		transition: opacity 0.15s ease;
	}

	.drag-handle:active {
		cursor: grabbing;
		opacity: 0.6;
	}

	.session-card:hover .drag-handle {
		opacity: 0.4;
	}

	/* With a mouse, the grip shows on the row pointed at only. */
	@media (pointer: fine) {
		.drag-handle {
			opacity: 0;
		}

		.session-card:hover .drag-handle {
			opacity: 0.4;
		}
	}

	.session-card {
		position: relative;
		display: flex;
		align-items: center;
		gap: 3px;
		padding: 4px 6px;
		border-radius: 6px;
		transition: background-color var(--duration-default) var(--ease-default);
		/* A long press opens the session menu: no iOS link callout over it. */
		-webkit-touch-callout: none;
	}

	.session-card:hover {
		background-color: var(--color-surface-hover);
	}

	/* Selected: tinted, with a bar on the left, so it differs from a hover
	   by more than a shade. */
	.session-card.selected,
	.session-card.selected:hover {
		background-color: var(--color-accent-subtle);
		box-shadow: inset 2px 0 0 var(--color-accent);
	}

	/* Open in a tab: a slight accent tint, below a hover in strength. */
	.session-card.open {
		background-color: color-mix(in srgb, var(--color-accent) 7%, transparent);
	}

	.session-card.open:hover {
		background-color: color-mix(in srgb, var(--color-accent) 12%, var(--color-surface-hover));
	}

	.live-mark {
		display: inline-block;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background-color: var(--color-success);
		box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-success) 25%, transparent);
	}

	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip: rect(0 0 0 0);
		white-space: nowrap;
	}

	.select-box {
		flex-shrink: 0;
		width: 15px;
		height: 15px;
		margin: 0 4px 0 2px;
		accent-color: var(--color-accent);
		cursor: pointer;
	}

	.card-main {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 0;
		border: none;
		background: transparent;
		color: inherit;
		cursor: pointer;
		text-align: left;
		font-family: var(--font-sans);
	}

	.os-slot {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
	}

	.kind-mark {
		color: var(--color-text-tertiary, var(--color-text-secondary));
		opacity: 0.8;
	}

	.session-info {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	/* Beside the name when the sidebar is wide enough, under it when not.
	   Neither the name nor the vault is ever cut to make room for the other:
	   "k8s-c…" beside "Kubernetes …" tells you nothing about either. */
	.session-name-row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 2px 6px;
		min-width: 0;
	}

	/* Where the session lives, when the list spans vaults. Same box as the
	   auth badge so the card has one vocabulary for "a fact about this
	   session". */
	.vault-chip {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		flex: 0 0 auto;
		max-width: 100%;
		min-width: 0;
		padding: 1px 6px;
		font-size: var(--text-2xs);
		font-weight: 500;
		color: var(--color-text-secondary);
		background-color: var(--color-surface-hover);
		border-radius: 4px;
	}

	.vault-chip.shared {
		color: #10b981;
		background-color: rgba(16, 185, 129, 0.12);
	}

	.cred-chip.missing {
		color: var(--color-warning, #f59e0b);
		background-color: rgba(245, 158, 11, 0.12);
	}

	.vault-chip-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.session-name {
		flex: 0 1 auto;
		min-width: 0;
		/* Half a step over body text: the name is what the eye scans for. */
		font-size: 0.84375rem;
		font-weight: 500;
		color: var(--color-text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.more-addresses {
		margin-left: 4px;
		padding: 0 4px;
		border-radius: 3px;
		font-size: var(--text-xs);
		color: var(--color-text-secondary);
		background: var(--color-bg-hover, rgba(127, 127, 127, 0.12));
	}

	.session-detail {
		font-size: var(--text-xs);
		color: var(--color-text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		font-family: var(--font-mono, monospace);
	}

	.session-meta {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
	}

	.session-meta .session-detail {
		flex: 0 1 auto;
		min-width: 0;
	}

	/* Tags: the words people file sessions under, small enough not to compete
	   with the name. */
	/* A zero basis: it always fits on the name's line and takes what is left. */
	.tags {
		flex: 1 1 0;
		min-width: 0;
		display: flex;
		gap: 4px;
		overflow: hidden;
		white-space: nowrap;
	}

	.tag-chip {
		flex-shrink: 0;
		max-width: 80px;
		padding: 0 5px;
		border-radius: 999px;
		/* Outlined, not filled: a filled grey chip read as a button. */
		border: 1px solid var(--color-border-strong);
		background: transparent;
		color: var(--color-text-secondary);
		font-size: var(--text-2xs);
		line-height: 15px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.tag-chip.more {
		border-color: transparent;
		background: transparent;
		color: var(--color-text-tertiary);
	}

	.auth-mark {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		color: var(--color-text-tertiary);
		opacity: 0.8;
	}

	/* How a session logs in is the same on most rows, so a lock on each of
	   fifty rows is noise: it shows on the row under the pointer, and stays
	   in the tooltip and the editor. The notes mark stays, as it differs. */
	.session-card:not(:hover):not(:focus-within) .auth-mark:not(.docs-mark) {
		display: none;
	}

	.auth-badge {
		flex-shrink: 0;
		padding: 2px 6px;
		font-size: var(--text-2xs);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--color-text-secondary);
		background-color: var(--color-surface-hover);
		border-radius: 4px;
	}

	/* These buttons were already meant to be hover-only, but `opacity: 0` hides
	   them without giving their width back — they kept reserving ~76px of a
	   ~209px row, which is why session names rendered as "prod…" even on a wide
	   monitor. Taking them out of flow lets the name use the full row at rest
	   and the buttons fade in over the right edge on hover. */
	.session-actions {
		position: absolute;
		right: 6px;
		top: 50%;
		transform: translateY(-50%);
		display: flex;
		align-items: center;
		gap: 2px;
		padding-left: 20px;
		opacity: 0;
		pointer-events: none;
		/* Fade the name out behind the buttons rather than letting the two overlap. */
		background: linear-gradient(
			to right,
			transparent 0,
			var(--color-bg-secondary) 20px
		);
		transition: opacity var(--duration-default) var(--ease-default);
	}

	.session-card:hover .session-actions,
	.session-card:focus-within .session-actions {
		opacity: 1;
		pointer-events: auto;
	}

	/* No hover on touch, so the actions have to stay put and visible. */
	@media (pointer: coarse) {
		.session-actions {
			position: static;
			transform: none;
			padding-left: 0;
			background: none;
			opacity: 1;
			pointer-events: auto;
		}
	}

	.action-btn {
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
		transition:
			background-color var(--duration-default) var(--ease-default),
			color var(--duration-default) var(--ease-default);
	}

	.action-btn:hover {
		background-color: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.action-btn:active {
		transform: scale(0.92);
	}

	.connect-btn:hover {
		color: var(--color-accent);
	}

	.delete-btn:hover {
		color: var(--color-danger);
	}

	/* A phone: the row is the button. Tapping it connects; a long press opens
	   the same menu a right-click does, with Edit and Delete. The drag handle,
	   the three buttons and the auth badge would otherwise leave the name two
	   letters wide ("Pro…"), and a delete button under a thumb is one slip from
	   losing a session. */
	@media (max-width: 700px) {
		.drag-handle,
		.session-actions,
		.auth-badge,
		.auth-mark {
			display: none;
		}

		.session-card {
			padding-left: 4px;
		}
	}

	.session-card.unavailable {
		opacity: 0.55;
	}

	.session-card.unavailable .card-main {
		cursor: not-allowed;
	}

	.session-card.unavailable .connect-btn {
		cursor: not-allowed;
		opacity: 0.3;
	}

	.unavailable-text {
		color: var(--color-text-secondary);
	}


	/* Double-click to open: the row a single click picked stays marked, so
	   it is clear something happened. */
	.session-card.dbl:focus-within {
		background-color: var(--color-surface-hover);
	}

	/* The session of the tab on screen: picked, as a selected row looks.
	   It follows the tab bar. */
	.session-card.current,
	.session-card.current:hover,
	.session-card.current:focus-within {
		background-color: var(--color-accent-subtle);
		box-shadow: inset 2px 0 0 var(--color-accent);
	}

	/* Compact layout: the icon and the name only, as a list of many servers
	   wants; the rest is in the row's tooltip. Text and the row's hit area
	   stay the size they were. */
	:global(html.compact) .session-card {
		padding-top: 2px;
		padding-bottom: 2px;
	}

	:global(html.compact) .session-name-row {
		flex-wrap: nowrap;
	}

	.names-only .tags,
	.names-only .vault-chip,
	.names-only .session-detail,
	.names-only .session-meta,
	.names-only .auth-mark,
	.names-only .auth-badge {
		display: none;
	}

	.fav-mark {
		flex-shrink: 0;
		color: var(--color-accent);
	}

	.mark-tip {
		display: inline-flex;
		flex-shrink: 0;
	}

	.route-mark {
		color: var(--color-text-tertiary);
	}
</style>
