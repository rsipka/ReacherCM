<script lang="ts">
	/**
	 * Saved credentials: a login kept once and used by many sessions in the
	 * same vault, linked from a session's editor or set on a folder for the
	 * sessions inside. Shown as a tab of the sessions sidebar.
	 */
	import Modal from '#lib/components/shared/Modal.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import KeyPicker from './KeyPicker.svelte';
	import PasswordHistory from './PasswordHistory.svelte';
	import SecretRefField from '#lib/components/pwmanager/SecretRefField.svelte';
	import type { SecretRef } from '#lib/ipc/pwmanager.js';
	import { providerName } from '#lib/state/pwmanager.svelte.js';
	import FaIcon from '#lib/components/shared/FaIcon.svelte';
	import FolderIcon from './FolderIcon.svelte';
	import FolderIconPicker from './FolderIconPicker.svelte';
	import ContextMenuBackdrop from '#lib/components/shared/ContextMenuBackdrop.svelte';
	import { faPlus, faFolderPlus, faChevronRight } from '@fortawesome/free-solid-svg-icons';
	import { positionMenu } from '#lib/utils/positionMenu.js';
	import { longPress } from '#lib/utils/longPress.js';
	import { isMobile } from '#lib/platform.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { ALL_VAULTS, vaultState, type VaultFilter } from '#lib/state/vault.svelte.js';
	import type { AuthMethod, Folder, SessionConfig } from '#lib/ipc/sessions.js';
	import {
		savedCredentialList,
		savedCredentialCreate,
		savedCredentialUpdate,
		savedCredentialDelete,
		savedCredentialMove,
		credentialFolderList,
		credentialFolderCreate,
		credentialFolderRename,
		credentialFolderMove,
		credentialFolderSetIcon,
		credentialFolderDelete,
		credentialFolderPath,
		type SavedCredential,
		type CredentialFolder
	} from '#lib/ipc/savedCredentials.js';

	interface Props {
		/** What the sidebar shows: all vaults, this device's own (null), or one. */
		vaultFilter: VaultFilter;
		/** Where a new credential goes. */
		targetVaultId: string | null;
		sessions: SessionConfig[];
		folders: Folder[];
		/** Something the session list shows changed. */
		onchange?: () => void;
	}

	let { vaultFilter, targetVaultId, sessions, folders, onchange }: Props = $props();

	let all = $state<SavedCredential[]>([]);
	let loading = $state(true);
	let error = $state<string | undefined>();

	let credFolders = $state<CredentialFolder[]>([]);

	async function load(): Promise<void> {
		try {
			[all, credFolders] = await Promise.all([savedCredentialList(), credentialFolderList()]);
			error = undefined;
		} catch (e) {
			error = String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void load();
	});

	const inView = (item: { vault_id?: string | null }) => vaultFilter === ALL_VAULTS || (item.vault_id ?? null) === (vaultFilter ?? null);
	const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

	let shown = $derived(all.filter(inView).sort(byName));
	let shownFolders = $derived(credFolders.filter(inView).sort(byName));

	// ---- Folders --------------------------------------------------------------

	/** The folder a credential shows in: its own, when that is in its vault. */
	function folderOf(c: SavedCredential): CredentialFolder | undefined {
		const f = c.folder_id ? credFolders.find((x) => x.id === c.folder_id) : undefined;
		return f && (f.vault_id ?? null) === (c.vault_id ?? null) ? f : undefined;
	}

	/** The folder a folder shows under: its parent, when in its vault and not in a cycle. */
	function parentOf(f: CredentialFolder): CredentialFolder | undefined {
		const seen = new Set<string>([f.id]);
		let p = f.parent_id ? credFolders.find((x) => x.id === f.parent_id) : undefined;
		if (!p || (p.vault_id ?? null) !== (f.vault_id ?? null)) return undefined;
		// A cycle in stored folders: show this one at the top.
		for (let up: CredentialFolder | undefined = p; up; up = up.parent_id ? credFolders.find((x) => x.id === up!.parent_id) : undefined) {
			if (seen.has(up.id)) return undefined;
			seen.add(up.id);
		}
		return p;
	}

	const COLLAPSED_KEY = 'reachercm.credentialFolders.collapsed';
	function loadCollapsed(): Set<string> {
		try {
			return new Set(JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '[]') as string[]);
		} catch {
			return new Set();
		}
	}
	let collapsed = $state<Set<string>>(loadCollapsed());
	function toggleFolder(id: string): void {
		const next = new Set(collapsed);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		collapsed = next;
		try {
			localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
		} catch {
			/* not kept: fine */
		}
	}

	type Row =
		| { kind: 'folder'; folder: CredentialFolder; depth: number; count: number }
		| { kind: 'cred'; cred: SavedCredential; depth: number };

	/** How many credentials are in a folder, at any depth. */
	function countIn(id: string): number {
		const direct = shown.filter((c) => folderOf(c)?.id === id).length;
		return direct + shownFolders.filter((f) => parentOf(f)?.id === id).reduce((n, f) => n + countIn(f.id), 0);
	}

	/** The list as shown: folders first, then credentials, at each level. */
	let rows = $derived.by(() => {
		const out: Row[] = [];
		const walk = (parent: string | null, depth: number) => {
			for (const f of shownFolders.filter((x) => (parentOf(x)?.id ?? null) === parent)) {
				out.push({ kind: 'folder', folder: f, depth, count: countIn(f.id) });
				if (!collapsed.has(f.id)) walk(f.id, depth + 1);
			}
			for (const c of shown.filter((x) => (folderOf(x)?.id ?? null) === parent)) out.push({ kind: 'cred', cred: c, depth });
		};
		walk(null, 0);
		return out;
	});

	/** The folders an item of `vault` can go into, with their path. */
	function targetsIn(vault: string | null, except?: CredentialFolder): { folder: CredentialFolder; path: string }[] {
		const below = (f: CredentialFolder): boolean => {
			for (let up: CredentialFolder | undefined = f; up; up = parentOf(up)) if (up.id === except?.id) return true;
			return false;
		};
		return credFolders
			.filter((f) => (f.vault_id ?? null) === vault && !(except && below(f)))
			.map((f) => ({ folder: f, path: credentialFolderPath(f.id, credFolders) }))
			.sort((a, b) => a.path.localeCompare(b.path));
	}

	async function run(action: () => Promise<unknown>): Promise<void> {
		try {
			await action();
			error = undefined;
		} catch (e) {
			error = String(e);
		}
		await load();
	}

	// Name dialog: a new folder or a rename.
	let nameDialog = $state<{ folder?: CredentialFolder; parent?: CredentialFolder | null; vault: string | null } | null>(null);
	let folderName = $state('');
	let nameBusy = $state(false);

	function newFolder(parent: CredentialFolder | null): void {
		menu = null;
		nameDialog = { parent, vault: parent ? (parent.vault_id ?? null) : targetVaultId };
		folderName = '';
	}

	function renameFolder(f: CredentialFolder): void {
		menu = null;
		nameDialog = { folder: f, vault: f.vault_id ?? null };
		folderName = f.name;
	}

	async function saveName(): Promise<void> {
		const d = nameDialog;
		if (!d || !folderName.trim() || nameBusy) return;
		nameBusy = true;
		await run(async () => {
			if (d.folder) await credentialFolderRename(d.folder.id, folderName);
			else {
				const created = await credentialFolderCreate(folderName, d.parent?.id ?? null, d.vault);
				if (d.parent && collapsed.has(d.parent.id)) toggleFolder(d.parent.id);
				return created;
			}
		});
		nameBusy = false;
		nameDialog = null;
	}

	// Move dialog: a credential or a folder to another folder of its vault.
	let moving = $state<{ cred?: SavedCredential; folder?: CredentialFolder } | null>(null);

	function moveTo(target: CredentialFolder | null): void {
		const m = moving;
		moving = null;
		if (!m) return;
		if (m.cred) void run(() => savedCredentialMove([m.cred!.id], target?.id ?? null));
		else if (m.folder) void run(() => credentialFolderMove(m.folder!.id, target?.id ?? null));
	}

	// Icon picker.
	let iconFor = $state<CredentialFolder | null>(null);

	// Delete folder.
	let deletingFolder = $state<CredentialFolder | null>(null);

	// ---- Menu -----------------------------------------------------------------

	let menu = $state<{ x: number; y: number; folder?: CredentialFolder; cred?: SavedCredential } | null>(null);
	let sheetMenu = $state(false);

	function openMenu(e: MouseEvent, item: { folder?: CredentialFolder; cred?: SavedCredential }): void {
		e.preventDefault();
		e.stopPropagation();
		sheetMenu = isMobile() && window.innerWidth < 640;
		menu = { x: e.clientX, y: e.clientY, ...item };
	}

	/** Run a menu item, then close the menu: the item reads the menu's row. */
	function fromMenu(action: () => void): void {
		action();
		menu = null;
	}

	// ---- Drag to a folder (mouse) ---------------------------------------------

	let drag = $state<{ cred?: SavedCredential; folder?: CredentialFolder } | null>(null);
	/** Where a drop goes: a folder id, '' for the top, undefined for nowhere. */
	let dropOn = $state<string | undefined>(undefined);
	let suppressClick = false;

	function dragVault(d: { cred?: SavedCredential; folder?: CredentialFolder }): string | null {
		return (d.cred ?? d.folder)!.vault_id ?? null;
	}

	function startDrag(e: PointerEvent, item: { cred?: SavedCredential; folder?: CredentialFolder }): void {
		if (e.pointerType !== 'mouse' || e.button !== 0) return;
		const x0 = e.clientX;
		const y0 = e.clientY;
		let started = false;
		const allowed = new Set(targetsIn(dragVault(item), item.folder).map((t) => t.folder.id));
		const onMove = (me: PointerEvent) => {
			if (!started) {
				if (Math.hypot(me.clientX - x0, me.clientY - y0) < 6) return;
				started = true;
				drag = item;
			}
			const el = document.elementFromPoint(me.clientX, me.clientY)?.closest('[data-cred-drop]') as HTMLElement | null;
			const id = el?.dataset.credDrop;
			dropOn = id === undefined ? undefined : id === '' || allowed.has(id) ? id : undefined;
		};
		const onUp = () => {
			window.removeEventListener('pointermove', onMove);
			window.removeEventListener('pointerup', onUp);
			if (!started) return;
			suppressClick = true;
			setTimeout(() => (suppressClick = false), 0);
			const target = dropOn;
			const d = drag;
			drag = null;
			dropOn = undefined;
			if (!d || target === undefined) return;
			const folderId = target === '' ? null : target;
			if (d.cred && (folderOf(d.cred)?.id ?? null) !== folderId) void run(() => savedCredentialMove([d.cred!.id], folderId));
			if (d.folder && (parentOf(d.folder)?.id ?? null) !== folderId && d.folder.id !== folderId) void run(() => credentialFolderMove(d.folder!.id, folderId));
		};
		window.addEventListener('pointermove', onMove);
		window.addEventListener('pointerup', onUp);
	}

	/** Sessions whose login comes from a credential, directly or from a folder. */
	function usedBy(id: string): SessionConfig[] {
		return sessions.filter((s) => s.credential?.id === id && !s.credential.missing);
	}

	function foldersUsing(id: string): Folder[] {
		return folders.filter((f) => f.credential_id === id);
	}

	/** The password manager a credential's secret comes from, if it names an entry. */
	function managerOf(c: SavedCredential): string {
		const a = c.auth_method;
		const r = a.type === 'Password' ? a.password_ref : a.type === 'Key' ? a.passphrase_ref : null;
		return r ? providerName(r.provider) : '';
	}

	function vaultName(c: { vault_id?: string | null }): string {
		return c.vault_id ? (vaultState.vaults.get(c.vault_id)?.name ?? t('vault.private')) : t('vault.private');
	}

	// ---- Editor -------------------------------------------------------------

	let editorOpen = $state(false);
	let editing = $state<SavedCredential | null>(null);
	let name = $state('');
	let username = $state('');
	let domain = $state('');
	let authType = $state<'Password' | 'Key' | 'Agent'>('Password');
	let password = $state('');
	let keyPath = $state('');
	let keyId = $state('');
	let passphrase = $state('');
	// A password manager entry in place of the typed password or passphrase.
	let passwordRef = $state<SecretRef | null>(null);
	let passphraseRef = $state<SecretRef | null>(null);
	let saving = $state(false);
	let editorError = $state<string | undefined>();
	/** The folder it goes in ('' for the top). */
	let editFolder = $state('');
	/** The vault the credential is or will be in, for the folder choice. */
	let editVault = $state<string | null>(null);

	function openEditor(c: SavedCredential | null, inFolder: CredentialFolder | null = null): void {
		menu = null;
		editing = c;
		editVault = c ? (c.vault_id ?? null) : inFolder ? (inFolder.vault_id ?? null) : targetVaultId;
		editFolder = c ? (folderOf(c)?.id ?? '') : (inFolder?.id ?? '');
		name = c?.name ?? '';
		username = c?.username ?? '';
		domain = c?.domain ?? '';
		const a = c?.auth_method;
		authType = a?.type ?? 'Password';
		password = a?.password ?? '';
		keyPath = a?.path ?? '';
		keyId = a?.key_id ?? '';
		passphrase = a?.passphrase ?? '';
		passwordRef = (a?.type === 'Password' && a.password_ref) || null;
		passphraseRef = (a?.type === 'Key' && a.passphrase_ref) || null;
		editorError = undefined;
		editorOpen = true;
	}

	let canSave = $derived(name.trim().length > 0 && username.trim().length > 0 && !saving);

	async function save(): Promise<void> {
		if (!canSave) return;
		const authMethod: AuthMethod =
			authType === 'Password'
				? { type: 'Password', password: passwordRef ? undefined : password || undefined, password_ref: passwordRef ?? undefined }
				: authType === 'Key'
					? {
							type: 'Key',
							path: keyId ? undefined : keyPath.trim() || undefined,
							key_id: keyId || undefined,
							passphrase: keyId || passphraseRef ? undefined : passphrase || undefined,
							passphrase_ref: keyId ? undefined : (passphraseRef ?? undefined)
						}
					: { type: 'Agent' };
		saving = true;
		editorError = undefined;
		try {
			if (editing) {
				await savedCredentialUpdate({ ...editing, name: name.trim(), username: username.trim(), domain: domain.trim() || null, auth_method: authMethod, folder_id: editFolder || null });
			} else {
				await savedCredentialCreate({ name, username, domain, authMethod, vaultId: editVault, folderId: editFolder || null });
			}
			editorOpen = false;
			await load();
			onchange?.();
		} catch (e) {
			editorError = String(e);
		} finally {
			saving = false;
		}
	}

	// ---- Delete -------------------------------------------------------------

	let deleting = $state<SavedCredential | null>(null);
	let deleteBusy = $state(false);

	async function confirmDelete(): Promise<void> {
		if (!deleting) return;
		deleteBusy = true;
		try {
			await savedCredentialDelete(deleting.id);
			deleting = null;
			await load();
			onchange?.();
		} catch (e) {
			error = String(e);
			deleting = null;
		} finally {
			deleteBusy = false;
		}
	}
</script>

<div class="cred-list">
	<div class="cred-head">
		<p class="cred-intro">{t('credential.intro')}</p>
		<button class="add-btn" title={t('credential.new_folder')} aria-label={t('credential.new_folder')} onclick={() => newFolder(null)}>
			<FaIcon icon={faFolderPlus} size={13} />
		</button>
		<button class="add-btn" title={t('credential.new')} aria-label={t('credential.new')} onclick={() => openEditor(null)}>
			<FaIcon icon={faPlus} size={13} />
		</button>
	</div>

	{#if error}
		<p class="cred-error" role="alert">{error}</p>
	{/if}

	{#if loading}
		<div class="cred-empty"><span class="spinner"></span></div>
	{:else if rows.length === 0}
		<p class="cred-empty">{t('credential.none')}</p>
	{:else}
		<div class="cred-scroll" class:dragging={drag !== null}>
			{#each rows as row (row.kind === 'folder' ? `f:${row.folder.id}` : `c:${row.cred.id}`)}
				{#if row.kind === 'folder'}
					{@const f = row.folder}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="cred-folder"
						class:drop-active={drag !== null && dropOn === f.id}
						data-cred-drop={f.id}
						style:padding-left="{4 + Math.min(row.depth, 8) * 14}px"
						oncontextmenu={(e) => openMenu(e, { folder: f })}
						onpointerdown={(e) => startDrag(e, { folder: f })}
						use:longPress
					>
						<button
							class="folder-main"
							aria-expanded={!collapsed.has(f.id)}
							onclick={() => { if (!suppressClick) toggleFolder(f.id); }}
						>
							<span class="chevron" class:open={!collapsed.has(f.id)}><FaIcon icon={faChevronRight} size={9} /></span>
							<FolderIcon icon={f.icon} color={f.color} size={13} />
							<span class="folder-name">{f.name}</span>
							{#if vaultFilter === ALL_VAULTS && row.depth === 0}<span class="folder-vault">{vaultName(f)}</span>{/if}
							<span class="folder-count">{row.count}</span>
						</button>
						<button class="row-more" title={t('credential.more')} aria-label={t('credential.more')} onclick={(e) => openMenu(e, { folder: f })}>⋯</button>
					</div>
				{:else}
					{@const c = row.cred}
					{@const used = usedBy(c.id).length}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="cred-row"
						class:drag-source={drag?.cred?.id === c.id}
						data-cred-drop={folderOf(c)?.id ?? ''}
						style:padding-left="{Math.min(row.depth, 8) * 14 + (row.depth > 0 ? 10 : 0)}px"
						oncontextmenu={(e) => openMenu(e, { cred: c })}
						onpointerdown={(e) => startDrag(e, { cred: c })}
						use:longPress
					>
						<button class="cred-main" onclick={() => { if (!suppressClick) openEditor(c); }} title={t('common.edit')}>
							<span class="cred-name">{c.name}</span>
							<span class="cred-detail">
								{c.domain ? `${c.domain}\\` : ''}{c.username} · {t(`credential.auth_${c.auth_method.type.toLowerCase()}`)}{managerOf(c) ? ` · ${managerOf(c)}` : ''}
								{#if vaultFilter === ALL_VAULTS} · {vaultName(c)}{/if}
							</span>
							<span class="cred-usage">{t('credential.used_by', { count: String(used) })}</span>
						</button>
						<button class="cred-delete" onclick={() => (deleting = c)} title={t('common.delete')} aria-label={t('common.delete')}>×</button>
					</div>
				{/if}
			{/each}
			{#if drag !== null}
				<div class="top-drop" class:drop-active={dropOn === ''} data-cred-drop="">{t('credential.folder_top')}</div>
			{/if}
		</div>
	{/if}
</div>

{#if menu}
	<ContextMenuBackdrop onclose={() => (menu = null)} />
	<div class="context-menu" class:pop-in={!sheetMenu} class:context-sheet={sheetMenu} role="menu" use:positionMenu={{ x: menu?.x ?? 0, y: menu?.y ?? 0 }}>
		{#if sheetMenu}<div class="context-sheet-title">{menu?.folder?.name ?? menu?.cred?.name}</div>{/if}
		{#if menu?.folder}
			{@const f = menu.folder}
			<button class="context-item" role="menuitem" onclick={() => openEditor(null, f)}>{t('credential.new_here')}</button>
			<button class="context-item" role="menuitem" onclick={() => newFolder(f)}>{t('folder.new_subfolder')}</button>
			<button class="context-item" role="menuitem" onclick={() => renameFolder(f)}>{t('folder.rename')}</button>
			<button class="context-item context-icon-item" role="menuitem" onclick={() => fromMenu(() => (iconFor = f))}><span>{t('folder.icon')}</span><FolderIcon icon={f.icon} color={f.color} /></button>
			<button class="context-item" role="menuitem" onclick={() => fromMenu(() => (moving = { folder: f }))}>{t('credential.move_to')}</button>
			<div class="context-sep"></div>
			<button class="context-item context-danger" role="menuitem" onclick={() => fromMenu(() => (deletingFolder = f))}>{t('folder.delete')}</button>
		{:else if menu?.cred}
			{@const c = menu.cred}
			<button class="context-item" role="menuitem" onclick={() => openEditor(c)}>{t('common.edit')}</button>
			<button class="context-item" role="menuitem" onclick={() => fromMenu(() => (moving = { cred: c }))}>{t('credential.move_to')}</button>
			<div class="context-sep"></div>
			<button class="context-item context-danger" role="menuitem" onclick={() => fromMenu(() => (deleting = c))}>{t('common.delete')}</button>
		{/if}
	</div>
{/if}

<Modal open={nameDialog !== null} onclose={() => (nameDialog = null)} title={nameDialog?.folder ? t('credential.rename_folder') : t('credential.new_folder')} maxWidth="380px">
	<form class="cred-form" onsubmit={(e) => { e.preventDefault(); void saveName(); }}>
		<Input label={t('credential.folder_name')} bind:value={folderName} placeholder={t('credential.folder_name_placeholder')} disabled={nameBusy} />
		{#if nameDialog?.parent}
			<p class="cred-hint">{t('credential.folder_in', { folder: credentialFolderPath(nameDialog.parent.id, credFolders) })}</p>
		{/if}
	</form>
	{#snippet actions()}
		<Button variant="ghost" onclick={() => (nameDialog = null)}>{t('common.cancel')}</Button>
		<Button variant="primary" disabled={!folderName.trim() || nameBusy} onclick={() => void saveName()}>{t('common.save')}</Button>
	{/snippet}
</Modal>

<Modal open={moving !== null} onclose={() => (moving = null)} title={t('credential.move_title', { name: moving?.cred?.name ?? moving?.folder?.name ?? '' })} maxWidth="380px">
	{#if moving}
		{@const m = moving ?? {}}
		{@const current = m.cred ? (folderOf(m.cred)?.id ?? null) : m.folder ? (parentOf(m.folder)?.id ?? null) : null}
		<div class="move-list" role="listbox">
			<button class="move-item" class:current={current === null} role="option" aria-selected={current === null} onclick={() => moveTo(null)}>
				<span class="move-top">⤒</span>{t('credential.folder_top')}
			</button>
			{#each targetsIn((m.cred ?? m.folder)?.vault_id ?? null, m.folder) as target (target.folder.id)}
				<button class="move-item" class:current={current === target.folder.id} role="option" aria-selected={current === target.folder.id} onclick={() => moveTo(target.folder)}>
					<FolderIcon icon={target.folder.icon} color={target.folder.color} />{target.path}
				</button>
			{/each}
		</div>
	{/if}
	{#snippet actions()}
		<Button variant="ghost" onclick={() => (moving = null)}>{t('common.cancel')}</Button>
	{/snippet}
</Modal>

<FolderIconPicker
	open={iconFor !== null}
	name={iconFor?.name ?? ''}
	icon={iconFor?.icon ?? null}
	color={iconFor?.color ?? null}
	onclose={() => (iconFor = null)}
	onsave={(icon, color) => {
		const f = iconFor;
		iconFor = null;
		if (f) void run(() => credentialFolderSetIcon(f.id, icon, color));
	}}
/>

<Modal open={deletingFolder !== null} onclose={() => (deletingFolder = null)} title={t('folder.delete_title', { name: deletingFolder?.name ?? '' })} maxWidth="420px">
	{#if deletingFolder}
		{@const f = deletingFolder ?? { id: '', name: '' }}
		{@const parent = deletingFolder ? parentOf(deletingFolder) : undefined}
		<p class="cred-hint">{t('folder.delete_message')}</p>
		<p class="cred-hint">
			{t('credential.folder_delete_note', {
				credentials: String(all.filter((c) => folderOf(c)?.id === f.id).length),
				folders: String(credFolders.filter((x) => parentOf(x)?.id === f.id).length),
				parent: parent ? parent.name : t('credential.folder_top')
			})}
		</p>
	{/if}
	{#snippet actions()}
		<Button variant="ghost" onclick={() => (deletingFolder = null)}>{t('common.cancel')}</Button>
		<Button variant="danger" onclick={() => { const f = deletingFolder; deletingFolder = null; if (f) void run(() => credentialFolderDelete(f.id)); }}>{t('common.delete')}</Button>
	{/snippet}
</Modal>

<Modal open={editorOpen} onclose={() => (editorOpen = false)} title={editing ? t('credential.edit') : t('credential.new')} maxWidth="440px">
	<form class="cred-form" onsubmit={(e) => { e.preventDefault(); void save(); }}>
		<Input label={t('credential.name')} bind:value={name} placeholder={t('credential.name_placeholder')} disabled={saving} />
		<Input label={t('session.username')} bind:value={username} placeholder="root" disabled={saving} />
		<div class="auth-toggle" role="radiogroup" aria-label={t('session.auth_method')}>
			{#each ['Password', 'Key', 'Agent'] as const as kind (kind)}
				<button type="button" class="auth-btn" class:active={authType === kind} disabled={saving} onclick={() => (authType = kind)}>
					{t(`credential.auth_${kind.toLowerCase()}`)}
				</button>
			{/each}
		</div>
		{#if authType === 'Password'}
			<SecretRefField label={t('session.password_optional')} bind:value={password} bind:secretRef={passwordRef} placeholder={t('session.stored_encrypted')} disabled={saving} onpick={(e) => { if (!username.trim() && e.username) username = e.username; }} />
			{#if passwordRef}<p class="cred-hint">{t('credential.entry_hint')}</p>{/if}
			{#if editing && !passwordRef}
				<PasswordHistory owner={{ credential: editing.id }} disabled={saving} onrestore={(p) => (password = p)} />
			{/if}
		{:else if authType === 'Key'}
			<KeyPicker bind:path={keyPath} bind:keyId disabled={saving} />
			{#if !keyId}
				<SecretRefField label={t('session.passphrase_optional')} bind:value={passphrase} bind:secretRef={passphraseRef} placeholder={t('session.stored_encrypted')} disabled={saving} />
				{#if passphraseRef}<p class="cred-hint">{t('credential.entry_hint')}</p>{/if}
			{/if}
		{/if}
		<Input label={t('credential.domain')} bind:value={domain} placeholder="CORP" disabled={saving} />
		{#if targetsIn(editVault).length > 0}
			<div class="field">
				<label class="field-label" for="cred-folder">{t('credential.folder')}</label>
				<select id="cred-folder" class="field-select" bind:value={editFolder} disabled={saving}>
					<option value="">{t('credential.folder_top')}</option>
					{#each targetsIn(editVault) as target (target.folder.id)}
						<option value={target.folder.id}>{target.path}</option>
					{/each}
				</select>
			</div>
		{/if}
		<p class="cred-hint">{editing ? t('credential.edit_hint', { count: String(usedBy(editing.id).length) }) : t('credential.vault_hint')}</p>
		{#if editorError}<p class="cred-error" role="alert">{editorError}</p>{/if}
	</form>
	{#snippet actions()}
		<Button variant="ghost" onclick={() => (editorOpen = false)}>{t('common.cancel')}</Button>
		<Button variant="primary" disabled={!canSave} onclick={() => void save()}>{t('common.save')}</Button>
	{/snippet}
</Modal>

<Modal open={deleting !== null} onclose={() => (deleting = null)} title={t('credential.delete_title', { name: deleting?.name ?? '' })} maxWidth="420px">
	{#if deleting}
		{@const users = usedBy(deleting.id)}
		{@const inFolders = foldersUsing(deleting.id)}
		<p class="cred-hint">
			{users.length + inFolders.length > 0
				? t('credential.delete_used', { sessions: String(users.length), folders: String(inFolders.length) })
				: t('credential.delete_unused')}
		</p>
		{#if users.length > 0}
			<ul class="cred-users">
				{#each users.slice(0, 8) as s (s.id)}<li>{s.name}</li>{/each}
				{#if users.length > 8}<li>…</li>{/if}
			</ul>
		{/if}
	{/if}
	{#snippet actions()}
		<Button variant="ghost" onclick={() => (deleting = null)}>{t('common.cancel')}</Button>
		<Button variant="danger" disabled={deleteBusy} onclick={() => void confirmDelete()}>{t('common.delete')}</Button>
	{/snippet}
</Modal>

<style>
	.cred-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-height: 0;
		flex: 1;
	}

	.cred-head {
		display: flex;
		align-items: flex-start;
		gap: 6px;
	}

	.cred-intro {
		flex: 1;
		margin: 0;
		font-size: var(--text-2xs);
		color: var(--color-text-secondary);
		line-height: 1.4;
	}

	.add-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 30px;
		height: 30px;
		flex-shrink: 0;
		padding: 0;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: transparent;
		color: var(--color-accent);
		cursor: pointer;
	}

	.add-btn:hover {
		background-color: rgba(0, 122, 255, 0.1);
	}

	.cred-scroll {
		display: flex;
		flex-direction: column;
		gap: 2px;
		overflow-y: auto;
		min-height: 0;
	}

	.cred-row {
		display: flex;
		align-items: stretch;
		border-radius: 6px;
	}

	.cred-row:hover {
		background-color: var(--color-surface-hover);
	}

	.cred-main {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 1px;
		flex: 1;
		min-width: 0;
		padding: 6px 8px;
		border: none;
		background: transparent;
		text-align: left;
		cursor: pointer;
		color: inherit;
	}

	.cred-name {
		font-size: var(--text-sm);
		font-weight: 500;
		color: var(--color-text-primary);
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.cred-detail,
	.cred-usage {
		font-size: var(--text-2xs);
		color: var(--color-text-secondary);
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.cred-delete {
		flex-shrink: 0;
		width: 26px;
		border: none;
		background: transparent;
		color: var(--color-text-secondary);
		font-size: 16px;
		cursor: pointer;
		opacity: 0;
	}

	.cred-row:hover .cred-delete,
	.cred-delete:focus-visible {
		opacity: 1;
	}

	.cred-delete:hover {
		color: var(--color-danger, #ef4444);
	}

	.cred-empty {
		margin: 12px 0;
		text-align: center;
		font-size: var(--text-xs);
		color: var(--color-text-secondary);
	}

	.cred-error {
		margin: 0;
		font-size: var(--text-xs);
		color: var(--color-danger, #ef4444);
	}

	.cred-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.auth-toggle {
		display: flex;
		gap: 4px;
	}

	.auth-btn {
		flex: 1;
		padding: 6px 8px;
		font-size: var(--text-xs);
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.auth-btn.active {
		border-color: var(--color-accent);
		color: var(--color-accent);
		background-color: rgba(0, 122, 255, 0.08);
	}

	.cred-hint {
		margin: 0;
		font-size: var(--text-2xs);
		color: var(--color-text-secondary);
		line-height: 1.4;
	}

	.cred-folder {
		display: flex;
		align-items: center;
		border-radius: 6px;
		user-select: none;
	}

	.cred-folder:hover {
		background-color: var(--color-surface-hover);
	}

	.cred-folder.drop-active,
	.top-drop.drop-active {
		background-color: rgba(0, 122, 255, 0.12);
		box-shadow: inset 0 0 0 1px var(--color-accent);
	}

	.folder-main {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1;
		min-width: 0;
		padding: 6px 4px;
		border: none;
		background: transparent;
		color: var(--color-text-primary);
		font-size: var(--text-sm);
		font-weight: 600;
		text-align: left;
		cursor: pointer;
	}

	.chevron {
		display: inline-flex;
		width: 10px;
		color: var(--color-text-secondary);
		transition: transform 0.12s ease;
	}

	.chevron.open {
		transform: rotate(90deg);
	}

	.folder-name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.folder-vault,
	.folder-count {
		flex-shrink: 0;
		font-size: var(--text-2xs);
		font-weight: 400;
		color: var(--color-text-secondary);
	}

	.row-more {
		flex-shrink: 0;
		width: 26px;
		border: none;
		background: transparent;
		color: var(--color-text-secondary);
		font-size: 14px;
		cursor: pointer;
		opacity: 0;
	}

	.cred-folder:hover .row-more,
	.row-more:focus-visible {
		opacity: 1;
	}

	@media (hover: none) {
		.row-more {
			opacity: 1;
		}
	}

	.cred-scroll.dragging {
		cursor: grabbing;
	}

	.cred-row.drag-source {
		opacity: 0.5;
	}

	.top-drop {
		margin-top: 4px;
		padding: 8px;
		border: 1px dashed var(--color-border);
		border-radius: 6px;
		text-align: center;
		font-size: var(--text-2xs);
		color: var(--color-text-secondary);
	}

	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.field-label {
		font-size: var(--text-2xs);
		color: var(--color-text-secondary);
	}

	.field-select {
		padding: 7px 8px;
		font-size: var(--text-sm);
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background-color: var(--color-bg-secondary, transparent);
		color: var(--color-text-primary);
	}

	.move-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
		max-height: 320px;
		overflow-y: auto;
	}

	.move-item {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 7px 10px;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: var(--color-text-primary);
		font-size: var(--text-sm);
		text-align: left;
		cursor: pointer;
	}

	.move-item:hover {
		background-color: var(--color-surface-hover);
	}

	.move-item.current {
		color: var(--color-accent);
		font-weight: 600;
	}

	.move-top {
		width: 12px;
		text-align: center;
	}

	.context-menu {
		position: fixed;
		min-width: 180px;
		max-width: 280px;
		padding: 4px 0;
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: 8px;
		box-shadow: var(--shadow-elevated);
		z-index: 1000;
	}

	.context-menu.context-sheet {
		left: 0 !important;
		right: 0;
		top: auto !important;
		bottom: 0;
		min-width: 0;
		max-width: none;
		max-height: 55dvh;
		overflow-y: auto;
		padding: 6px max(4px, var(--safe-right)) max(10px, var(--safe-bottom)) max(4px, var(--safe-left));
		border-width: 1px 0 0;
		border-radius: 14px 14px 0 0;
	}

	.context-sheet-title {
		padding: 6px 12px 5px;
		font-size: var(--text-sm);
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		border-bottom: 1px solid var(--color-border);
		margin-bottom: 4px;
	}

	.context-item {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 6px 12px;
		border: none;
		background: transparent;
		color: var(--color-text-primary);
		font-size: var(--text-sm);
		text-align: left;
		cursor: pointer;
	}

	.context-sheet .context-item {
		min-height: 38px;
	}

	.context-item:hover,
	.context-item:focus-visible {
		background-color: var(--color-surface-hover);
	}

	.context-icon-item span {
		flex: 1;
	}

	.context-danger {
		color: var(--color-danger, #ef4444);
	}

	.context-sep {
		height: 1px;
		margin: 4px 0;
		background-color: var(--color-border);
	}

	.cred-users {
		margin: 8px 0 0;
		padding-left: 18px;
		font-size: var(--text-xs);
		color: var(--color-text-primary);
	}
</style>
