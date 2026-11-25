<script lang="ts">
	import PasswordField from '#lib/components/shared/PasswordField.svelte';
	import { openHistoryPanel } from '#lib/state/history.svelte.js';
	import QuickConnect, { type QuickConnectPrefill } from './QuickConnect.svelte';
	import FaIcon from '#lib/components/shared/FaIcon.svelte';
	import {
		faPlus,
		faBolt,
		faDisplay,
		faServer,
		faFolderPlus,
		faFileImport,
		faMagnifyingGlass,
		faXmark,
		faListCheck,
		faFolder,
		faFolderOpen,
		faPen,
		faCheckDouble,
		faAnglesUp,
		faAnglesDown,
		faArrowDownWideShort,
		faFilter,
		faStar,
		faTrashCan,
		faCheck,
		faChevronLeft,
		faChevronRight,
		faShareNodes,
		faRightToBracket
	} from '@fortawesome/free-solid-svg-icons';
	import { registerSessionActions, shortcutLabel, actionLabel } from '#lib/state/shortcuts.svelte.js';
	import { pendingSessionAction, takeSessionAction } from '#lib/state/palette.svelte.js';
	import { parseAddress } from '#lib/utils/address.js';
	import { noteSessionUsed, forgetSession, recentSessionIds } from '#lib/state/recents.svelte.js';
	import { onMount, tick } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import SessionEditor from './SessionEditor.svelte';
	import SshConfigImport from './SshConfigImport.svelte';
	import RdpImport from './RdpImport.svelte';
	import ManagerImport from './ManagerImport.svelte';
	import type { AnyImportSource } from './ImportSourcePicker.svelte';
	import CredentialList from './CredentialList.svelte';
	import BulkEditor from './BulkEditor.svelte';
	import { savedCredentialList, credentialsFor, type SavedCredential } from '#lib/ipc/savedCredentials.js';
	import SessionCard from './SessionCard.svelte';
	import VaultSelector from '#lib/components/vault/VaultSelector.svelte';
	import ContextMenuBackdrop from '#lib/components/shared/ContextMenuBackdrop.svelte';
	import ConfirmAction from '#lib/components/devops/ConfirmAction.svelte';
	import Modal from '#lib/components/shared/Modal.svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import FolderIcon from './FolderIcon.svelte';
	import FolderIconPicker from './FolderIconPicker.svelte';
	import UntrustedRefsDialog from '#lib/components/pwmanager/UntrustedRefsDialog.svelte';
	import { sessionUntrustedSecretRefs, sessionApproveSecretRefs, untrustedRefsFromError, type UntrustedRefs } from '#lib/ipc/sessions.js';
	import type { SessionKind, JumpHostConfig } from '#lib/ipc/sessions.js';
	import { webSessionOpen, webSessionUrl } from '#lib/ipc/sessions.js';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { sessionKind, sessionGet, sessionList, sessionDelete, sessionUpdate, sessionListFolders, sessionCreateFolder, sessionUpdateFolderParent, sessionDeleteFolder, sessionRenameFolder, sessionMoveToFolder, sessionSetFavorite, sessionSetOrder, sessionSetFolderCredential, sessionSetFolderTmux,
		sessionSetFolderIcon, sessionBulkUndo, type BulkResult, type SessionConfig, type Folder } from '#lib/ipc/sessions.js';
	import { setActivePage } from '#lib/state/navigation.svelte.js';
	import * as share from '#lib/state/share.svelte.js';
	import { DEFAULT_TELNET_OPTIONS } from '#lib/ipc/telnet.js';
	import { DEFAULT_SERIAL_OPTIONS } from '#lib/ipc/serial.js';
	import { spiceParamsFromOptions, spiceReadVv } from '#lib/ipc/spice.js';
	import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
	import { freerdpLaunch } from '#lib/ipc/freerdp.js';
	import { winboxLaunch } from '#lib/ipc/winbox.js';
	import { osNeedsDetection } from '#lib/data/distro-icons.js';
	import { sshConnect, sshDisconnect, sshDetectOs, sshProbeAddresses, sshTmuxChosen, tmuxStartScript, type JumpHostConnectParams } from '#lib/ipc/ssh.js';
	import { rdpFilesSession, rdpFilesPort } from '#lib/sessions/rdp-files.js';
	import { BMC_SSH_PORT, bmcConsoleSession, bmcVncSession, rememberedConsole } from '#lib/sessions/bmc-console.js';
	import { addressKey, addressName, addressText, autoOrder, checksAddresses, plainOrder, connectMode, hasAltAddresses, isUnreachable, lastAddress, rememberAddress, sessionAddresses, type ResolvedAddress } from '#lib/sessions/addresses.js';
	import AddressPicker from './AddressPicker.svelte';
	// Passwords are now stored encrypted in vault, not in memory cache
	import { createTab, updateTabOs, getSessionTabStates, getTabs, activateTab, toggleFilesView } from '#lib/state/tabs.svelte.js';
	import { getSettings, sessionLogConfig } from '#lib/state/settings.svelte.js';
	import { isMobile, isWindows } from '#lib/platform.js';
	import { unsupportedReason } from '#lib/sessions/support.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { promptForMasterPassword } from '#lib/state/password-prompt.svelte.js';
	import ConnectSteps from './ConnectSteps.svelte';
	import { hasMasterPassword } from '#lib/ipc/credentials.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { providerName } from '#lib/state/pwmanager.svelte.js';
	import type { SecretRef } from '#lib/ipc/pwmanager.js';
	import { untrack } from 'svelte';
	import { positionMenu } from '#lib/utils/positionMenu.js';
	import { hasNetworkAddress, openNetworkTools } from '#lib/state/net-tools.svelte.js';
	import { longPress } from '#lib/utils/longPress.js';
	import { buildFolderTree, byName, bySavedOrder, canMoveFolder, commonVault, folderPath, foldersInVault, moveStep, placeAt, vaultOf, type FolderTreeNode } from '#lib/sessions/folder-tree.js';
	import { canScanLinkCode, scanLinkCode } from '#lib/state/link-scan.js';
	import { vaultState, checkState, initIdentity, importIdentity, linkDevice, ALL_VAULTS, loadVaultFilter, type VaultFilter } from '#lib/state/vault.svelte.js';
	import { tmuxFor } from '#lib/terminal/tmux.js';
	import { openOtherWindows, tmuxView } from '#lib/terminal/tmux-tabs.js';

	let showQuickConnect = $state(false);
	let showEditor = $state(false);
	/** The import dialog open now, by source; null when none is. */
	let importSource = $state<AnyImportSource | null>(null);
	/** The source the dialog opens on: the one used last. */
	let lastImportSource = $state<AnyImportSource>('ssh_config');

	function openImport(source: AnyImportSource): void {
		lastImportSource = source;
		importSource = source;
	}

	function importOpen(source: AnyImportSource | 'manager'): boolean {
		if (source === 'manager') return importSource !== null && importSource !== 'ssh_config' && importSource !== 'rdp';
		return importSource === source;
	}

	function setImportOpen(source: AnyImportSource | 'manager', open: boolean): void {
		if (!open && importOpen(source)) importSource = null;
	}
	let editingSession = $state<SessionConfig | undefined>();
	let sessions = $state<SessionConfig[]>([]);
	let loading = $state(true);
	let deleteConfirm = $state<string | null>(null);

	/** Which list the sidebar shows: the sessions, or the saved credentials. */
	let tab = $state<'sessions' | 'credentials'>('sessions');
	/** Saved credentials, for a folder's credential menu. */
	let savedCredentials = $state<SavedCredential[]>([]);

	// What the list shows: everything, this device's default vault (null), or
	// one vault by id. Restored from the last run; the selector confirms it on
	// mount and updates it on change.
	let selectedVaultId = $state<VaultFilter>(loadVaultFilter());

	/** Where a new session or folder goes. "All vaults" is not a place, so it
	 *  resolves to the device's default. */
	let targetVaultId = $derived(selectedVaultId === ALL_VAULTS ? null : selectedVaultId);

	// Search query
	let searchQuery = $state('');

	// ---- The field is also the quick connect --------------------------------
	//
	// Type a name and the list filters. Type something that reads as an
	// address and a "Connect to …" suggestion appears; Enter takes it when no
	// saved session matched. The detection is deliberately conservative: a
	// session called "a.b" must filter, never connect (see utils/address).

	let addressSuggestion = $derived(parseAddress(searchQuery));
	let quickPrefill = $state<QuickConnectPrefill | null>(null);

	function connectToAddress(): void {
		if (!addressSuggestion) return;
		quickPrefill = addressSuggestion;
		showQuickConnect = true;
	}

	function onSearchKeydown(e: KeyboardEvent): void {
		if (e.key !== 'Enter' || !addressSuggestion) return;
		// A saved session matched what was typed; Enter must not skip past it
		// to a stranger with the same name.
		if (filteredSessions.length > 0) return;
		e.preventDefault();
		connectToAddress();
	}

	// ---- The "+" menu -------------------------------------------------------
	let showAddMenu = $state(false);
	let addMenuEl = $state<HTMLDivElement | undefined>();
	/** Where the menu drops from: the "+" button's bottom-right corner. The
	 *  menu is `position: fixed` so the sidebar's `overflow: hidden` can't cut
	 *  it off when it is wider than the panel. */
	let addMenuAnchor = $state({ x: 0, y: 0 });

	function toggleAddMenu(e: MouseEvent): void {
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		addMenuAnchor = { x: rect.right, y: rect.bottom + 4 };
		showAddMenu = !showAddMenu;
	}

	$effect(() => {
		if (!showAddMenu) return;
		function outside(e: MouseEvent) {
			if (addMenuEl && !addMenuEl.contains(e.target as Node)) showAddMenu = false;
		}
		function escape(e: KeyboardEvent) {
			if (e.key === 'Escape') showAddMenu = false;
		}
		document.addEventListener('mousedown', outside, true);
		document.addEventListener('keydown', escape, true);
		return () => {
			document.removeEventListener('mousedown', outside, true);
			document.removeEventListener('keydown', escape, true);
		};
	});

	function fromMenu(action: () => void): void {
		showAddMenu = false;
		action();
	}

	// Something outside the list changed what it holds: a vault joined twice
	// was merged back into one, or a favorite was set on the start screen.
	$effect(() => {
		const reload = () => void loadSessions();
		window.addEventListener('reach:sessions-changed', reload);
		return () => window.removeEventListener('reach:sessions-changed', reload);
	});

	// The command palette, or another workspace, asks for a session to be
	// opened, with every prompt a normal connect has (password, passphrase):
	// a shell inside a container opens this way, as the session with a
	// different login shell. The sidebar shows this list for it, so it runs
	// whichever panel was open.
	$effect(() => {
		const r = pendingSessionAction();
		if (!r) return;
		untrack(() => {
			takeSessionAction();
			if (r.kind === 'new-session') {
				handleNewSession();
			} else if (r.kind === 'locate') {
				void locateSession(r.sessionId);
			} else if (r.kind === 'edit') {
				void sessionGet(r.sessionId)
					.then((s) => handleEdit(s))
					.catch(() => addToast(t('tabs.session_gone'), 'error'));
			} else if (r.kind === 'quick-connect') {
				quickPrefill = r.prefill ?? null;
				showQuickConnect = true;
			} else if (r.kind === 'bmc-console') {
				void sessionGet(r.sessionId)
					.then((s) => {
						setActivePage('terminal');
						consoleOnConnect = { id: s.id, command: r.command };
						return handleConnect(bmcConsoleSession(s, r.port));
					})
					.catch((err) => addToast(String(err), 'error'));
			} else if (r.kind === 'bmc-vnc') {
				void sessionGet(r.sessionId)
					.then((s) => {
						setActivePage('terminal');
						openVnc(bmcVncSession(s, r.port, r.password));
					})
					.catch((err) => addToast(String(err), 'error'));
			} else if (r.kind === 'connect-many') {
				setActivePage('terminal');
				void (async () => {
					for (const id of r.sessionIds) {
						try {
							await handleConnect(await sessionGet(id));
						} catch {
							addToast(t('tabs.session_gone'), 'error');
						}
					}
				})();
			} else {
				void sessionGet(r.sessionId)
					.then((s) => {
						setActivePage('terminal');
						// A container's shell is never wrapped in tmux.
						return handleConnect(r.shell ? { ...s, shell: r.shell, tmux: { mode: 'off' } } : s);
					})
					.catch((err) => addToast(String(err), 'error'));
			}
		});
	});

	onMount(() => {
		registerSessionActions({
			newSession: () => handleNewSession(),
			quickConnect: () => {
				quickPrefill = null;
				showQuickConnect = true;
			}
		});
		// Every vault lists from its file on this device, which syncs with its
		// backend in the background. When a sync brings in a change, such as
		// a session someone added to a shared vault, show it.
		const unlistenSynced = listen<string>('vault-synced', () => {
			void loadSessions();
		});
		return () => {
			registerSessionActions(null);
			void unlistenSynced.then((unlisten) => unlisten());
		};
	});

	// Folders
	let folders = $state<Folder[]>([]);
	let collapsedFolders = $state<Set<string>>(new Set(JSON.parse(localStorage.getItem('collapsedFolders') ?? '[]')));
	let creatingFolder = $state(false);
	let creatingFolderParentId = $state<string | null>(null);
	let newFolderName = $state('');

	/** What a search also finds a session by: its notes and its fields, but
	 *  not the values of hidden ones. */
	function docsText(s: SessionConfig): string[] {
		const fields = (s.custom_fields ?? []).flatMap(f => (f.hidden ? [f.name] : [f.name, f.value]));
		return s.notes ? [s.notes, ...fields] : fields;
	}

	// ---- How the list is sorted and filtered (the toolbar above it) ----------
	// Kept on this device only: it is how this screen is looked at, not data.
	type SortKey = 'default' | 'name' | 'host' | 'recent';
	const VIEW_KEY = 'reach-session-list-view';
	const KINDS: { id: SessionKind; label: string }[] = [
		{ id: 'ssh', label: 'SSH' },
		{ id: 'rdp', label: 'RDP' },
		{ id: 'vnc', label: 'VNC' },
		{ id: 'spice', label: 'SPICE' },
		{ id: 'telnet', label: 'Telnet' },
		{ id: 'winbox', label: 'WinBox' },
		{ id: 'web', label: 'Web' },
		{ id: 'wsl', label: 'WSL' }
	];
	function loadView(): { sort: SortKey; favorites: boolean; kinds: SessionKind[]; tags: string[] } {
		const none = { sort: 'default' as SortKey, favorites: false, kinds: [], tags: [] };
		try {
			const v = JSON.parse(localStorage.getItem(VIEW_KEY) ?? 'null');
			if (!v || typeof v !== 'object') return none;
			return {
				sort: ['default', 'name', 'host', 'recent'].includes(v.sort) ? v.sort : 'default',
				favorites: v.favorites === true,
				kinds: Array.isArray(v.kinds) ? v.kinds.filter((k: unknown) => KINDS.some(x => x.id === k)) : [],
				tags: Array.isArray(v.tags) ? v.tags.filter((x: unknown) => typeof x === 'string') : []
			};
		} catch {
			return none;
		}
	}
	const initialView = loadView();
	let sortKey = $state<SortKey>(initialView.sort);
	let filterFavorites = $state(initialView.favorites);
	let filterKinds = $state<SessionKind[]>(initialView.kinds);
	let filterTags = $state<string[]>(initialView.tags);
	let filtering = $derived(filterFavorites || filterKinds.length > 0 || filterTags.length > 0);
	let allTags = $derived([...new Set(sessions.flatMap(s => s.tags))].sort((a, b) => a.localeCompare(b)));
	/** The sort or filter menu under its toolbar button. */
	let viewMenu = $state<{ kind: 'sort' | 'filter'; x: number; y: number } | null>(null);

	$effect(() => {
		const v = { sort: sortKey, favorites: filterFavorites, kinds: filterKinds, tags: filterTags };
		try {
			localStorage.setItem(VIEW_KEY, JSON.stringify(v));
		} catch {
			// Not remembered; the list still works.
		}
	});

	function openViewMenu(e: MouseEvent, kind: 'sort' | 'filter'): void {
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		viewMenu = viewMenu?.kind === kind ? null : { kind, x: rect.left, y: rect.bottom + 4 };
	}

	function toggleIn<T>(list: T[], value: T): T[] {
		return list.includes(value) ? list.filter(x => x !== value) : [...list, value];
	}

	function clearFilters(): void {
		filterFavorites = false;
		filterKinds = [];
		filterTags = [];
	}

	function sortSessions(list: SessionConfig[]): SessionConfig[] {
		if (sortKey === 'default') return bySavedOrder(list);
		const out = [...list];
		if (sortKey === 'name') out.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
		else if (sortKey === 'host') out.sort((a, b) => a.host.localeCompare(b.host, undefined, { numeric: true }) || a.port - b.port);
		else {
			// Used here lately first, the rest after in their usual order.
			const recent = recentSessionIds();
			const rank = (s: SessionConfig) => {
				const i = recent.indexOf(s.id);
				return i < 0 ? recent.length : i;
			};
			out.sort((a, b) => rank(a) - rank(b));
		}
		return out;
	}

	/** Why this device cannot open a session (a WinBox one on a phone), or null. */
	function unsupported(session: SessionConfig): string | null {
		return unsupportedReason(session, { mobile: isMobile(), windows: isWindows() });
	}

	// Filter sessions by selected vault, the toolbar's filter, then by search query
	let filteredSessions = $derived.by(() => {
		const showUnsupported = getSettings().showUnsupportedSessions;
		let result = sessions.filter(s => {
			// Sessions this device cannot open show greyed out, unless
			// Settings > Sessions hides them.
			if (!showUnsupported && unsupported(s)) return false;
			if (selectedVaultId === ALL_VAULTS) return true;
			if (selectedVaultId === null) return !s.vault_id;
			return s.vault_id === selectedVaultId;
		});
		if (filterFavorites) result = result.filter(s => s.favorite);
		if (filterKinds.length) result = result.filter(s => filterKinds.includes(sessionKind(s)));
		if (filterTags.length) result = result.filter(s => s.tags.some(tag => filterTags.includes(tag)));
		const raw = searchQuery.trim();
		const re = searchRegex.re;
		if (re) {
			result = result.filter(s => {
				const fields = [s.name, s.host, s.username, `${s.username ? s.username + '@' : ''}${s.host}:${s.port}`, ...s.tags, ...docsText(s)];
				return fields.some(f => re.test(f));
			});
		} else if (searchRegex.error) {
			result = [];
		} else if (raw) {
			const q = raw.toLowerCase();
			result = result.filter(s =>
				s.name.toLowerCase().includes(q) ||
				s.host.toLowerCase().includes(q) ||
				(s.alt_addresses ?? []).some(a => a.host.toLowerCase().includes(q) || (a.label ?? '').toLowerCase().includes(q)) ||
				s.username.toLowerCase().includes(q) ||
				s.tags.some(tag => tag.toLowerCase().includes(q)) ||
				docsText(s).some(text => text.toLowerCase().includes(q))
			);
		}
		return sortSessions(result);
	});

	/** "/pattern/flags" searches by regular expression (case-insensitive
	 *  unless flags are given), over name, host, user, user@host:port and
	 *  tags. A pattern that does not compile says why and matches nothing. */
	let searchRegex = $derived.by((): { re: RegExp | null; error: string | null } => {
		const m = /^\/(.+)\/([a-z]*)$/.exec(searchQuery.trim());
		if (!m) return { re: null, error: null };
		try {
			// g and y would make test() remember where it stopped.
			const flags = (m[2] || 'i').replace(/[gy]/g, '');
			return { re: new RegExp(m[1], flags), error: null };
		} catch (e) {
			return { re: null, error: e instanceof Error ? e.message : String(e) };
		}
	});

	// ---- Selecting several sessions -------------------------------------------
	// Ctrl/Cmd+click toggles one, Shift+click selects the range from the last
	// one clicked; "Select" turns plain clicks into selection (and is the way
	// on a phone). Only sessions on screen stay selected, so a bulk action
	// never reaches one the search has hidden.
	let selectMode = $state(false);
	let selected = $state(new Set<string>());
	/** Sessions with a tab open: tinted, the one on screen picked, a green
	 *  dot when connected (see SessionCard). */
	let tabStates = $derived(getSessionTabStates());
	let lastSelected = $state<string | null>(null);
	let selecting = $derived(selectMode || selected.size > 0);
	let bulkDeleteConfirm = $state(false);
	/** The move dialog: which sessions, the folder filter, the choice. */
	let moveIds_ = $state<string[] | null>(null);
	let moveFilter = $state('');
	let moveTarget = $state<string | null | undefined>(undefined);
	let moveNewName = $state('');
	const NEW_FOLDER = '__new__';
	/** The vault of the sessions with these ids, or undefined when they
	 *  are in several: a session goes only into a folder of its own vault. */
	function vaultOfIds(ids: string[]): string | null | undefined {
		const set = new Set(ids);
		return commonVault(sessions.filter(s => set.has(s.id)));
	}
	/** The folders these sessions can be moved into. */
	function moveTargets(ids: string[]): Folder[] {
		const vault = vaultOfIds(ids);
		return vault === undefined ? [] : foldersInVault(folders, vault);
	}
	let moveFolders = $derived.by(() => {
		const targets = moveIds_ ? moveTargets(moveIds_) : [];
		const filter = moveFilter.trim().toLowerCase();
		return filter ? targets.filter(f => folderPath(f.id, folders).toLowerCase().includes(filter)) : targets;
	});
	/** The sessions being moved are in several vaults: no folder takes them all. */
	let moveMixed = $derived(moveIds_ !== null && vaultOfIds(moveIds_) === undefined);
	/** Sessions to move into the folder being created. */
	let pendingMove = $state<string[] | null>(null);

	/** The sessions in the order they are shown, folded folders left out. */
	function visibleOrder(): string[] {
		const ordered: string[] = [];
		function visit(node: FolderTreeNode): void {
			if (!searchQuery.trim() && collapsedFolders.has(node.folder.id)) return;
			ordered.push(...node.sessions.map(session => session.id));
			node.children.forEach(visit);
		}
		folderTree.roots.forEach(visit);
		ordered.push(...folderTree.ungrouped.map(session => session.id));
		return ordered;
	}

	$effect(() => {
		const shown = new Set(filteredSessions.map(s => s.id));
		const current = untrack(() => selected);
		const kept = [...current].filter(id => shown.has(id));
		if (kept.length !== current.size) selected = new Set(kept);
	});

	function toggleSelect(session: SessionConfig, e: MouseEvent): void {
		const next = new Set(selected);
		const order = visibleOrder();
		if (e.shiftKey && lastSelected && order.includes(lastSelected)) {
			const a = order.indexOf(lastSelected);
			const b = order.indexOf(session.id);
			for (const id of order.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(id);
		} else if (next.has(session.id)) {
			next.delete(session.id);
		} else {
			next.add(session.id);
		}
		lastSelected = session.id;
		selected = next;
	}

	function selectAllShown(): void {
		selected = new Set(filteredSessions.map(s => s.id));
		selectMode = true;
	}

	function clearSelection(): void {
		selected = new Set();
		selectMode = false;
		bulkDeleteConfirm = false;
		lastSelected = null;
	}

	/** What a menu or drag on `session` acts on: the selection when the
	 *  session is in it, else the session alone. */
	function targetsOf(session: SessionConfig): string[] {
		return selected.has(session.id) ? [...selected] : [session.id];
	}

	function openMove(ids: string[]): void {
		moveIds_ = ids;
		moveFilter = '';
		moveTarget = undefined;
		moveNewName = '';
	}

	async function confirmMove(): Promise<void> {
		const ids = moveIds_;
		if (!ids || moveTarget === undefined) return;
		let target = moveTarget;
		if (target === NEW_FOLDER) {
			const name = moveNewName.trim();
			if (!name) return;
			const vault = vaultOfIds(ids);
			if (vault === undefined) {
				addToast(t('session.move_mixed_vaults'), 'error');
				return;
			}
			try {
				target = (await sessionCreateFolder(name, null, vault)).id;
				folders = await sessionListFolders();
			} catch (err) {
				addToast(String(err), 'error');
				return;
			}
		}
		moveIds_ = null;
		await moveIds(ids, target);
	}

	async function moveIds(ids: string[], folderId: string | null): Promise<void> {
		closeContextMenu();
		try {
			const n = await sessionMoveToFolder(ids, folderId);
			await loadSessions();
			if (ids.length > 1) {
				const name = folderId ? (folders.find(f => f.id === folderId)?.name ?? '') : '';
				addToast(folderId ? t('session.moved_n', { count: String(n), folder: name }) : t('session.moved_out_n', { count: String(n) }), 'success');
				clearSelection();
			}
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	/** Favorite or unfavorite a session, or every selected one if it is
	 *  among them; the start screen hears of it. */
	async function setFavorite(session: SessionConfig, favorite: boolean): Promise<void> {
		const ids = selected.has(session.id) && selected.size > 1 ? [...selected] : [session.id];
		closeContextMenu();
		try {
			await sessionSetFavorite(ids, favorite);
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	// ---- The saved order within a folder (Move up/down, drag, Sort by name) ---
	/** Moving sessions in the list needs it to show them all in their saved
	 *  order: with another sort, a search or a filter on, a step would jump
	 *  over sessions not shown. */
	let canReorder = $derived(sortKey === 'default' && !searchQuery.trim() && !filtering);

	/** The sessions shown beside this one: its folder's own, or those
	 *  outside any folder, in the order on screen. */
	function siblingsOf(id: string): SessionConfig[] {
		if (folderTree.ungrouped.some(s => s.id === id)) return folderTree.ungrouped;
		const walk = (nodes: FolderTreeNode[]): SessionConfig[] | undefined => {
			for (const node of nodes) {
				if (node.sessions.some(s => s.id === id)) return node.sessions;
				const found = walk(node.children);
				if (found) return found;
			}
			return undefined;
		};
		return walk(folderTree.roots) ?? [];
	}

	/** Store this order for a folder's sessions, unless it already is. */
	async function saveOrder(before: string[], next: string[]): Promise<void> {
		if (before.join('\n') === next.join('\n') && sessions.filter(s => next.includes(s.id)).every(s => typeof s.order === 'number')) return;
		try {
			await sessionSetOrder(next);
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
		} catch (err) {
			addToast(String(err), 'error');
		}
		await loadSessions();
	}

	/** Move a session a step up (-1) or down (+1) among those beside it. */
	async function moveSessionStep(session: SessionConfig, step: -1 | 1): Promise<void> {
		closeContextMenu();
		if (!canReorder) return;
		const ids = siblingsOf(session.id).map(s => s.id);
		const next = moveStep(ids, session.id, step);
		if (next !== ids) await saveOrder(ids, next);
	}

	/** Rewrite the saved order of a folder's sessions, and of every folder
	 *  inside it, by name; the list then shows the saved order. */
	async function sortFolderByName(folder: Folder): Promise<void> {
		closeContextMenu();
		try {
			for (const id of foldersInside(folder)) {
				const own = sessions.filter(s => s.folder_id === id);
				if (own.length) await sessionSetOrder(byName(own).map(s => s.id));
			}
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
		} catch (err) {
			addToast(String(err), 'error');
		}
		sortKey = 'default';
		await loadSessions();
	}

	// ---- Editing the selected sessions at once (BulkEditor) -------------------
	let bulkEditOpen = $state(false);
	/** Undoes the last bulk edit until the app quits or the vault locks. */
	let lastBulkUndo = $state<string | null>(null);
	let bulkEditSessions = $derived(bulkEditOpen ? sessions.filter(s => selected.has(s.id)) : []);

	function openBulkEdit(): void {
		closeContextMenu();
		if (selected.size === 1) {
			const only = sessions.find(s => selected.has(s.id));
			if (only) handleEdit(only);
			return;
		}
		if (selected.size > 1) bulkEditOpen = true;
	}

	async function bulkEdited(result: BulkResult): Promise<void> {
		await loadSessions();
		const changed = result.results.filter(r => r.status === 'changed').length;
		const failed = result.results.filter(r => r.status === 'failed').length;
		if (result.undo) lastBulkUndo = result.undo;
		if (failed > 0) return; // the editor shows what failed
		clearSelection(); // done with them; a failure keeps them for Retry
		const token = result.undo;
		addToast(
			t('bulk.done_toast', { count: String(changed) }),
			'success',
			30000,
			token ? { label: t('bulk.undo'), run: () => void undoBulkEdit(token) } : undefined
		);
	}

	async function undoBulkEdit(token: string | null = lastBulkUndo): Promise<void> {
		closeContextMenu();
		if (!token) return;
		try {
			const r = await sessionBulkUndo(token);
			lastBulkUndo = null;
			await loadSessions();
			if (r.kept.length > 0) {
				addToast(t('bulk.undo_partial', { count: String(r.restored), names: r.kept.join(', ') }), 'warning', 8000);
			} else {
				addToast(t('bulk.undone_toast', { count: String(r.restored) }), 'success');
			}
		} catch (err) {
			lastBulkUndo = null;
			addToast(String(err), 'error');
		}
	}

	/** A folder and every folder under it, at any depth. */
	function foldersInside(folder: Folder): Set<string> {
		const inside = new Set([folder.id]);
		let grew = true;
		while (grew) {
			grew = false;
			for (const f of folders) {
				if (f.parent_id && inside.has(f.parent_id) && !inside.has(f.id)) {
					inside.add(f.id);
					grew = true;
				}
			}
		}
		return inside;
	}

	/** The sessions on screen inside a folder, at any depth. */
	function sessionsInside(folder: Folder): SessionConfig[] {
		const inside = foldersInside(folder);
		return filteredSessions.filter(s => s.folder_id && inside.has(s.folder_id));
	}

	/** Add every session inside a folder, at any depth, to the selection. */
	function selectFolderSessions(folder: Folder): void {
		closeContextMenu();
		const next = new Set(selected);
		for (const s of sessionsInside(folder)) next.add(s.id);
		selected = next;
		selectMode = true;
	}

	/** Bulk edit (or the editor, for one) everything inside a folder. */
	function editFolderSessions(folder: Folder): void {
		const inside = sessionsInside(folder);
		closeContextMenu();
		if (inside.length === 0) return;
		selected = new Set(inside.map(s => s.id));
		selectMode = true;
		openBulkEdit();
	}

	// ---- Folder menu: open all, new session here, rename, delete --------------
	/** More than this many asks first: each one is a connection and a tab. */
	const OPEN_ALL_ASK_ABOVE = 10;
	let openAllConfirm = $state<{ folder: Folder; sessions: SessionConfig[] } | null>(null);

	function openAllIn(folder: Folder): void {
		const inside = sessionsInside(folder);
		closeContextMenu();
		if (inside.length > OPEN_ALL_ASK_ABOVE) openAllConfirm = { folder, sessions: inside };
		else openSessions(inside);
	}

	function openSessions(list: SessionConfig[]): void {
		openAllConfirm = null;
		// Ones this device cannot open (WinBox on a phone) are left out.
		for (const session of list) if (!unsupported(session)) void handleConnect(session);
	}

	/** The folder a session made with "New session here" starts in. */
	let newSessionFolder = $state<Folder | null>(null);

	function newSessionIn(folder: Folder): void {
		closeContextMenu();
		editingSession = undefined;
		cloningSession = undefined;
		newSessionFolder = folder;
		showEditor = true;
	}

	let renamingFolderId = $state<string | null>(null);
	let renameValue = $state('');

	function startRename(folder: Folder): void {
		closeContextMenu();
		renamingFolderId = folder.id;
		renameValue = folder.name;
	}

	async function commitRename(): Promise<void> {
		const id = renamingFolderId;
		const name = renameValue.trim();
		renamingFolderId = null;
		const folder = folders.find(f => f.id === id);
		if (!id || !folder || !name || name === folder.name) return;
		try {
			await sessionRenameFolder(id, name);
			folders = await sessionListFolders();
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	function focusSelect(el: HTMLInputElement): void {
		el.focus();
		el.select();
	}

	let folderDeleteConfirm = $state<Folder | null>(null);

	function askDeleteFolder(folder: Folder): void {
		closeContextMenu();
		folderDeleteConfirm = folder;
	}

	/** Open or fold a folder together with every folder under it. */
	function setFoldersInside(folder: Folder, collapsed: boolean): void {
		closeContextMenu();
		const next = new Set(collapsedFolders);
		for (const id of foldersInside(folder)) {
			if (collapsed) next.add(id);
			else next.delete(id);
		}
		collapsedFolders = next;
		localStorage.setItem('collapsedFolders', JSON.stringify([...next]));
	}

	async function deleteSelected(): Promise<void> {
		const ids = [...selected];
		bulkDeleteConfirm = false;
		let done = 0;
		try {
			for (const id of ids) {
				await sessionDelete(id);
				done++;
			}
		} catch (err) {
			addToast(String(err), 'error');
		}
		await loadSessions();
		addToast(t('session.deleted_n', { count: String(done) }), 'success');
		clearSelection();
	}

	$effect(() => {
		if (!selecting) return;
		function onKey(e: KeyboardEvent) {
			// A dialog on top closes itself first.
			if (e.key === 'Escape' && !moveIds_ && !bulkDeleteConfirm && !bulkEditOpen) clearSelection();
		}
		window.addEventListener('keydown', onKey);
		return () => window.removeEventListener('keydown', onKey);
	});

	function newFolderFor(ids: string[]): void {
		closeContextMenu();
		pendingMove = ids;
		creatingFolderParentId = null;
		creatingFolder = true;
		newFolderName = '';
	}


	// Build the hierarchy from Folder.parent_id. Each vault shows only its
	// own folders (Private ones only under Private); All vaults shows every
	// folder that holds a session shown. Search keeps ancestors of matching
	// sessions visible; outside search, empty folders stay visible in the
	// selected vault so they can receive sessions or subfolders.
	let shownFolders = $derived(selectedVaultId === ALL_VAULTS ? folders : foldersInVault(folders, selectedVaultId));
	let folderTree = $derived.by(() => buildFolderTree(shownFolders, filteredSessions, () =>
		!searchQuery.trim() && selectedVaultId !== ALL_VAULTS
	));

	function toggleFolder(folderId: string): void {
		const next = new Set(collapsedFolders);
		if (next.has(folderId)) next.delete(folderId);
		else next.add(folderId);
		collapsedFolders = next;
		localStorage.setItem('collapsedFolders', JSON.stringify([...next]));
	}

	/** Some folder on screen is open: the toolbar button folds them all. */
	let anyExpanded = $derived(shownFolders.some(f => !collapsedFolders.has(f.id)));

	function setAllFolders(collapsed: boolean): void {
		collapsedFolders = new Set(collapsed ? shownFolders.map(f => f.id) : []);
		localStorage.setItem('collapsedFolders', JSON.stringify([...collapsedFolders]));
	}

	/** Star the selected sessions, or unstar them when all already are. */
	async function favoriteSelected(): Promise<void> {
		const ids = [...selected];
		if (!ids.length) return;
		const all = sessions.filter(s => selected.has(s.id)).every(s => s.favorite);
		try {
			await sessionSetFavorite(ids, !all);
			window.dispatchEvent(new CustomEvent('reach:sessions-changed'));
			await loadSessions();
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	/** Ctrl+A (Cmd+A) while the list has focus selects what it shows; in the
	 *  search field it keeps selecting the text. */
	function onListKeydown(e: KeyboardEvent): void {
		if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey || e.key.toLowerCase() !== 'a') return;
		const el = e.target as HTMLElement | null;
		if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return;
		e.preventDefault();
		selectAllShown();
	}

	async function handleCreateFolder(): Promise<void> {
		const name = newFolderName.trim();
		if (!name) return;
		try {
			const parent = creatingFolderParentId ? folders.find(folder => folder.id === creatingFolderParentId) : undefined;
			const ids = pendingMove;
			// A folder made to take sessions goes in their vault.
			const vault = parent ? vaultOf(parent) : ids?.length ? vaultOfIds(ids) : targetVaultId;
			if (vault === undefined) {
				addToast(t('session.move_mixed_vaults'), 'error');
				return;
			}
			const folder = await sessionCreateFolder(name, creatingFolderParentId, vault);
			if (creatingFolderParentId) {
				const next = new Set(collapsedFolders);
				next.delete(creatingFolderParentId);
				collapsedFolders = next;
				localStorage.setItem('collapsedFolders', JSON.stringify([...next]));
			}
			newFolderName = '';
			creatingFolder = false;
			creatingFolderParentId = null;
			folders = await sessionListFolders();
			pendingMove = null;
			if (ids?.length) await moveIds(ids, folder.id);
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	async function handleDeleteFolder(folderId: string): Promise<void> {
		const folder = folders.find(item => item.id === folderId);
		const parentId = folder?.parent_id ?? null;
		const movedChildren: Folder[] = [];
		const movedSessions = sessions.filter(item => item.folder_id === folderId);
		try {
			for (const child of folders.filter(item => item.parent_id === folderId)) {
				await sessionUpdateFolderParent(child.id, parentId);
				movedChildren.push(child);
			}
			if (movedSessions.length) await sessionMoveToFolder(movedSessions.map(session => session.id), parentId);
			await sessionDeleteFolder(folderId);
		} catch (err) {
			if (movedSessions.length) {
				try { await sessionMoveToFolder(movedSessions.map(session => session.id), folderId); } catch { /* reload below shows any unrecoverable write */ }
			}
			for (const child of movedChildren.reverse()) {
				try { await sessionUpdateFolderParent(child.id, folderId); } catch { /* same */ }
			}
			addToast(String(err), 'error');
		} finally {
			folders = await sessionListFolders();
			await loadSessions();
		}
	}

	// Drag & drop via pointer events (HTML5 DnD doesn't work in Tauri WebView2 on Windows)
	let dragSession = $state<SessionConfig | undefined>();
	let dropTarget = $state<string | null | undefined>();
	/** Over a session row: the dragged ones go just before or after it. */
	let dropSlot = $state<{ id: string; before: boolean } | undefined>();
	let dragging = $state(false);

	function handleDragStart(e: PointerEvent, session: SessionConfig): void {
		dragSession = session;
		dragging = true;
		// Only folders of the dragged sessions' vault take them.
		const allowed = new Set(moveTargets(targetsOf(session)).map(f => f.id));
		const onMove = (me: PointerEvent) => {
			if (!dragSession) return;
			// Find drop target by checking which folder header we're over
			dropTarget = undefined;
			dropSlot = undefined;
			const el = document.elementFromPoint(me.clientX, me.clientY);
			const row = canReorder ? (el?.closest('[data-session-id]') as HTMLElement | null) : null;
			if (row) {
				const id = (row.closest('[data-folder-id]') as HTMLElement | null)?.dataset.folderId;
				const group = id === undefined || id === '__ungrouped__' ? null : allowed.has(id) ? id : undefined;
				if (group !== undefined) {
					const rect = row.getBoundingClientRect();
					dropTarget = group;
					dropSlot = { id: row.dataset.sessionId!, before: me.clientY < rect.top + rect.height / 2 };
					return;
				}
			}
			if (el) {
				const folderEl = el.closest('[data-folder-id]') as HTMLElement | null;
				if (folderEl) {
					const id = folderEl.dataset.folderId!;
					dropTarget = id === '__ungrouped__' ? null : allowed.has(id) ? id : undefined;
				} else {
					dropTarget = undefined;
				}
			}
		};
		const onUp = async () => {
			window.removeEventListener('pointermove', onMove);
			window.removeEventListener('pointerup', onUp);
			const session = dragSession;
			const target = dropTarget;
			const slot = dropSlot;
			dragSession = undefined;
			dropTarget = undefined;
			dropSlot = undefined;
			dragging = false;
			if (session && slot && target !== undefined) {
				await dropAt(session, target, slot);
			} else if (session && target !== undefined && (selected.has(session.id) || session.folder_id !== target)) {
				await moveIds(targetsOf(session), target);
			}
		};
		window.addEventListener('pointermove', onMove);
		window.addEventListener('pointerup', onUp);
	}

	/** Drop the dragged sessions just before or after a row, moving them
	 *  into that row's folder first if they come from another. */
	async function dropAt(session: SessionConfig, target: string | null, slot: { id: string; before: boolean }): Promise<void> {
		const siblings = siblingsOf(slot.id).map(s => s.id);
		const shown = filteredSessions.map(s => s.id);
		// Several selected ones go in the order they are shown.
		const moving = targetsOf(session).sort((a, b) => shown.indexOf(a) - shown.indexOf(b));
		if (moving.includes(slot.id) && moving.length === 1) return;
		const incoming = moving.filter(id => !siblings.includes(id));
		if (incoming.length) {
			await moveIds(incoming, target);
			// The move was refused (another vault's folder): leave the order alone.
			if (sessions.some(s => incoming.includes(s.id) && (s.folder_id ?? null) !== target)) return;
		}
		const index = siblings.indexOf(slot.id) + (slot.before ? 0 : 1);
		await saveOrder(siblings, placeAt(siblings, moving, index));
	}

	// Right-click context menu
	let contextMenu = $state<{ x: number; y: number; session?: SessionConfig; folder?: Folder } | undefined>();

	/** On a phone the session and folder menus come up from the bottom, in
	 *  reach of the thumb, with rows a finger can hit. A tablet keeps the
	 *  menu at the finger, as a mouse has it. */
	const SHEET_MAX_WIDTH = 640;
	let sheetMenu = $state(false);

	function openSessionContextMenu(e: MouseEvent, session: SessionConfig): void {
		e.preventDefault();
		e.stopPropagation();
		sheetMenu = isMobile() && window.innerWidth < SHEET_MAX_WIDTH;
		contextMenu = { x: e.clientX, y: e.clientY, session };
		sessionMenuPage = 'main';
	}

	function openBackgroundContextMenu(e: MouseEvent): void {
		e.preventDefault();
		sheetMenu = false;
		contextMenu = { x: e.clientX, y: e.clientY };
	}

	/** Which list a folder's menu shows: the main one, or one of its choices. */
	let folderMenuPage = $state<'main' | 'move' | 'login' | 'tmux'>('main');
	/** Which list a session's menu shows: the main one, or Connect to (an
	 *  address and a login for one connect). */
	let sessionMenuPage = $state<'main' | 'connect'>('main');
	/** Connect to's choices: an address index (null: as the session says)
	 *  and a saved credential id (null: the session's own login). */
	let menuAddress = $state<number | null>(null);
	let menuCredential = $state<string | null>(null);
	/** Connect to's choice to leave tmux out this once. */
	let menuNoTmux = $state(false);

	function openFolderContextMenu(e: MouseEvent, folder: Folder): void {
		e.preventDefault();
		e.stopPropagation();
		folderMenuPage = 'main';
		sheetMenu = isMobile() && window.innerWidth < SHEET_MAX_WIDTH;
		contextMenu = { x: e.clientX, y: e.clientY, folder };
	}

	function openFolderMenu(e: MouseEvent, folder: Folder): void {
		e.stopPropagation();
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		folderMenuPage = 'main';
		sheetMenu = isMobile() && window.innerWidth < SHEET_MAX_WIDTH;
		contextMenu = { x: rect.right, y: rect.bottom, folder };
	}

	function closeContextMenu(): void {
		contextMenu = undefined;
		folderMenuPage = 'main';
		sessionMenuPage = 'main';
	}

	async function moveToFolder(session: SessionConfig, folderId: string | null): Promise<void> {
		await moveIds(targetsOf(session), folderId);
	}

	function contextNewFolder(parentId: string | null = null): void {
		const session = contextMenu?.session;
		closeContextMenu();
		creatingFolderParentId = parentId;
		newFolderName = '';
		if (session) newFolderFor(targetsOf(session));
		else creatingFolder = true;
	}

	async function moveFolder(folder: Folder, parentId: string | null): Promise<void> {
		closeContextMenu();
		if (!canMoveFolder(folder.id, parentId, folders)) return;
		try {
			await sessionUpdateFolderParent(folder.id, parentId);
			folders = await sessionListFolders();
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	// Vault state (TLS-style: auto-unlock, no password needed)
	let locked = $derived(vaultState.locked);

	// Unlocking from the lock screen (password, fingerprint, security key) has
	// to bring the list back: a load that ran while the vault was locked found
	// nothing, and the list would otherwise stay empty until something else
	// asked it to reload.
	let wasOpen = !vaultState.locked && !vaultState.held;
	$effect(() => {
		const open = !vaultState.locked && !vaultState.held;
		if (open && !wasOpen) void loadSessions();
		wasOpen = open;
	});
	let hasIdentity = $derived(vaultState.hasIdentity);
	let keychainError = $derived(vaultState.keychainError);
	let initializing = $state(false);
	let initError = $state('');
	let importKey = $state('');
	let importing = $state(false);
	let linkCode = $state('');

	// Connect prompt state
	let connectSession = $state<SessionConfig | undefined>();
	let connectPassword = $state('');
	let connectKeyPassphrase = $state('');
	let connecting = $state(false);
	let connectingId = $state<string | undefined>();
	let connectError = $state<string | undefined>();
	let rememberPassword = $state(false);
	let hasSavedPassword = $state(false);
	// When the saved auth method (key/agent) gets rejected by the server, switch
	// the modal to "fallback to password" mode without losing context.
	let passwordFallback = $state(false);
	let fallbackPassword = $state('');
	// Whether the prompt asks for the password or key passphrase. Not while a
	// session that has what it needs (a saved secret, a key unlocked at start,
	// a passphrase-less key) connects by itself: an empty field there only
	// distracts. It appears if that attempt fails, so a retry can use one.
	let askSecret = $state(true);
	// Which of the session's addresses this connect uses: picked by hand
	// (Connect via, or the picker of a session set to Ask), else chosen at
	// connect (Auto). The one being tried, for the prompt to show.
	let connectAddress = $state<ResolvedAddress | undefined>();
	let tryingAddress = $state<ResolvedAddress | undefined>();
	// A session set to Ask, waiting for an address to be picked.
	let pickAddressFor = $state<SessionConfig | undefined>();

	let showConnectPrompt = $derived(!!connectSession);

	// Programmatic focus for the fallback input — `autofocus` is unreliable
	// inside conditionally-rendered blocks across WebView versions.
	let fallbackInputEl: HTMLInputElement | undefined = $state();
	$effect(() => {
		if (passwordFallback && fallbackInputEl) {
			fallbackInputEl.focus();
		}
	});

	/** Set or clear the saved credential a folder's sessions log in with. */
	async function setFolderCredential(folder: Folder, credentialId: string | null): Promise<void> {
		closeContextMenu();
		try {
			await sessionSetFolderCredential(folder.id, credentialId);
			await loadSessions();
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	/** Turn tmux on or off for a folder's sessions; null: as the folder above says. */
	async function setFolderTmux(folder: Folder, value: boolean | null): Promise<void> {
		closeContextMenu();
		try {
			await sessionSetFolderTmux(folder.id, value);
			await loadSessions();
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	/** The folder whose icon is being picked. */
	let iconFolder = $state<Folder | null>(null);

	function pickFolderIcon(folder: Folder): void {
		closeContextMenu();
		iconFolder = folder;
	}

	async function saveFolderIcon(icon: string | null, color: string | null): Promise<void> {
		const folder = iconFolder;
		iconFolder = null;
		if (!folder || ((folder.icon ?? null) === icon && (folder.color ?? null) === color)) return;
		try {
			await sessionSetFolderIcon(folder.id, icon, color);
			folders = await sessionListFolders();
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	async function loadSessions(): Promise<void> {
		try {
			[sessions, folders] = await Promise.all([sessionList(), sessionListFolders()]);
			savedCredentials = await savedCredentialList().catch(() => []);
		} catch (err) {
			console.error('Failed to load sessions:', err);
		} finally {
			loading = false;
		}
		void warnIfNoMasterPassword();
	}

	// Identities created before the issue-25 fix never persisted a
	// password-encrypted copy of the secret key, so the OS keychain is the only
	// thing that can open them. If that entry is ever lost the vault is
	// unrecoverable — the failure reported in issue #30. Ask for one while the
	// vault is still open and it can be set, in a window that sets it rather
	// than a notice that points to Settings. Once per launch.
	let warnedNoPassword = false;
	async function warnIfNoMasterPassword(): Promise<void> {
		if (warnedNoPassword) return;
		try {
			if (!(await hasMasterPassword())) {
				warnedNoPassword = true;
				promptForMasterPassword();
			}
		} catch {
			// A diagnostic must never break loading the session list.
		}
	}

	/**
	 * Open a saved RDP session with its selected desktop client. The embedded
	 * panel connects once it knows its size.
	 */
	async function openRdp(session: SessionConfig, password: string, remember: boolean, passwordRef?: SecretRef | null): Promise<void> {
		if (session.rdp_use_freerdp && !isMobile()) {
			await freerdpLaunch(
				{
					host: session.host,
					port: session.port,
					username: session.username,
					domain: session.domain ?? null,
					password: passwordRef ? null : password,
					usePasswordRef: !!passwordRef,
					clipboard: session.rdp_clipboard ?? null,
					graphicsPipeline: session.detected_os === 'gnome',
					sharePath: session.share_path ?? null,
					rdp: session.rdp ?? null,
					program: getSettings().freerdpPath || null,
				},
				session.id,
				(error) => addToast(error, 'error')
			);
			addToast(t('freerdp.started', { name: session.name }), 'success');
		} else {
			const tab = createTab('rdp', `${session.username}@${session.host}`, undefined, session.name, session.detected_os ?? 'windows');
			tab.rdpConnectParams = {
				id: tab.id,
				host: session.host,
				port: session.port,
				username: session.username,
				// The backend fetches the password from the manager; none goes beside it.
				password: passwordRef ? '' : password,
				passwordRef: passwordRef ?? null,
				sessionId: session.id,
				domain: session.domain ?? undefined,
				sharePath: session.share_path ?? undefined,
				clipboard: session.rdp_clipboard ?? null,
				forwardLockKeys: session.rdp_forward_lock_keys ?? true,
				options: session.rdp ?? null,
				// GNOME Remote Desktop serves no client without the graphics
				// pipeline, so a GNOME session asks for it whatever the setting says.
				graphicsPipeline: session.detected_os === 'gnome' ? true : undefined,
				width: 0,
				height: 0,
			};
			addToast(t('session.connected_toast', { name: session.name }), 'success');
		}

		const stored = session.auth_method.type === 'Password' ? session.auth_method.password : undefined;
		if (remember && !passwordRef && password && password !== stored && !oneOffCredential(session)) {
			try {
				await sessionUpdate({ ...session, auth_method: { type: 'Password', password } });
				await loadSessions();
			} catch {
				// The desktop is open either way; the prompt returns next time.
			}
		}
	}

	/**
	 * Open a saved VNC session in a desktop tab. A VNC server may ask for no
	 * password at all, so a session without one connects as it is; one that
	 * does ask says so on the panel.
	 */
	function openVnc(session: SessionConfig): void {
		const tab = createTab('vnc', session.host, undefined, session.name, session.detected_os ?? null);
		tab.vncConnectParams = {
			id: tab.id,
			host: session.host,
			port: session.port,
			// A Mac's macOS account; empty for a VNC password.
			username: session.username,
			password: session.auth_method.type === 'Password' && !session.auth_method.password_ref ? (session.auth_method.password ?? '') : '',
			passwordRef: session.auth_method.type === 'Password' ? (session.auth_method.password_ref ?? null) : null,
			sessionId: session.id,
			viaSessionId: session.via_session_id ?? null,
		};
		addToast(t('session.connected_toast', { name: session.name }), 'success');
	}

	/**
	 * Open a saved SPICE session (a virtual machine's display) in a desktop
	 * tab. Like VNC, a server may ask for no password.
	 */
	function openSpice(session: SessionConfig): void {
		const tab = createTab('spice', session.host, undefined, session.name, session.detected_os ?? null);
		tab.spiceConnectParams = {
			id: tab.id,
			host: session.host,
			port: session.port,
			password: session.auth_method.type === 'Password' && !session.auth_method.password_ref ? (session.auth_method.password ?? '') : '',
			passwordRef: session.auth_method.type === 'Password' ? (session.auth_method.password_ref ?? null) : null,
			sessionId: session.id,
			viaSessionId: session.via_session_id ?? null,
			...spiceParamsFromOptions(session.spice),
		};
		addToast(t('session.connected_toast', { name: session.name }), 'success');
	}

	/**
	 * Open a virt-viewer file (.vv), as Proxmox VE's SPICE console button and
	 * virt-manager hand out, in a desktop tab. Not saved as a session: a
	 * Proxmox ticket in it works once, within about 30 seconds.
	 */
	async function openVvFile(): Promise<void> {
		let path: string | null;
		try {
			const picked = await openFileDialog({
				multiple: false,
				directory: false,
				// Phones pick by type, and a .vv file has none they know.
				filters: isMobile() ? undefined : [{ name: t('spice.vv_files'), extensions: ['vv'] }],
			});
			path = typeof picked === 'string' ? picked : null;
		} catch {
			return;
		}
		if (!path) return;
		try {
			const vv = await spiceReadVv(path);
			const title = vv.title || (vv.host.startsWith('pvespiceproxy:') ? 'SPICE' : vv.host);
			setActivePage('terminal');
			const tab = createTab('spice', vv.host, undefined, title, null);
			tab.spiceConnectParams = {
				id: tab.id,
				host: vv.host,
				port: vv.port,
				password: vv.password,
				tlsPort: vv.tlsPort,
				ca: vv.ca,
				hostSubject: vv.hostSubject,
				proxy: vv.proxy,
			};
		} catch (e) {
			addToast(t('spice.vv_failed', { error: String(e) }), 'error');
		}
	}

	/**
	 * Open a saved Telnet session in a terminal tab, which connects by
	 * itself once it is listening. Telnet has no login of its own: a saved
	 * user name and password are typed in when the device asks for them.
	 */
	function openTelnet(session: SessionConfig): void {
		setActivePage('terminal');
		const auth = session.auth_method;
		const tab = createTab('telnet', session.host, undefined, session.name, session.detected_os ?? null);
		tab.sessionId = session.id;
		tab.terminalTheme = session.terminal_theme ?? null;
		tab.cursorStyle = session.cursor_style ?? null;
		tab.cursorBlink = session.cursor_blink ?? null;
		tab.historyMode = session.history ?? null;
		tab.telnetConnectParams = {
			host: session.host,
			port: session.port,
			options: { ...DEFAULT_TELNET_OPTIONS, ...(session.telnet ?? {}) },
			username: session.username || null,
			password: auth.type === 'Password' && !auth.password_ref ? (auth.password ?? null) : null,
			passwordRef: auth.type === 'Password' ? (auth.password_ref ?? null) : null,
			sessionId: session.id,
			wireguard: session.wireguard_id ?? null,
		};
	}

	/**
	 * Open a saved FTP session in the full-pane file manager. Its panel
	 * connects it, asking for a password or about a certificate there.
	 */
	function openFtp(session: SessionConfig): void {
		if (isMobile()) {
			addToast(t('files.desktop_only'), 'error');
			return;
		}
		setActivePage('terminal');
		const tab = createTab('ftp', session.host, undefined, session.name, null);
		tab.ftpConnectParams = { sessionId: session.id, host: session.host, port: session.port, username: session.username };
	}

	/**
	 * Open a saved server management session (iDRAC, iLO...) in a tab of
	 * its own. Its panel logs in, asking for a password or about a
	 * certificate there. An open one is switched to.
	 */
	/**
	 * Open a saved Proxmox session in a tab of its own. Its panel logs in,
	 * asking for a password, a code or about a certificate there. An open
	 * one is switched to.
	 */
	function openProxmox(session: SessionConfig): void {
		setActivePage('terminal');
		const open = getTabs().find((tab) => tab.type === 'proxmox' && tab.proxmoxConnectParams?.sessionId === session.id);
		if (open) {
			activateTab(open.id);
			return;
		}
		const tab = createTab('proxmox', session.host, undefined, session.name, null);
		tab.proxmoxConnectParams = { sessionId: session.id, host: session.host, port: session.port, username: session.username };
	}

	function openBmc(session: SessionConfig): void {
		setActivePage('terminal');
		const open = getTabs().find((tab) => tab.type === 'bmc' && tab.bmcConnectParams?.sessionId === session.id);
		if (open) {
			activateTab(open.id);
			return;
		}
		const tab = createTab('bmc', session.host, undefined, session.name, null);
		tab.bmcConnectParams = { sessionId: session.id, host: session.host, port: session.port, username: session.username };
	}

	/**
	 * Open a saved serial session in a terminal tab, which opens the port by
	 * itself once it is listening. The port is the session's `host`.
	 */
	function openSerial(session: SessionConfig): void {
		if (isMobile()) {
			addToast(t('serial.desktop_only'), 'error');
			return;
		}
		setActivePage('terminal');
		const tab = createTab('serial', session.host, undefined, session.name, session.detected_os ?? null);
		tab.sessionId = session.id;
		tab.terminalTheme = session.terminal_theme ?? null;
		tab.historyMode = session.history ?? null;
		tab.serialConnectParams = {
			port: session.host,
			options: { ...DEFAULT_SERIAL_OPTIONS, ...(session.serial ?? {}) },
		};
	}

	/**
	 * Start WinBox 4 on a saved session: a program of its own, so no tab.
	 * Settings say where it is and how the password reaches it.
	 */
	async function openWinbox(session: SessionConfig): Promise<void> {
		if (isMobile()) {
			addToast(t('winbox.desktop_only'), 'error');
			return;
		}
		const auth = session.auth_method;
		const settings = getSettings();
		try {
			const launched = await winboxLaunch(
				{
					host: session.host,
					port: session.port,
					username: session.username || null,
					password: auth.type === 'Password' && !auth.password_ref ? (auth.password ?? null) : null,
					passwordRef: auth.type === 'Password' ? (auth.password_ref ?? null) : null,
					passwordMode: settings.winboxPassword,
					program: settings.winboxPath || null,
				},
				session.id
			);
			addToast(t(launched.copied ? 'winbox.started_copied' : 'winbox.started', { name: session.name }), 'success');
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	/** Launch a local WSL session as a terminal tab */
	async function openWsl(session: SessionConfig): Promise<void> {
		const distro = session.wsl_distro?.trim();
		const args = distro ? ['-d', distro] : [];
		if (session.username?.trim()) {
			args.push('-u', session.username.trim());
		}
		// In tmux inside the distribution: it outlives the tab and the app.
		const tmux = tmuxFor(session, folders);
		if (tmux) {
			try {
				args.push('-e', 'sh', '-c', await tmuxStartScript(tmux));
			} catch (err) {
				addToast(String(err), 'error');
				return;
			}
		}
		setActivePage('terminal');
		const connectionId = `wsl-${crypto.randomUUID()}`;
		const tab = createTab(
			'local',
			distro ? `${distro} · WSL` : 'WSL',
			connectionId,
			session.name,
			undefined,
			{ program: 'wsl.exe', args },
			distro ?? ''
		);
		tab.sessionId = session.id;
		tab.terminalTheme = session.terminal_theme ?? null;
		tab.cursorStyle = session.cursor_style ?? null;
		tab.cursorBlink = session.cursor_blink ?? null;
		tab.monitoring = session.monitoring ?? null;
		tab.historyMode = session.history ?? null;
		addToast(t('session.connected_toast', { name: session.name }), 'success');
	}

	// Password manager entries the user has not approved for a session (see
	// UntrustedRefsDialog). One question at a time; the connect waits on it.
	let askedRefs = $state<UntrustedRefs | null>(null);
	let approvingRefs = $state(false);
	let approveRefsError = $state<string | undefined>();
	let answerAsked: ((allowed: boolean) => void) | null = null;

	function askToApprove(asked: UntrustedRefs): Promise<boolean> {
		answerAsked?.(false);
		return new Promise((resolve) => {
			approveRefsError = undefined;
			askedRefs = asked;
			answerAsked = resolve;
		});
	}

	function finishAsking(allowed: boolean): void {
		const answer = answerAsked;
		answerAsked = null;
		askedRefs = null;
		approveRefsError = undefined;
		answer?.(allowed);
	}

	async function allowAskedRefs(): Promise<void> {
		if (!askedRefs || approvingRefs) return;
		approvingRefs = true;
		try {
			await sessionApproveSecretRefs(askedRefs);
			finishAsking(true);
			void loadSessions();
		} catch (err) {
			// The session changed while the dialog was open, or could not be
			// saved: say so, approve nothing.
			approveRefsError = String(err);
		} finally {
			approvingRefs = false;
		}
	}

	// ---- Connect with: one connect with another saved credential ---------
	/** A session as one connect sees it with credential `c` in place of its
	 *  own login. Never saved: openRdp and the prompt's Remember leave it be. */
	type OneOff = SessionConfig & { one_off_credential: string };
	function withCredential(session: SessionConfig, c: SavedCredential): OneOff {
		return {
			...session,
			username: c.username,
			auth_method: c.auth_method,
			domain: c.domain?.trim() ? c.domain : session.domain,
			credential: { id: c.id, name: c.name, folder_id: null, missing: false },
			one_off_credential: c.id,
		};
	}
	function oneOffCredential(s: SessionConfig): string | undefined {
		return (s as Partial<OneOff>).one_off_credential;
	}
	/** Kinds that log in with a user name and a password or key. */
	function takesCredential(s: SessionConfig): boolean {
		const k = sessionKind(s);
		return k === 'ssh' || k === 'rdp' || k === 'telnet';
	}
	/** The credentials the session's menu offers: its vault's, and for RDP
	 *  and Telnet only those with a password. */
	function connectWithChoices(s: SessionConfig): SavedCredential[] {
		const k = sessionKind(s);
		return credentialsFor(savedCredentials, s.vault_id).filter((c) => k === 'ssh' || c.auth_method.type === 'Password');
	}
	/** Why a credential cannot be used once (a tooltip), or undefined. A
	 *  password manager entry is approved for the session it is linked to,
	 *  and the backend reads it from the stored session; the same holds for
	 *  the entries of the jump hosts and proxy, approved for its user name. */
	function connectWithBlocked(s: SessionConfig, c: SavedCredential): string | undefined {
		const a = c.auth_method;
		if ((a.type === 'Password' && a.password_ref) || (a.type === 'Key' && a.passphrase_ref)) return t('session.connect_with_ref');
		const routeRefs = (s.jump_chain ?? []).some((j) => (j.auth_method.type === 'Password' && j.auth_method.password_ref) || (j.auth_method.type === 'Key' && j.auth_method.passphrase_ref)) || !!s.proxy?.password_ref;
		if (routeRefs && c.username !== s.username) return t('session.connect_with_route');
		return undefined;
	}
	/** Whether the menu offers Connect to: another address, or another login. */
	function hasConnectChoices(s: SessionConfig): boolean {
		return hasAltAddresses(s) || (takesCredential(s) && connectWithChoices(s).length > 0);
	}
	function openConnectPage(s: SessionConfig): void {
		// Start from what a plain Connect would do: Auto, else the address
		// that worked last, else the main one; and the session's own login.
		const list = sessionAddresses(s);
		const last = lastAddress(s.id);
		menuAddress = connectMode(s) === 'auto' ? null : (list.find((a) => last && addressKey(a) === last)?.index ?? 0);
		menuCredential = null;
		menuNoTmux = false;
		sessionMenuPage = 'connect';
	}
	/** Connect to's Connect: the picked address with the picked login. */
	function connectChosen(session: SessionConfig): void {
		const address = menuAddress === null ? undefined : sessionAddresses(session).find((a) => a.index === menuAddress);
		const c = menuCredential ? savedCredentials.find((x) => x.id === menuCredential) : undefined;
		const noTmux = menuNoTmux;
		closeContextMenu();
		const s = c ? withCredential(session, c) : session;
		void handleConnect(noTmux ? withoutTmux(s) : s, address);
	}
	/** A session as one connect sees it with tmux left out: a plain shell,
	 *  this once. Never saved (like a container's shell). */
	function withoutTmux<T extends SessionConfig>(s: T): T {
		return { ...s, tmux: { mode: 'off' } };
	}

	function hasRefs(s: SessionConfig): boolean {
		const ref = (a: SessionConfig['auth_method']) => !!(a.type === 'Password' ? a.password_ref : a.type === 'Key' ? a.passphrase_ref : null);
		return ref(s.auth_method) || (s.jump_chain ?? []).some((j) => ref(j.auth_method)) || !!s.proxy?.password_ref;
	}

	/**
	 * Before connecting: the session's password manager entries, and those
	 * of the SSH session a VNC one goes through, must be approved by this
	 * user. Asks about any that are not; false when the user said no.
	 */
	async function approveRefsFirst(session: SessionConfig): Promise<boolean> {
		const ids: string[] = [];
		if (hasRefs(session)) ids.push(session.id);
		const via = sessionKind(session) === 'vnc' || sessionKind(session) === 'spice' ? session.via_session_id : null;
		if (via) {
			const viaSession = sessions.find((s) => s.id === via);
			if (!viaSession || hasRefs(viaSession)) ids.push(via);
		}
		for (const id of ids) {
			let asked: UntrustedRefs | null;
			try {
				asked = await sessionUntrustedSecretRefs(id);
			} catch {
				// The connect itself will say what is wrong.
				continue;
			}
			if (asked && !(await askToApprove(asked))) return false;
		}
		return true;
	}

	/**
	 * A web session: its own window (no tab), which the backend opens and
	 * signs in; or the system's browser, which nothing here can sign in.
	 */
	async function openWeb(session: SessionConfig): Promise<void> {
		try {
			if (session.web?.open_in === 'browser' || isMobile()) {
				await openUrl(webSessionUrl(session));
				addToast(t('web.opening_browser'), 'success');
			} else {
				await webSessionOpen(session.id);
			}
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	/** The session whose new SSH tab opens on the file manager, not the
	 *  terminal (Open file manager in its menu). */
	let filesOnConnect: string | undefined;

	/** Session menu > Open file manager: the full-pane file manager of an
	 *  SSH session. A connected tab of it switches over; otherwise it is
	 *  connected first. An RDP session's is the same machine over SFTP, in
	 *  a tab of its own (see #lib/sessions/rdp-files). */
	/** A server management session's controller over SSH, as the same user,
	 *  on the port its text console last used: a plain shell (racadm, CLP). */
	function openBmcSsh(session: SessionConfig): void {
		const port = rememberedConsole(session.id)?.port ?? BMC_SSH_PORT;
		setActivePage('terminal');
		consoleOnConnect = { id: session.id, command: '' };
		void handleConnect(bmcConsoleSession(session, port));
	}

	function openFileManager(session: SessionConfig): void {
		const open = getTabs().find((tab) => tab.type === 'ssh' && tab.sessionId === session.id && tab.connectionId && !tab.link);
		if (open) {
			activateTab(open.id);
			toggleFilesView(open.id, true);
			return;
		}
		filesOnConnect = session.id;
		void handleConnect(sessionKind(session) === 'rdp' ? rdpFilesSession(session) : session);
	}

	/** An RDP session's file manager found no SSH server: how to turn one on. */
	let sftpHelp = $state<{ session: SessionConfig; host: string; port: number; error: string } | undefined>();
	const SFTP_POWERSHELL = 'Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0\nStart-Service sshd\nSet-Service sshd -StartupType Automatic';
	let sftpCopied = $state(false);

	async function copySftpCommands(): Promise<void> {
		try {
			await navigator.clipboard.writeText(SFTP_POWERSHELL);
			sftpCopied = true;
			setTimeout(() => (sftpCopied = false), 1500);
		} catch (err) {
			addToast(String(err), 'error');
		}
	}

	function retrySftp(): void {
		const s = sftpHelp?.session;
		sftpHelp = undefined;
		if (s) openFileManager(s);
	}

	/** A server management session's text console being connected: the
	 *  command its tab types once logged in (see #lib/sessions/bmc-console). */
	let consoleOnConnect: { id: string; command: string } | undefined;

	/** A connect of a server management session's text console: the saved
	 *  session is a BMC one. */
	function isBmcConsole(session: SessionConfig): boolean {
		return sessionKind(session) === 'ssh' && sessions.some((s) => s.id === session.id && sessionKind(s) === 'bmc');
	}

	/** A connect of an RDP session's file manager: the saved session is RDP. */
	function isRdpFiles(session: SessionConfig): boolean {
		return sessionKind(session) === 'ssh' && sessions.some((s) => s.id === session.id && sessionKind(s) === 'rdp');
	}

	async function handleConnect(session: SessionConfig, address?: ResolvedAddress): Promise<void> {
		if (filesOnConnect !== session.id) filesOnConnect = undefined;
		// Not on this device (WinBox on a phone): say why rather than fail.
		const why = unsupported(session);
		if (why) {
			addToast(t(why), 'info');
			return;
		}
		// A session set to Ask: the address first.
		if (!address && sessionKind(session) === 'ssh' && connectMode(session) === 'ask') {
			pickAddressFor = session;
			return;
		}
		pickAddressFor = undefined;
		if (!(await approveRefsFirst(session))) return;
		connectAddress = address;
		noteSessionUsed(session.id);
		if (sessionKind(session) === 'vnc') {
			openVnc(session);
			return;
		}
		if (sessionKind(session) === 'spice') {
			openSpice(session);
			return;
		}
		if (sessionKind(session) === 'wsl') {
			void openWsl(session);
			return;
		}
		if (sessionKind(session) === 'telnet') {
			openTelnet(session);
			return;
		}
		if (sessionKind(session) === 'serial') {
			openSerial(session);
			return;
		}
		if (sessionKind(session) === 'winbox') {
			void openWinbox(session);
			return;
		}
		if (sessionKind(session) === 'web') {
			void openWeb(session);
			return;
		}
		if (sessionKind(session) === 'ftp') {
			openFtp(session);
			return;
		}
		if (sessionKind(session) === 'bmc') {
			openBmc(session);
			return;
		}
		if (sessionKind(session) === 'proxmox') {
			openProxmox(session);
			return;
		}
		if (session.files && isMobile()) {
			addToast(t('files.desktop_only'), 'error');
			return;
		}
		// Check if credentials are stored in the session (from vault)
		const storedPassword = session.auth_method.type === 'Password' ? session.auth_method.password : undefined;
		// A password manager entry stands in for a saved secret: the backend
		// fetches it at connect, so there is nothing to ask for.
		const passwordRef = session.auth_method.type === 'Password' ? (session.auth_method.password_ref ?? null) : null;

		if (sessionKind(session) === 'rdp' && (storedPassword || passwordRef)) {
			try {
				await openRdp(session, storedPassword ?? '', false, passwordRef);
			} catch (err) {
				addToast(String(err), 'error');
			}
			return;
		}
		const storedPassphrase = session.auth_method.type === 'Key' ? session.auth_method.passphrase : undefined;
		const passphraseRef = session.auth_method.type === 'Key' ? (session.auth_method.passphrase_ref ?? null) : null;

		// A key session has everything it needs the moment it names a key:
		// most keys have no passphrase at all, and an imported one carries its
		// own. Asking first made every passphrase-less key look broken — the
		// user typed a password into a passphrase box, and the key then failed
		// to load (issue #46). If the key really is locked, the backend says
		// so and the prompt appears then, which is the only time it helps.
		const keyReady =
			session.auth_method.type === 'Key' &&
			(!!session.auth_method.key_id || !!session.auth_method.path?.trim());

		// Auto-connect if we have stored credentials OR Agent auth
		if (storedPassword || storedPassphrase || passwordRef || passphraseRef || keyReady || session.auth_method.type === 'Agent') {
			connectSession = session;
			connectError = undefined;
			askSecret = false;
			rememberPassword = true;
			hasSavedPassword = !!storedPassword || !!storedPassphrase || !!passwordRef || !!passphraseRef;

			// With an entry from a password manager the stored plaintext is not
			// sent: the entry wins (an empty field here means "use the entry").
			if (session.auth_method.type === 'Password') {
				connectPassword = passwordRef ? '' : (storedPassword ?? '');
			} else if (session.auth_method.type === 'Key') {
				connectKeyPassphrase = passphraseRef ? '' : (storedPassphrase ?? '');
			}

			await doConnect();
			return;
		}

		// Otherwise show the prompt for credentials
		connectSession = session;
		connectPassword = '';
		connectKeyPassphrase = '';
		connectError = undefined;
		askSecret = true;
		rememberPassword = false;
		hasSavedPassword = false;
		passwordFallback = false;
		fallbackPassword = '';
	}

	async function doConnect(): Promise<void> {
		if (!connectSession) return;
		// A new attempt: the last one's steps, and the step it broke at, go.
		connectingId = undefined;
		// Don't submit an empty password while in fallback mode — that just
		// burns another round-trip with the same failure.
		if (passwordFallback && !fallbackPassword) {
			connectError = t('session.password_required');
			return;
		}
		connecting = true;
		connectError = undefined;

		const session = connectSession;

		if (sessionKind(session) === 'rdp') {
			const rdpRef = session.auth_method.type === 'Password' && !connectPassword ? (session.auth_method.password_ref ?? null) : null;
			if (!connectPassword && !rdpRef) {
				connectError = t('session.password_required');
				connecting = false;
				return;
			}
			try {
				await openRdp(session, connectPassword, rememberPassword, rdpRef);
				connectSession = undefined;
			} catch (err) {
				connectError = String(err);
			} finally {
				connecting = false;
			}
			return;
		}

		const id = crypto.randomUUID();
		connectingId = id;

		// If the user is in password-fallback mode, force password auth regardless of saved method.
		const authType = passwordFallback ? 'Password' : session.auth_method.type;

		const passwordToSave = authType === 'Password' ? (passwordFallback ? fallbackPassword : connectPassword) : connectKeyPassphrase;

		// Build jump chain from session config if present
		const hopParams = (j: JumpHostConfig): JumpHostConnectParams => ({
				host: j.host,
				port: j.port,
				username: j.username,
				authMethod: j.auth_method.type === 'Key' ? 'key' : j.auth_method.type.toLowerCase(),
				// A password manager entry wins; the plaintext goes only without one.
				password: j.auth_method.type === 'Password' && !j.auth_method.password_ref ? j.auth_method.password : undefined,
				passwordRef: j.auth_method.type === 'Password' ? (j.auth_method.password_ref ?? undefined) : undefined,
				keyPath: j.auth_method.type === 'Key' ? j.auth_method.path : undefined,
				keyId: j.auth_method.type === 'Key' ? j.auth_method.key_id : undefined,
				keyPassphrase: j.auth_method.type === 'Key' && !j.auth_method.passphrase_ref ? j.auth_method.passphrase : undefined,
				keyPassphraseRef: j.auth_method.type === 'Key' ? (j.auth_method.passphrase_ref ?? undefined) : undefined,
			});
		const jumpChain: JumpHostConnectParams[] | undefined = session.jump_chain && session.jump_chain.length > 0
			? session.jump_chain.map(hopParams)
			: undefined;
		// An address through another saved SSH session: that session's jump
		// hosts, then its server, logged in to as that session does. Its
		// password manager entries are its own, so they are not used here.
		const viaChain = (viaId: string): JumpHostConnectParams[] => {
			const via = sessions.find((s) => s.id === viaId && sessionKind(s) === 'ssh');
			if (!via) throw t('session.address_via_missing');
			const hops = [...(via.jump_chain ?? []), { host: via.host, port: via.port, username: via.username, auth_method: via.auth_method }].map(hopParams);
			if (hops.some((h) => h.passwordRef || h.keyPassphraseRef)) throw t('session.address_via_refs', { name: via.name });
			return hops;
		};
		const viaChains: Record<string, JumpHostConnectParams[]> = {};
		for (const a of sessionAddresses(session)) {
			if (!a.viaSessionId) continue;
			try {
				viaChains[a.viaSessionId] = viaChain(a.viaSessionId);
			} catch {
				// Said when that address is tried.
			}
		}

		// The addresses to try, in order: the one picked, the main one, or
		// all of them as Auto orders them.
		let targets: ResolvedAddress[];
		const all = sessionAddresses(session);
		if (connectAddress) targets = [connectAddress];
		else if (connectMode(session) !== 'auto') targets = [all[0]];
		else if (!checksAddresses(session, getSettings().checkAddresses)) {
			// Silent: nothing goes to an address before it is tried.
			targets = plainOrder(all, lastAddress(session.id));
		} else {
			const probes = await sshProbeAddresses(all.map((a) => (a.routed ? { host: '', port: 0 } : { host: a.host, port: a.port }))).catch(() => all.map(() => null));
			targets = autoOrder(all, probes, lastAddress(session.id));
			// Cancelled while checking.
			if (connectingId !== id) return;
		}

		let untrusted: UntrustedRefs | null = null;
		try {
			const sessionKeyPath = session.auth_method.type === 'Key' ? session.auth_method.path : undefined;
			const sessionKeyId = session.auth_method.type === 'Key' ? session.auth_method.key_id : undefined;
			const passwordToUse = authType === 'Password'
				? (passwordFallback ? fallbackPassword : connectPassword)
				: undefined;
			// The session's password manager entries, unless something was typed
			// in the prompt for this attempt: then that is what the user means.
			const passwordRef = authType === 'Password' && !passwordFallback && !passwordToUse && session.auth_method.type === 'Password'
				? (session.auth_method.password_ref ?? undefined)
				: undefined;
			const keyPassphraseRef = authType === 'Key' && !connectKeyPassphrase && session.auth_method.type === 'Key'
				? (session.auth_method.passphrase_ref ?? undefined)
				: undefined;
			const bmcConsole = isBmcConsole(session);
			const paramsFor = (addr: ResolvedAddress) => ({
				id,
				host: addr.host,
				port: addr.port,
				username: session.username,
				authMethod: authType === 'Key' ? 'key' : authType.toLowerCase(),
				password: passwordToUse,
				passwordRef,
				keyPath: authType === 'Key' && sessionKeyPath ? sessionKeyPath.trim() : undefined,
				keyId: authType === 'Key' && sessionKeyId ? sessionKeyId : undefined,
				keyPassphrase: authType === 'Key' && connectKeyPassphrase ? connectKeyPassphrase : undefined,
				keyPassphraseRef,
				cols: 80,
				rows: 24,
				shell: session.shell ?? undefined,
				// An X11 desktop starts the desktop, never tmux; a files
				// session (SFTP, SCP) only keeps a plain shell behind.
				tmux: session.x11_desktop || session.files ? null : tmuxView(tmuxFor(session, folders)),
				x11Desktop: session.files ? null : (session.x11_desktop ?? null),
				terminal: session.terminal ?? null,
				// A controller's command line is no shell to set up.
				injectColors: session.files || bmcConsole ? false : getSettings().typeShellSetup,
				sessionLog: session.files ? null : sessionLogConfig(session.name),
				showLoginMessage: getSettings().showLoginMessage,
				tryAgentKeys: authType === 'Key' && session.try_agent_keys === true,
				sshOptions: session.ssh_options ?? null,
				// Password manager entries are taken from the stored session.
				sessionId: session.id,
				// An address marked direct goes straight, without the session's
				// jump hosts or proxy.
				jumpChain: addr.viaSessionId ? viaChain(addr.viaSessionId) : addr.routed && !addr.tunnelOnly ? jumpChain : undefined,
				// The WireGuard tunnel this address goes through, brought up
				// (after asking) by sshConnect.
				wireguard: addr.wireguardId ?? null,
				proxy: addr.routed && !addr.viaSessionId && !addr.tunnelOnly && session.proxy ? {
					proxy_type: session.proxy.proxy_type,
					host: session.proxy.host,
					port: session.proxy.port,
					username: session.proxy.username ?? undefined,
					password: session.proxy.password_ref ? undefined : (session.proxy.password ?? undefined),
					password_ref: session.proxy.password_ref ?? undefined,
				} : undefined,
			});
			// A reconnect may need another address: the tab keeps them all.
			const fallback = hasAltAddresses(session) ? { addresses: all, jumpChain, viaChains, proxy: paramsFor(all[0]).proxy ?? undefined, sessionPort: session.port, check: checksAddresses(session, getSettings().checkAddresses) } : undefined;
			// Each address in turn, while the last one could not be reached.
			// A refused login stops here: it is the same server.
			let connectParams = paramsFor(targets[0]);
			let used = targets[0];
			const missed: string[] = [];
			for (let i = 0; ; i++) {
				used = targets[i];
				connectParams = paramsFor(used);
				tryingAddress = targets.length > 1 || connectAddress ? used : undefined;
				try {
					await sshConnect(connectParams);
					break;
				} catch (e) {
					const msg = String(e);
					if (connectingId !== id || i >= targets.length - 1 || !isUnreachable(msg)) {
						throw missed.length ? `${msg}\n\n${missed.join('\n')}` : e;
					}
					missed.push(`${addressName(used, session.port)}: ${msg}`);
				}
			}
			if (hasAltAddresses(session)) rememberAddress(session.id, used);
			if (fallback) Object.assign(connectParams, { fallback });
			// Asked which tmux session (or the old name or the new one): the
			// tab keeps the answer, so a reconnect goes straight back to it
			// rather than asking again.
			if (connectParams.tmux?.ask || connectParams.tmux?.legacy) {
				const chosen = await sshTmuxChosen(id).catch(() => undefined);
				connectParams.tmux = chosen === null ? null : { ...connectParams.tmux, name: chosen ?? connectParams.tmux.name, ask: false, legacy: undefined };
			}

			// Cancelled while the handshake was in flight: the backend had no
			// connection to drop yet, so this one is unwanted. Close it instead
			// of opening a tab. (Idea from the ReachQ fork by CPTProgrammer.)
			if (connectingId !== id) {
				sshDisconnect(id).catch(() => {});
				return;
			}

			if (bmcConsole && consoleOnConnect?.id === session.id) {
				Object.assign(connectParams, { afterLogin: consoleOnConnect.command });
				consoleOnConnect = undefined;
			}
			const tab = createTab('ssh', `${session.username}@${used.host}`, id, session.name, bmcConsole ? null : session.detected_os);
			tab.sshConnectParams = connectParams;
			tab.sessionId = session.id;
			tab.terminalTheme = session.terminal_theme ?? null;
			tab.cursorStyle = session.cursor_style ?? null;
			tab.cursorBlink = session.cursor_blink ?? null;
			tab.monitoring = session.monitoring ?? null;
			tab.historyMode = session.history ?? null;
			if (session.files) {
				// An SFTP or SCP session: straight into the file manager.
				tab.filesProtocol = session.files;
				tab.filesView = true;
				tab.monitoring = false;
				tab.historyMode = 'off';
			} else if (filesOnConnect === session.id) {
				filesOnConnect = undefined;
				toggleFilesView(tab.id, true);
			}
			addToast(t('session.connected_toast', { name: session.name }), 'success');
			void openOtherWindows(tab);
			connectSession = undefined;
			passwordFallback = false;
			fallbackPassword = '';

			// Detect OS in background and persist to session. A host saved as
			// plain Linux, unknown or Debian is asked again: older versions saved
			// that for MikroTik and UniFi devices and Raspberry Pi OS, which have
			// their own logos now.
			// Not for an RDP session's file manager: its OS says how the
			// remote desktop connects (GNOME), not what SSH would find.
			if (!isRdpFiles(session) && !bmcConsole && osNeedsDetection(session.detected_os, session.os_manual)) {
				sshDetectOs(id).then(async (osId) => {
					if (osId && osId !== session.detected_os) {
						updateTabOs(id, osId);
						try {
							// Only the OS changes, on the session as stored. The copy
							// connected with may carry a one-off override (a shell
							// inside a container), which must never be saved.
							const stored = await sessionGet(session.id);
							await sessionUpdate({ ...stored, detected_os: osId });
							await loadSessions();
						} catch {
							// Non-critical: icon will show next time
						}
					}
				}).catch(() => {});
			}
		} catch (err) {
			// Cancelled while connecting: the prompt is gone, so the failure
			// is nobody's business, and it must not switch the next prompt to
			// the password fallback or ask to approve entries.
			if (connectingId !== id) return;
			// The session's password manager entries are not approved (it
			// changed since the list was read): ask, below, then try again.
			untrusted = untrustedRefsFromError(err);
			const errStr = String(err);
			if (!untrusted && isRdpFiles(session) && isUnreachable(errStr)) {
				// An RDP session's file manager needs an SSH server there,
				// which a Windows machine has only with OpenSSH Server on:
				// the prompt goes, a dialog says how to turn it on.
				const saved = sessions.find((s) => s.id === session.id);
				if (saved) sftpHelp = { session: saved, host: session.host, port: rdpFilesPort(session), error: errStr };
				connectSession = undefined;
				return;
			}
			if (!untrusted) {
				connectError = errStr;
				askSecret = true;
			}
			// If the saved auth method got rejected and we haven't already tried the password fallback,
			// switch the modal to password mode so the user can retry without changing the session.
			const isAuthRejection = /auth/i.test(errStr) && (
				/reject/i.test(errStr) || /fail/i.test(errStr) || /denied/i.test(errStr)
			);
			// A locked key is not a rejected credential: the server never saw
			// one. The prompt is already open on the passphrase field, which is
			// what this needs — offering a password instead would be answering
			// a question nobody asked.
			const needsPassphrase = /passphrase/i.test(errStr);
			if (!untrusted && isAuthRejection && !needsPassphrase && !passwordFallback && session.auth_method.type !== 'Password') {
				passwordFallback = true;
				fallbackPassword = '';
			}
		} finally {
			// A cancelled attempt finishing late must not mark a newer one,
			// still in flight, as done.
			if (connectingId === id) {
				connecting = false;
				tryingAddress = undefined;
			}
		}
		if (untrusted) {
			if (await askToApprove(untrusted)) await doConnect();
			else cancelConnect();
		}
	}

	/** The password manager entry the prompt's secret field stands in for, if any. */
	function promptRef(session: SessionConfig): SecretRef | null {
		const a = session.auth_method;
		if (a.type === 'Password') return a.password_ref ?? null;
		if (a.type === 'Key') return a.passphrase_ref ?? null;
		return null;
	}

	function cancelConnect(): void {
		if (connecting && connectingId) {
			// Try to clean up the in-flight connection on the backend
			sshDisconnect(connectingId).catch(() => {});
		}
		connecting = false;
		connectingId = undefined;
		connectSession = undefined;
		connectAddress = undefined;
		tryingAddress = undefined;
		passwordFallback = false;
		fallbackPassword = '';
	}

	// ---- Locate a tab's session in the list (the tab menu's) ------------------
	let listEl = $state<HTMLDivElement | undefined>();
	let vaultSelector = $state<{ select: (filter: VaultFilter) => void } | undefined>();
	/** The session just located, marked for a moment. */
	let locatedId = $state<string | null>(null);
	let locatedTimer: ReturnType<typeof setTimeout> | undefined;

	/** Show a session in the list: whatever hides it (the search, a filter,
	 *  another vault, a folded folder) is put aside, then it is scrolled to,
	 *  focused and marked. */
	async function locateSession(id: string): Promise<void> {
		tab = 'sessions';
		let session = sessions.find((s) => s.id === id);
		if (!session) {
			await loadSessions();
			session = sessions.find((s) => s.id === id);
		}
		if (!session) {
			addToast(t('tabs.session_gone'), 'error');
			return;
		}
		if (!filteredSessions.some((s) => s.id === id)) {
			searchQuery = '';
			filterFavorites = false;
			filterKinds = [];
			filterTags = [];
		}
		if (!filteredSessions.some((s) => s.id === id)) vaultSelector?.select(ALL_VAULTS);
		const open = new Set(collapsedFolders);
		const seen = new Set<string>();
		for (let f = session.folder_id; f && !seen.has(f); f = folders.find((x) => x.id === f)?.parent_id ?? null) {
			seen.add(f);
			open.delete(f);
		}
		if (open.size !== collapsedFolders.size) {
			collapsedFolders = open;
			localStorage.setItem('collapsedFolders', JSON.stringify([...open]));
		}
		await tick();
		const row = listEl?.querySelector<HTMLElement>(`[data-session-id="${CSS.escape(id)}"]`);
		if (!row) return;
		row.scrollIntoView({ block: 'center', behavior: 'smooth' });
		row.querySelector<HTMLElement>('.card-main')?.focus({ preventScroll: true });
		locatedId = id;
		clearTimeout(locatedTimer);
		locatedTimer = setTimeout(() => (locatedId = null), 1800);
	}

	function handleEdit(session: SessionConfig): void {
		editingSession = session;
		cloningSession = undefined;
		showEditor = true;
	}

	/** Opens the editor on a copy; nothing is saved until Save. */
	let cloningSession = $state<SessionConfig | undefined>();
	function handleClone(session: SessionConfig): void {
		editingSession = undefined;
		newSessionFolder = null;
		cloningSession = session;
		showEditor = true;
	}

	async function handleDelete(session: SessionConfig): Promise<void> {
		if (deleteConfirm !== session.id) {
			deleteConfirm = session.id;
			// Auto-clear confirmation after 3 seconds
			setTimeout(() => {
				deleteConfirm = null;
			}, 3000);
			return;
		}

		try {
			await sessionDelete(session.id);
			forgetSession(session.id);
			deleteConfirm = null;
			await loadSessions();
		} catch (err) {
			console.error('Delete failed:', err);
			addToast(String(err), 'error');
		}
	}

	function handleNewSession(): void {
		editingSession = undefined;
		cloningSession = undefined;
		newSessionFolder = null;
		showEditor = true;
	}

	function handleEditorSave(): void {
		loadSessions();
	}

	// TLS-style: initialize identity (generates X25519 keypair, stores in OS keychain)
	async function handleInitialize(): Promise<void> {
		initializing = true;
		initError = '';
		try {
			await initIdentity(''); // No password needed - TLS-style
			await loadSessions();
			addToast(t('session.identity_created_toast'), 'success');
		} catch (err) {
			initError = String(err);
		} finally {
			initializing = false;
		}
	}

	// Import identity from backup key
	async function handleImport(): Promise<void> {
		if (!importKey.trim()) {
			initError = t('session.enter_backup_key');
			return;
		}
		importing = true;
		initError = '';
		try {
			await importIdentity(importKey.trim());
			await loadSessions();
			importKey = '';
			addToast(t('session.identity_restored_toast'), 'success');
		} catch (err) {
			initError = String(err);
		} finally {
			importing = false;
		}
	}

	// First run on another device: open the vaults the first one syncs
	async function handleLink(): Promise<void> {
		importing = true;
		initError = '';
		try {
			await linkDevice(linkCode.trim());
			linkCode = '';
			await loadSessions();
			addToast(t('sync.joined_toast'), 'success');
		} catch (err) {
			initError = String(err);
		} finally {
			importing = false;
		}
	}

	async function handleScanLink(): Promise<void> {
		initError = '';
		try {
			const code = await scanLinkCode();
			if (code) {
				linkCode = code;
				await handleLink();
			}
		} catch (err) {
			initError = String(err);
		}
	}

	// Reset - delete existing data and start fresh
	async function handleReset(): Promise<void> {
		// Delete identity file and vaults to start fresh
		try {
			const { invoke } = await import('@tauri-apps/api/core');
			await invoke('vault_reset');
			await checkState();
			addToast(t('session.data_cleared_toast'), 'info');
		} catch (err) {
			initError = String(err);
		}
	}

	// Load sessions and vaults on mount (auto-unlock via OS keychain).
	//
	// Every path out of here must clear `loading`. Without the catch, a single
	// rejected vault IPC left the spinner up forever and the session list never
	// appeared — the panel looked like it was loading endlessly. That is easy to
	// hit in normal use: open a session, use the file manager (which reads
	// credentials out of the vault), then switch back to Sessions, which
	// remounts this component and re-runs the whole check against a vault that
	// is busy.
	//
	// checkState() already refreshes the vault list once it unlocks, so the
	// second refreshVaults() that used to be here was a redundant round-trip and
	// one more thing that could reject.
	$effect(() => {
		untrack(() => {
			checkState()
				.then(async () => {
					if (!vaultState.locked) {
						await loadSessions();
					} else {
						loading = false;
					}
				})
				.catch((err) => {
					console.error('Failed to load the session list:', err);
					addToast(String(err), 'error');
					loading = false;
				});
		});
	});
</script>

{#snippet newFolderRow()}
	<div class="new-folder-row">
		<svg width="14" height="14" viewBox="0 0 24 24" fill="none" class="new-folder-icon"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
		{#if creatingFolderParentId}<span class="new-folder-parent">{folderPath(creatingFolderParentId, folders)} /</span>{/if}
		<form class="new-folder-form" onsubmit={(e) => { e.preventDefault(); handleCreateFolder(); }}>
			<input class="new-folder-input" type="text" placeholder={t('session.folder_name')} bind:value={newFolderName} />
			<button class="new-folder-save" type="submit" disabled={!newFolderName.trim()}>{t('common.save')}</button>
			<button class="new-folder-cancel" type="button" onclick={() => { creatingFolder = false; creatingFolderParentId = null; pendingMove = null; newFolderName = ''; }}>{t('common.cancel')}</button>
		</form>
	</div>
{/snippet}

{#snippet sessionRow(session: SessionConfig, depth: number)}
	{#if deleteConfirm === session.id}
		<div class="delete-confirm" style:margin-left={depth >= 0 ? `${Math.min(depth + 1, 8) * 14}px` : undefined}>
			<span class="delete-confirm-text">{t('session.delete_confirm', { name: session.name })}</span>
			<button class="delete-confirm-btn" onclick={() => handleDelete(session)}>{t('common.confirm')}</button>
			<button class="delete-cancel-btn" onclick={() => (deleteConfirm = null)}>{t('common.cancel')}</button>
		</div>
	{:else}
		<div class:folder-session={depth >= 0} class:drop-before={dropSlot?.id === session.id && dropSlot.before} class:drop-after={dropSlot?.id === session.id && !dropSlot.before} class:located={locatedId === session.id} data-session-id={session.id} style:padding-left={depth >= 0 ? `${Math.min(depth + 1, 8) * 14}px` : undefined}>
			<SessionCard {session} vault={selectedVaultId === ALL_VAULTS && session.vault_id ? (vaultState.vaults.get(session.vault_id) ?? null) : null} onconnect={() => handleConnect(session)} onedit={() => handleEdit(session)} ondelete={() => handleDelete(session)} oncontextmenu={(e) => openSessionContextMenu(e, session)} ondragstart={(e) => handleDragStart(e, session)} ondragend={() => {}} {selecting} selected={selected.has(session.id)} onselect={(e) => toggleSelect(session, e)} open={tabStates.has(session.id)} current={tabStates.get(session.id)?.current ?? false} connected={tabStates.get(session.id)?.connected ?? false} viaName={session.via_session_id ? (sessions.find((s) => s.id === session.via_session_id)?.name ?? null) : null} />
		</div>
	{/if}
{/snippet}

{#snippet folderNode(node: FolderTreeNode)}
	<div class="folder-node" data-folder-id={node.folder.id}>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="folder-header" class:drop-active={dropTarget === node.folder.id && dragging && !dropSlot} data-folder-id={node.folder.id} style:padding-left={`${6 + Math.min(node.depth, 8) * 14}px`} oncontextmenu={(e) => openFolderContextMenu(e, node.folder)} use:longPress={renamingFolderId !== node.folder.id}>
			{#if renamingFolderId === node.folder.id}
			<form class="folder-rename" onsubmit={(e) => { e.preventDefault(); void commitRename(); }}>
				<input class="new-folder-input" type="text" aria-label={t('folder.rename')} bind:value={renameValue} use:focusSelect onblur={() => void commitRename()} onkeydown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); renamingFolderId = null; } }} />
			</form>
			{:else}
			<button class="folder-toggle" onclick={() => toggleFolder(node.folder.id)} onkeydown={(e) => { if (e.key === 'F2') { e.preventDefault(); startRename(node.folder); } }} aria-expanded={!collapsedFolders.has(node.folder.id)} title={folderPath(node.folder.id, folders)}>
				<svg width="10" height="10" viewBox="0 0 10 10" fill="none" class="folder-chevron" class:collapsed={collapsedFolders.has(node.folder.id)}><path d="M3 2l4 3-4 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>
				<FolderIcon icon={node.folder.icon} color={node.folder.color} />
				<span class="folder-name">{node.folder.name}</span><span class="folder-count">{node.totalSessions}</span>
			</button>
			{/if}
			<button class="folder-menu-btn" onclick={(e) => openFolderMenu(e, node.folder)} aria-label={t('folder.menu')} title={t('folder.menu')}><svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor"><circle cx="2" cy="6" r="1.2"/><circle cx="6" cy="6" r="1.2"/><circle cx="10" cy="6" r="1.2"/></svg></button>
			<button class="folder-delete-btn" onclick={() => askDeleteFolder(node.folder)} title={t('folder.delete')} aria-label={t('folder.delete')}><svg width="10" height="10" viewBox="0 0 10 10" fill="none"><path d="M1 1l8 8M9 1L1 9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg></button>
		</div>
		{#if searchQuery.trim() || !collapsedFolders.has(node.folder.id)}
			{#each node.sessions as session (session.id)}{@render sessionRow(session, node.depth)}{/each}
			{#each node.children as child (child.folder.id)}{@render folderNode(child)}{/each}
		{/if}
	</div>
{/snippet}

<div class="session-list" bind:this={listEl}>
	{#if !hasIdentity}
		<!-- First run: Initialize identity (TLS-style, no password) -->
		<div class="init-section">
			<div class="init-icon">
				<svg width="32" height="32" viewBox="0 0 24 24" fill="none">
					<path d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
				</svg>
			</div>
			<p class="init-title">{t('session.secure_sessions')}</p>
			<p class="init-desc">{t('session.secure_sessions_desc')}</p>
			{#if initError}
				<p class="init-error">{initError}</p>
			{/if}
			<button class="init-btn" onclick={handleInitialize} disabled={initializing || importing}>
				{#if initializing}{t('session.initializing')}{:else}{t('session.initialize')}{/if}
			</button>
			<!-- Or join the vaults another device already syncs -->
			<p class="init-desc">{t('session.link_device_hint')}</p>
			<PasswordField disabled={importing || initializing}>
				{#snippet children(type)}
					<input
						class="import-input"
						{type}
						placeholder={t('session.paste_link_code')}
						bind:value={linkCode}
						disabled={importing || initializing}
					/>
				{/snippet}
			</PasswordField>
			<div class="recovery-buttons">
				{#if canScanLinkCode()}
					<button class="reset-btn" onclick={handleScanLink} disabled={importing || initializing}>{t('sync.scan_qr')}</button>
				{/if}
				<button class="reset-btn" onclick={handleLink} disabled={importing || initializing || !linkCode.trim()}>
					{#if importing}{t('sync.joining')}{:else}{t('sync.join')}{/if}
				</button>
			</div>
		</div>
	{:else if keychainError}
		<!-- Keychain error: data exists but can't access key -->
		<div class="init-section">
			<div class="init-icon error">
				<svg width="32" height="32" viewBox="0 0 24 24" fill="none">
					<path d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
				</svg>
			</div>
			<p class="init-title">{t('session.keychain_error')}</p>
			<p class="init-desc">{t('session.keychain_error_desc')}</p>
			{#if initError}
				<p class="init-error">{initError}</p>
			{/if}
			<PasswordField disabled={importing}>
				{#snippet children(type)}
					<input
						class="import-input"
						{type}
						placeholder={t('session.paste_backup_key')}
						bind:value={importKey}
						disabled={importing}
					/>
				{/snippet}
			</PasswordField>
			<div class="recovery-buttons">
				<button class="init-btn" onclick={handleImport} disabled={importing || !importKey.trim()}>
					{#if importing}{t('session.restoring')}{:else}{t('session.restore_identity')}{/if}
				</button>
				<button class="reset-btn" onclick={handleReset} disabled={importing}>
					{t('session.start_fresh')}
				</button>
			</div>
		</div>
	{:else if locked}
		<!-- Locked but has identity - keychain access failed -->
		<div class="init-section">
			<p class="init-desc">{t('session.keychain_locked_desc')}</p>
		</div>
	{:else}
		<VaultSelector bind:this={vaultSelector} onvaultselect={(filter) => { selectedVaultId = filter; creatingFolder = false; creatingFolderParentId = null; pendingMove = null; newFolderName = ''; }} onrefresh={() => loadSessions()} />

		<div class="list-tabs" role="tablist">
			<button class="list-tab" class:active={tab === 'sessions'} role="tab" aria-selected={tab === 'sessions'} onclick={() => (tab = 'sessions')}>{t('credential.tab_sessions')}</button>
			<button class="list-tab" class:active={tab === 'credentials'} role="tab" aria-selected={tab === 'credentials'} onclick={() => (tab = 'credentials')}>{t('credential.tab_credentials')}</button>
		</div>

		{#if tab === 'credentials'}
			<CredentialList vaultFilter={selectedVaultId} {targetVaultId} {sessions} {folders} onchange={() => void loadSessions()} />
		{:else}
		<div class="tool-row">
			<div class="search-row">
				<span class="search-icon"><FaIcon icon={faMagnifyingGlass} size={11} /></span>
				<input
					class="search-input"
					type="text"
					placeholder={t('session.search_or_connect')}
					bind:value={searchQuery}
					onkeydown={onSearchKeydown}
				/>
				{#if searchQuery}
					<button class="search-clear" onclick={() => (searchQuery = '')} aria-label={t('common.clear')}>
						<FaIcon icon={faXmark} size={10} />
					</button>
				{/if}
			</div>

			<div class="add-wrap" bind:this={addMenuEl}>
				<button
					class="add-btn"
					class:open={showAddMenu}
					aria-haspopup="menu"
					aria-expanded={showAddMenu}
					title={t('session.add')}
					aria-label={t('session.add')}
					onclick={toggleAddMenu}
				>
					<FaIcon icon={faPlus} size={13} />
				</button>
				{#if showAddMenu}
					<div class="add-menu pop-in" role="menu" use:positionMenu={{ ...addMenuAnchor, align: 'end' }}>
						<button class="menu-item" role="menuitem" onclick={() => fromMenu(handleNewSession)}>
							<FaIcon icon={faServer} size={12} />
							<span>{t('session.new_session')}</span>
							{#if actionLabel('session.new')}<kbd>{actionLabel('session.new')}</kbd>{/if}
						</button>
						<button class="menu-item" role="menuitem" onclick={() => fromMenu(() => { quickPrefill = null; showQuickConnect = true; })}>
							<FaIcon icon={faBolt} size={12} />
							<span>{t('session.quick_connect')}</span>
							{#if actionLabel('session.quickConnect')}<kbd>{actionLabel('session.quickConnect')}</kbd>{/if}
						</button>
						<button class="menu-item" role="menuitem" onclick={() => fromMenu(() => void openVvFile())}>
							<FaIcon icon={faDisplay} size={12} />
							<span>{t('spice.open_vv')}</span>
						</button>
						<button class="menu-item" role="menuitem" onclick={() => fromMenu(() => contextNewFolder())}>
							<FaIcon icon={faFolderPlus} size={12} />
							<span>{t('session.new_folder')}</span>
						</button>
						<div class="menu-sep"></div>
						<!-- Sharing lives in the bottom bar too, but that bar only exists
						     under a terminal: someone joining usually has none open. -->
						<button class="menu-item" role="menuitem" onclick={() => fromMenu(() => share.openDialog('host'))}>
							<FaIcon icon={faShareNodes} size={12} />
							<span>{t('sharing.start')}</span>
						</button>
						<button class="menu-item" role="menuitem" onclick={() => fromMenu(() => share.openDialog('guest'))}>
							<FaIcon icon={faRightToBracket} size={12} />
							<span>{t('sharing.join')}</span>
						</button>
						<div class="menu-sep"></div>
						<button class="menu-item" role="menuitem" onclick={() => fromMenu(() => openImport(lastImportSource))}>
							<FaIcon icon={faFileImport} size={12} />
							<span>{t('session.import_sessions_menu')}</span>
						</button>
					</div>
				{/if}
			</div>
		</div>

		{#if !selecting}
			<div class="list-toolbar" role="toolbar" aria-label={t('session.list_tools')}>
				<button class="tb-btn" type="button" onclick={() => { selectMode = true; }} title={t('session.select_sessions')} aria-label={t('session.select_sessions')}>
					<FaIcon icon={faListCheck} size={13} />
				</button>
				<button class="tb-btn" type="button" onclick={selectAllShown} disabled={filteredSessions.length === 0} title={`${t('session.select_all')} (${shortcutLabel('a', { ctrl: true })})`} aria-label={t('session.select_all')}>
					<FaIcon icon={faCheckDouble} size={13} />
				</button>
				<span class="tb-sep"></span>
				<button class="tb-btn" type="button" onclick={() => setAllFolders(anyExpanded)} disabled={shownFolders.length === 0} title={anyExpanded ? t('session.collapse_all') : t('session.expand_all')} aria-label={anyExpanded ? t('session.collapse_all') : t('session.expand_all')}>
					<FaIcon icon={anyExpanded ? faAnglesUp : faAnglesDown} size={13} />
				</button>
				<button class="tb-btn" class:on={sortKey !== 'default'} type="button" onclick={(e) => openViewMenu(e, 'sort')} title={t('session.sort_by')} aria-label={t('session.sort_by')} aria-haspopup="menu" aria-expanded={viewMenu?.kind === 'sort'}>
					<FaIcon icon={faArrowDownWideShort} size={13} />
				</button>
				<button class="tb-btn" class:on={filtering} type="button" onclick={(e) => openViewMenu(e, 'filter')} title={t('session.filter')} aria-label={t('session.filter')} aria-haspopup="menu" aria-expanded={viewMenu?.kind === 'filter'}>
					<FaIcon icon={faFilter} size={12} />
				</button>
				<span class="tb-grow"></span>
				<button class="tb-btn" type="button" onclick={() => contextNewFolder()} title={t('session.new_folder')} aria-label={t('session.new_folder')}>
					<FaIcon icon={faFolderPlus} size={13} />
				</button>
				<button class="tb-btn" type="button" onclick={() => openImport(lastImportSource)} title={t('session.import_sessions_menu')} aria-label={t('session.import_sessions_menu')}>
					<FaIcon icon={faFileImport} size={13} />
				</button>
			</div>
			{#if filtering}
				<div class="filter-note">
					<span>{t('session.filter_on', { count: String(filteredSessions.length) })}</span>
					<button class="bulk-link" type="button" onclick={clearFilters}>{t('session.filter_clear')}</button>
				</div>
			{/if}
		{/if}

		{#if viewMenu}
			<ContextMenuBackdrop onclose={() => (viewMenu = null)} />
			<div class="context-menu pop-in" role="menu" use:positionMenu={{ x: viewMenu.x, y: viewMenu.y }}>
				{#if viewMenu.kind === 'sort'}
					<div class="context-label">{t('session.sort_by')}</div>
					{#each [['default', 'session.sort_default'], ['name', 'session.sort_name'], ['host', 'session.sort_host'], ['recent', 'session.sort_recent']] as const as [key, label] (key)}
						<button class="context-item" class:context-current={sortKey === key} role="menuitemradio" aria-checked={sortKey === key} type="button" onclick={() => { sortKey = key; viewMenu = null; }}>{t(label)}</button>
					{/each}
				{:else}
					<button class="context-item check-item" role="menuitemcheckbox" aria-checked={filterFavorites} type="button" onclick={() => (filterFavorites = !filterFavorites)}>
						<span class="check">{#if filterFavorites}<FaIcon icon={faCheck} size={10} />{/if}</span>{t('session.filter_favorites')}
					</button>
					<div class="context-sep"></div>
					<div class="context-label">{t('session.filter_type')}</div>
					{#each KINDS as kind (kind.id)}
						<button class="context-item check-item" role="menuitemcheckbox" aria-checked={filterKinds.includes(kind.id)} type="button" onclick={() => (filterKinds = toggleIn(filterKinds, kind.id))}>
							<span class="check">{#if filterKinds.includes(kind.id)}<FaIcon icon={faCheck} size={10} />{/if}</span>{kind.label}
						</button>
					{/each}
					{#if allTags.length}
						<div class="context-sep"></div>
						<div class="context-label">{t('session.filter_tags')}</div>
						<div class="context-folders">
							{#each allTags as tag (tag)}
								<button class="context-item check-item" role="menuitemcheckbox" aria-checked={filterTags.includes(tag)} type="button" onclick={() => (filterTags = toggleIn(filterTags, tag))}>
									<span class="check">{#if filterTags.includes(tag)}<FaIcon icon={faCheck} size={10} />{/if}</span>{tag}
								</button>
							{/each}
						</div>
					{/if}
					{#if filtering}
						<div class="context-sep"></div>
						<button class="context-item" type="button" onclick={() => { clearFilters(); viewMenu = null; }}>{t('session.filter_clear')}</button>
					{/if}
				{/if}
			</div>
		{/if}

		{#if addressSuggestion}
			<button class="connect-suggestion" onclick={connectToAddress}>
				<FaIcon icon={faBolt} size={12} />
				<span class="suggestion-text">
					{t('session.connect_to_address', {
						address: `${addressSuggestion.username ? addressSuggestion.username + '@' : ''}${addressSuggestion.host}${addressSuggestion.port ? ':' + addressSuggestion.port : ''}`
					})}
				</span>
				{#if filteredSessions.length === 0}<kbd>↵</kbd>{/if}
			</button>
		{/if}

		{#if searchRegex.error}
			<p class="search-error" role="alert">{t('session.regex_error', { error: searchRegex.error })}</p>
		{/if}

		{#if selecting}
			<div class="bulk-bar" role="toolbar" aria-label={t('session.selection')}>
				<span class="bulk-count">{t('session.selected_n', { count: String(selected.size) })}</span>
				{#if selected.size < filteredSessions.length}
					<button class="bulk-link" type="button" onclick={selectAllShown}>{t('session.select_all_n', { count: String(filteredSessions.length) })}</button>
				{/if}
				<span class="bulk-actions">
					<button class="tb-btn" type="button" disabled={selected.size === 0} onclick={openBulkEdit} title={t('bulk.edit_hint')} aria-label={t('bulk.edit')}>
						<FaIcon icon={faPen} size={12} />
					</button>
					<button class="tb-btn" type="button" disabled={selected.size === 0} onclick={() => openMove([...selected])} title={t('session.move_to')} aria-label={t('session.move_to')}>
						<FaIcon icon={faFolderOpen} size={13} />
					</button>
					<button class="tb-btn" type="button" disabled={selected.size === 0} onclick={() => void favoriteSelected()} title={t('session.add_favorite')} aria-label={t('session.add_favorite')}>
						<FaIcon icon={faStar} size={12} />
					</button>
					<button class="tb-btn danger" type="button" disabled={selected.size === 0} onclick={() => (bulkDeleteConfirm = true)} title={t('common.delete')} aria-label={t('common.delete')}>
						<FaIcon icon={faTrashCan} size={12} />
					</button>
					<span class="tb-sep"></span>
					<button class="tb-btn" type="button" onclick={clearSelection} title={t('session.done_selecting')} aria-label={t('session.done_selecting')}>
						<FaIcon icon={faXmark} size={13} />
					</button>
				</span>
			</div>
		{:else if filteredSessions.length > 1 && searchQuery.trim()}
			<!-- Only for a search: at rest it repeated the + menu's Select sessions on
			     a row of its own, above the list it pushed down. -->
			<div class="match-row">
				<span>{t('session.matches_n', { count: String(filteredSessions.length) })}</span>
				<span class="match-actions">
					<button class="match-select" type="button" onclick={() => { selectMode = true; }}>{t('session.select_sessions')}</button>
					<button class="match-select" type="button" onclick={selectAllShown}>{t('session.select_all')}</button>
				</span>
			</div>
		{/if}

		{#if loading}
			<div class="loading-state"><span class="spinner"></span><span class="loading-text">{t('session.loading')}</span></div>
		{:else if filteredSessions.length === 0 && folderTree.roots.length === 0}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="empty-state-area" data-folder-id="__ungrouped__" class:drop-active={dropTarget === null && dragging && !dropSlot} oncontextmenu={openBackgroundContextMenu}>
				{#if creatingFolder}{@render newFolderRow()}{:else}<p class="empty-state">{searchQuery.trim() ? t('session.no_matches') : t('session.no_sessions_vault')}</p>{/if}
			</div>
		{:else}
			{#if creatingFolder}{@render newFolderRow()}{/if}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<!-- svelte-ignore a11y_click_events_have_key_events -->
			<div class="sessions-scroll" data-folder-id="__ungrouped__" class:drop-active={dropTarget === null && dragging && !dropSlot} oncontextmenu={openBackgroundContextMenu} onclick={closeContextMenu} onkeydown={onListKeydown}>
				{#if dragging}
					<div class="no-folder-drop" data-folder-id="__ungrouped__" class:drop-active={dropTarget === null && !dropSlot}><FaIcon icon={faFolder} size={12} /><span>{t('session.no_folder')}</span></div>
				{/if}
				{#each folderTree.roots as node (node.folder.id)}{@render folderNode(node)}{/each}
				<div class="ungrouped-drop" data-folder-id="__ungrouped__">
					{#each folderTree.ungrouped as session (session.id)}{@render sessionRow(session, -1)}{/each}
				</div>
			</div>
		{/if}
		{/if}
	{/if}
	{#if contextMenu}
		<ContextMenuBackdrop onclose={closeContextMenu} />
		<div class="context-menu" class:pop-in={!sheetMenu} class:context-sheet={sheetMenu} use:positionMenu={{ x: contextMenu.x, y: contextMenu.y }}>
			{#if sheetMenu && (contextMenu.session || contextMenu.folder)}
				<div class="context-sheet-title">{contextMenu.session ? (selected.has(contextMenu.session.id) && selected.size > 1 ? t('session.selected_n', { count: String(selected.size) }) : contextMenu.session.name) : contextMenu.folder?.name}</div>
			{/if}
			{#if contextMenu.session && sessionMenuPage === 'connect'}
				{@const menuSession = contextMenu.session}
				{@const linked = menuSession.credential && !menuSession.credential.missing ? menuSession.credential.id : null}
				<button class="context-item context-back" onclick={() => (sessionMenuPage = 'main')} type="button">
					<FaIcon icon={faChevronLeft} size={9} />
					<span>{t('session.connect_to_menu')}</span>
				</button>
				<div class="context-sep"></div>
				<div class="context-folders context-folders-tall" role="group">
					{#if hasAltAddresses(menuSession)}
						<div class="context-label">{t('session.connect_address')}</div>
						{#if connectMode(menuSession) === 'auto'}
							<button class="context-item context-via" class:context-current={menuAddress === null} role="menuitemradio" aria-checked={menuAddress === null} onclick={() => (menuAddress = null)} type="button">
								{t('session.connect_mode_auto')}
							</button>
						{/if}
						{#each sessionAddresses(menuSession) as addr (addr.index)}
							<button class="context-item context-via" class:context-current={menuAddress === addr.index} role="menuitemradio" aria-checked={menuAddress === addr.index} onclick={() => (menuAddress = addr.index)} type="button">
								{addressName(addr, menuSession.port)}
								{#if addr.label}<span class="context-hint">{addressText(addr, menuSession.port)}</span>{/if}
							</button>
						{/each}
					{/if}
					{#if takesCredential(menuSession) && connectWithChoices(menuSession).length > 0}
						<div class="context-label" title={t('session.connect_with_hint')}>{t('session.connect_login')}</div>
						<button class="context-item context-via" class:context-current={menuCredential === null} role="menuitemradio" aria-checked={menuCredential === null} onclick={() => (menuCredential = null)} type="button">
							{t('session.connect_login_own')}
							{#if menuSession.username}<span class="context-hint">{menuSession.domain?.trim() ? `${menuSession.domain}\\` : ''}{menuSession.username}</span>{/if}
						</button>
						{#each connectWithChoices(menuSession).filter((c) => c.id !== linked) as c (c.id)}
							{@const blocked = connectWithBlocked(menuSession, c)}
							<button class="context-item context-via" class:context-current={menuCredential === c.id} role="menuitemradio" aria-checked={menuCredential === c.id} disabled={!!blocked} title={blocked ?? t('session.connect_with_hint')} onclick={() => (menuCredential = c.id)} type="button">
								{c.name}
								<span class="context-hint">{c.domain?.trim() ? `${c.domain}\\` : ''}{c.username}</span>
							</button>
						{/each}
					{/if}
					{#if tmuxFor(menuSession, folders)}
						<div class="context-label">tmux</div>
						<button class="context-item context-via" class:context-current={menuNoTmux} role="menuitemcheckbox" aria-checked={menuNoTmux} onclick={() => (menuNoTmux = !menuNoTmux)} type="button">
							{t('session.connect_without_tmux')}
							<span class="context-hint">{menuNoTmux ? '✓' : ''}</span>
						</button>
					{/if}
				</div>
				<div class="context-sep"></div>
				<button class="context-item context-go" onclick={() => connectChosen(menuSession)} type="button">
					{t('session.connect')}
				</button>
			{:else if contextMenu.session}
				<button class="context-item" onclick={() => { if (contextMenu?.session) handleConnect(contextMenu.session); closeContextMenu(); }} type="button">
					{t('session.connect')}
				</button>
				{#if (sessionKind(contextMenu.session) === 'ssh' || sessionKind(contextMenu.session) === 'rdp') && !isMobile()}
					<button class="context-item" onclick={() => { if (contextMenu?.session) openFileManager(contextMenu.session); closeContextMenu(); }} type="button" title={sessionKind(contextMenu.session) === 'rdp' ? t('session.rdp_files_hint') : undefined}>
						{sessionKind(contextMenu.session) === 'rdp' ? t('session.open_file_manager_sftp') : t('session.open_file_manager')}
					</button>
				{/if}
				{#if sessionKind(contextMenu.session) === 'bmc'}
					<button class="context-item" onclick={() => { if (contextMenu?.session) openBmcSsh(contextMenu.session); closeContextMenu(); }} type="button" title={t('session.open_ssh_bmc_hint')}>
						{t('session.open_ssh')}
					</button>
				{/if}
				{#if tmuxFor(contextMenu.session, folders)}
					<button class="context-item" onclick={() => { if (contextMenu?.session) handleConnect(withoutTmux(contextMenu.session)); closeContextMenu(); }} type="button">
						{t('session.connect_no_tmux')}
					</button>
				{/if}
				{#if hasConnectChoices(contextMenu.session)}
					{@const menuSession = contextMenu.session}
					<button class="context-item context-sub" title={t('session.connect_to_hint')} onclick={() => openConnectPage(menuSession)} type="button">
						<span>{t('session.connect_to_menu')}</span>
						<FaIcon icon={faChevronRight} size={9} />
					</button>
				{/if}
				<div class="context-sep"></div>
				{#if selected.has(contextMenu.session.id) && selected.size > 1}
					<button class="context-item" onclick={openBulkEdit} type="button">
						{t('bulk.edit_n', { count: String(selected.size) })}
					</button>
				{:else}
					<button class="context-item" onclick={() => { if (contextMenu?.session) handleEdit(contextMenu.session); closeContextMenu(); }} type="button">
						{t('session.edit')}
					</button>
					<button class="context-item" onclick={() => { if (contextMenu?.session) handleClone(contextMenu.session); closeContextMenu(); }} type="button">
						{t('session.clone')}
					</button>
				{/if}
				<button class="context-item" onclick={() => { if (contextMenu?.session) void setFavorite(contextMenu.session, !contextMenu.session.favorite); }} type="button">
					{contextMenu.session.favorite ? t('session.remove_favorite') : t('session.add_favorite')}
				</button>
				{#if contextMenu.session.kind !== 'rdp' && contextMenu.session.kind !== 'vnc' && contextMenu.session.kind !== 'spice'}
					<button class="context-item" onclick={() => { if (contextMenu?.session) openHistoryPanel({ sessionId: contextMenu.session.id, sessionName: contextMenu.session.name }); closeContextMenu(); }} type="button">
						{t('history.session_menu')}
					</button>
				{/if}
				{#if hasNetworkAddress(contextMenu.session) && !(selected.has(contextMenu.session.id) && selected.size > 1)}
					{@const netSession = contextMenu.session}
					<div class="context-sep"></div>
					<button class="context-item" onclick={() => { openNetworkTools({ tool: 'ping', sessionId: netSession.id, run: true }); closeContextMenu(); }} type="button">{t('net.menu_ping')}</button>
					<button class="context-item" onclick={() => { openNetworkTools({ tool: 'scan', sessionId: netSession.id, run: true }); closeContextMenu(); }} type="button">{t('net.menu_scan')}</button>
					{#if netSession.wol?.mac}
						<button class="context-item" onclick={() => { openNetworkTools({ tool: 'wake', sessionId: netSession.id, run: true }); closeContextMenu(); }} type="button">{t('net.menu_wake')}</button>
					{/if}
				{/if}
				{#if !(selected.has(contextMenu.session.id) && selected.size > 1)}
					{@const sibs = siblingsOf(contextMenu.session.id)}
					{@const at = sibs.findIndex(s => s.id === contextMenu?.session?.id)}
					{@const hint = canReorder ? undefined : t('session.reorder_hint')}
					<div class="context-sep"></div>
					<button class="context-item" disabled={!canReorder || at <= 0} title={hint} onclick={() => { if (contextMenu?.session) void moveSessionStep(contextMenu.session, -1); }} type="button">{t('session.move_up')}</button>
					<button class="context-item" disabled={!canReorder || at < 0 || at >= sibs.length - 1} title={hint} onclick={() => { if (contextMenu?.session) void moveSessionStep(contextMenu.session, 1); }} type="button">{t('session.move_down')}</button>
				{/if}
				<div class="context-sep"></div>
				<div class="context-label">{contextMenu.session && selected.has(contextMenu.session.id) && selected.size > 1 ? t('session.move_n_to_folder', { count: String(selected.size) }) : t('session.move_to_folder')}</div>
				<!-- Scrollable so a large number of folders doesn't make the menu
				     overflow the screen — caps at ~5 visible rows. -->
				<div class="context-folders">
					{#each moveTargets(targetsOf(contextMenu.session)) as folder (folder.id)}
						<button class="context-item context-folder-item" onclick={() => { if (contextMenu?.session) moveToFolder(contextMenu.session, folder.id); }} type="button">
							<FolderIcon icon={folder.icon} color={folder.color} />
							{folderPath(folder.id, folders)}
						</button>
					{/each}
				</div>
				<button class="context-item context-folder-item" onclick={() => contextNewFolder()} type="button">
					<svg width="12" height="12" viewBox="0 0 24 24" fill="none" class="ctx-folder-icon">
						<path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
					</svg>
					{t('session.new_folder')}
				</button>
				{#if contextMenu.session.folder_id || (selected.has(contextMenu.session.id) && selected.size > 1)}
					<button class="context-item" onclick={() => { if (contextMenu?.session) moveToFolder(contextMenu.session, null); }} type="button">
						{t('session.remove_from_folder')}
					</button>
				{/if}
				<div class="context-sep"></div>
				{#if selected.has(contextMenu.session.id) && selected.size > 1}
					<button class="context-item context-danger" onclick={() => { bulkDeleteConfirm = true; closeContextMenu(); }} type="button">
						{t('session.delete_n', { count: String(selected.size) })}
					</button>
				{:else}
					<button class="context-item context-danger" onclick={() => { if (contextMenu?.session) handleDelete(contextMenu.session); closeContextMenu(); }} type="button">
						{t('common.delete')}
					</button>
				{/if}
			{:else if contextMenu.folder}
				{@const folder = contextMenu.folder}
				{#if folderMenuPage === 'main'}
					{@const inside = sessionsInside(folder)}
					{@const hasSubfolders = folders.some(f => f.parent_id === folder.id)}
					{@const credential = folder.credential_id ? savedCredentials.find(c => c.id === folder.credential_id) : undefined}
					<button class="context-item" disabled={inside.length === 0} onclick={() => openAllIn(folder)} type="button">{t('folder.open_all', { count: String(inside.length) })}</button>
					<button class="context-item" onclick={() => newSessionIn(folder)} type="button">{t('folder.new_session')}</button>
					<button class="context-item" disabled={inside.length === 0 && !sessions.some(s => s.folder_id === folder.id)} onclick={() => void sortFolderByName(folder)} type="button">{t('folder.sort_by_name')}</button>
					<button class="context-item" onclick={() => contextNewFolder(folder.id)} type="button">{t('folder.new_subfolder')}</button>
					<button class="context-item" onclick={() => startRename(folder)} type="button">{t('folder.rename')}<kbd class="context-kbd">F2</kbd></button>
					<button class="context-item context-icon-item" onclick={() => pickFolderIcon(folder)} type="button"><span>{t('folder.icon')}</span><FolderIcon icon={folder.icon} color={folder.color} /></button>
					<div class="context-sep"></div>
					<button class="context-item" disabled={inside.length === 0} onclick={() => selectFolderSessions(folder)} type="button">{t('bulk.select_folder')}</button>
					<button class="context-item" disabled={inside.length === 0} onclick={() => editFolderSessions(folder)} type="button">{t('folder.edit_inside')}</button>
					{#if hasSubfolders}
						<button class="context-item" onclick={() => setFoldersInside(folder, false)} type="button">{t('folder.expand_inside')}</button>
						<button class="context-item" onclick={() => setFoldersInside(folder, true)} type="button">{t('folder.collapse_inside')}</button>
					{/if}
					<div class="context-sep"></div>
					<button class="context-item context-sub" onclick={() => (folderMenuPage = 'move')} type="button">
						<span class="context-sub-label">{t('folder.move')}</span>
						<span class="context-sub-value">{folder.parent_id ? folderPath(folder.parent_id, folders) : t('folder.top_level')}</span>
						<FaIcon icon={faChevronRight} size={9} />
					</button>
					<button class="context-item context-sub" onclick={() => (folderMenuPage = 'login')} type="button">
						<span class="context-sub-label">{t('folder.login')}</span>
						<span class="context-sub-value">{credential ? credential.name : t('credential.folder_none')}</span>
						<FaIcon icon={faChevronRight} size={9} />
					</button>
					<button class="context-item context-sub" onclick={() => (folderMenuPage = 'tmux')} type="button">
						<span class="context-sub-label">tmux</span>
						<span class="context-sub-value">{t(folder.tmux === true ? 'tmux.folder_on' : folder.tmux === false ? 'tmux.folder_off' : 'tmux.folder_inherit')}</span>
						<FaIcon icon={faChevronRight} size={9} />
					</button>
					<div class="context-sep"></div>
					<button class="context-item context-danger" onclick={() => askDeleteFolder(folder)} type="button">{t('folder.delete')}</button>
				{:else}
					<button class="context-item context-back" onclick={() => (folderMenuPage = 'main')} type="button">
						<FaIcon icon={faChevronLeft} size={9} />
						<span>{folderMenuPage === 'move' ? t('session.move_to_folder') : folderMenuPage === 'login' ? t('credential.folder_menu') : t('tmux.folder_menu')}</span>
					</button>
					<div class="context-sep"></div>
					<div class="context-folders context-folders-tall">
						{#if folderMenuPage === 'move'}
							<button class="context-item context-folder-item" class:context-current={!folder.parent_id} disabled={!folder.parent_id} onclick={() => moveFolder(folder, null)} type="button">{t('folder.top_level')}</button>
							{#each folders.filter(f => f.id === folder.parent_id || canMoveFolder(folder.id, f.id, folders)) as f (f.id)}
								<button class="context-item context-folder-item" class:context-current={folder.parent_id === f.id} disabled={folder.parent_id === f.id} onclick={() => moveFolder(folder, f.id)} type="button">{folderPath(f.id, folders)}</button>
							{/each}
						{:else if folderMenuPage === 'login'}
							<button class="context-item context-folder-item" class:context-current={!folder.credential_id} onclick={() => void setFolderCredential(folder, null)} type="button">{t('credential.folder_none')}</button>
							{#each credentialsFor(savedCredentials, folder.vault_id) as c (c.id)}
								<button class="context-item context-folder-item" class:context-current={folder.credential_id === c.id} onclick={() => void setFolderCredential(folder, c.id)} type="button">{c.name} ({c.username})</button>
							{/each}
						{:else}
							{#each [[null, 'tmux.folder_inherit'], [true, 'tmux.folder_on'], [false, 'tmux.folder_off']] as const as [value, label] (label)}
								<button class="context-item context-folder-item" class:context-current={(folder.tmux ?? null) === value} onclick={() => void setFolderTmux(folder, value)} type="button">{t(label)}</button>
							{/each}
						{/if}
					</div>
				{/if}
			{:else}
				<button class="context-item" onclick={() => { closeContextMenu(); handleNewSession(); }} type="button">{t('session.new_session')}</button>
				<button class="context-item" onclick={() => contextNewFolder()} type="button">{t('session.new_folder')}</button>
				<button class="context-item" onclick={() => { closeContextMenu(); openImport(lastImportSource); }} type="button">{t('session.import_sessions_menu')}</button>
				<div class="context-sep"></div>
				<button class="context-item" onclick={() => { closeContextMenu(); selectMode = true; }} type="button">{t('session.select_sessions')}</button>
				<button class="context-item" onclick={() => { closeContextMenu(); selectAllShown(); }} type="button">{t('session.select_all')}</button>
				{#if shownFolders.length}
					<div class="context-sep"></div>
					<button class="context-item" onclick={() => { closeContextMenu(); setAllFolders(true); }} type="button">{t('session.collapse_all')}</button>
					<button class="context-item" onclick={() => { closeContextMenu(); setAllFolders(false); }} type="button">{t('session.expand_all')}</button>
				{/if}
				<div class="context-sep"></div>
				<div class="context-label">{t('session.sort_by')}</div>
				{#each [['default', 'session.sort_default'], ['name', 'session.sort_name'], ['host', 'session.sort_host'], ['recent', 'session.sort_recent']] as const as [key, label] (key)}
					<button class="context-item context-folder-item" class:context-current={sortKey === key} onclick={() => { sortKey = key; closeContextMenu(); }} type="button">{t(label)}</button>
				{/each}
				{#if lastBulkUndo}
					<button class="context-item" onclick={() => void undoBulkEdit()} type="button">{t('bulk.undo_last')}</button>
				{/if}
			{/if}
		</div>
	{/if}
</div>

<FolderIconPicker
	open={iconFolder !== null}
	name={iconFolder?.name ?? ''}
	icon={iconFolder?.icon ?? null}
	color={iconFolder?.color ?? null}
	onclose={() => (iconFolder = null)}
	onsave={(icon, color) => void saveFolderIcon(icon, color)}
/>

<BulkEditor
	open={bulkEditOpen}
	sessions={bulkEditSessions}
	allSessions={sessions}
	credentials={savedCredentials}
	onclose={() => (bulkEditOpen = false)}
	onapplied={(r) => void bulkEdited(r)}
/>

<Modal open={sftpHelp !== undefined} onclose={() => (sftpHelp = undefined)} title={t('session.rdp_sftp_help_title')} maxWidth="560px">
	{#if sftpHelp}
		<div class="sftp-help">
			<p>{t('session.rdp_sftp_help_intro', { host: sftpHelp.host, port: String(sftpHelp.port) })}</p>
			<ul>
				<li>{t('session.rdp_sftp_help_2025')}</li>
				<li>
					{t('session.rdp_sftp_help_2019')}
					<div class="sftp-code">
						<pre>{SFTP_POWERSHELL}</pre>
						<Button variant="secondary" size="sm" onclick={() => void copySftpCommands()}>{sftpCopied ? t('common.copied') : t('common.copy')}</Button>
					</div>
				</li>
				<li>{t('session.rdp_sftp_help_old')}</li>
			</ul>
			<p>{t('session.rdp_sftp_help_after')}</p>
			<p class="sftp-error">{sftpHelp.error}</p>
		</div>
	{/if}
	{#snippet actions()}
		<Button variant="secondary" onclick={() => (sftpHelp = undefined)}>{t('common.close')}</Button>
		<Button variant="primary" onclick={retrySftp}>{t('session.rdp_sftp_help_retry')}</Button>
	{/snippet}
</Modal>

<Modal open={moveIds_ !== null} onclose={() => (moveIds_ = null)} title={t('session.move_n_title', { count: String(moveIds_?.length ?? 0) })} maxWidth="420px">
	<div class="move-body">
		{#if moveMixed}
			<p class="move-empty">{t('session.move_mixed_vaults')}</p>
		{/if}
		{#if (moveIds_ ? moveTargets(moveIds_) : []).length > 6}
			<input class="move-filter" type="search" placeholder={t('session.move_filter')} aria-label={t('session.move_filter')} bind:value={moveFilter} />
		{/if}
		<div class="move-list" role="radiogroup" aria-label={t('session.move_to')}>
			{#each moveFolders as folder (folder.id)}
				<button type="button" role="radio" aria-checked={moveTarget === folder.id} class="move-option" class:chosen={moveTarget === folder.id} onclick={() => (moveTarget = folder.id)} ondblclick={() => { moveTarget = folder.id; void confirmMove(); }}>
					{#if folder.icon || folder.color}<FolderIcon icon={folder.icon} color={folder.color} />{:else}<FaIcon icon={faFolder} size={12} />{/if}
					<span class="move-name">{folderPath(folder.id, folders)}</span>
					<span class="move-count">{sessions.filter(s => s.folder_id === folder.id).length}</span>
				</button>
			{/each}
			{#if moveFolders.length === 0 && moveFilter.trim()}
				<p class="move-empty">{t('session.no_matches')}</p>
			{/if}
		</div>
		<div class="move-extra">
			<button type="button" role="radio" aria-checked={moveTarget === NEW_FOLDER} class="move-option" class:chosen={moveTarget === NEW_FOLDER} onclick={() => (moveTarget = NEW_FOLDER)}>
				<FaIcon icon={faFolderPlus} size={12} />
				<span class="move-name">{t('session.new_folder')}</span>
			</button>
			{#if moveTarget === NEW_FOLDER}
				<!-- svelte-ignore a11y_autofocus -->
				<input class="move-filter" type="text" autofocus placeholder={t('session.folder_name')} aria-label={t('session.folder_name')} bind:value={moveNewName} onkeydown={(e) => e.key === 'Enter' && void confirmMove()} />
			{/if}
			{#if sessions.some(s => moveIds_?.includes(s.id) && s.folder_id)}
				<button type="button" role="radio" aria-checked={moveTarget === null} class="move-option" class:chosen={moveTarget === null} onclick={() => (moveTarget = null)}>
					<span class="move-name">{t('session.remove_from_folder')}</span>
				</button>
			{/if}
		</div>
	</div>
	{#snippet actions()}
		<Button variant="ghost" onclick={() => (moveIds_ = null)}>{t('common.cancel')}</Button>
		<Button variant="primary" disabled={moveTarget === undefined || (moveTarget === NEW_FOLDER && !moveNewName.trim())} onclick={() => void confirmMove()}>
			{t('session.move_n', { count: String(moveIds_?.length ?? 0) })}
		</Button>
	{/snippet}
</Modal>

{#if bulkDeleteConfirm}
	<!-- Deleting several sessions reaches every device the vault syncs to:
	     the names are listed and the count typed, as for other bulk deletes. -->
	<ConfirmAction
		open={bulkDeleteConfirm}
		title={t('session.delete_n', { count: String(selected.size) })}
		message={t('session.delete_n_message', { count: String(selected.size) })}
		items={sessions.filter(s => selected.has(s.id)).map(s => `${s.name} (${s.username ? s.username + '@' : ''}${s.host})`)}
		where={selectedVaultId === ALL_VAULTS ? t('vault.all_vaults') : selectedVaultId ? (vaultState.vaults.get(selectedVaultId)?.name ?? t('vault.private')) : t('vault.private')}
		environment="none"
		confirmLabel={t('session.delete_n', { count: String(selected.size) })}
		typeToConfirm={t('session.delete_n_typed', { count: String(selected.size) })}
		onconfirm={() => void deleteSelected()}
		onclose={() => (bulkDeleteConfirm = false)}
	/>
{/if}

{#if openAllConfirm}
	<ConfirmAction
		open={!!openAllConfirm}
		title={t('folder.open_all_title', { count: String(openAllConfirm.sessions.length) })}
		message={t('folder.open_all_message', { name: openAllConfirm.folder.name })}
		items={openAllConfirm.sessions.map(s => s.name)}
		where={folderPath(openAllConfirm.folder.id, folders)}
		environment="none"
		confirmLabel={t('folder.open_all_confirm', { count: String(openAllConfirm.sessions.length) })}
		typeToConfirm=""
		onconfirm={() => { if (openAllConfirm) openSessions(openAllConfirm.sessions); }}
		onclose={() => (openAllConfirm = null)}
	/>
{/if}

{#if folderDeleteConfirm}
	{@const doomed = folderDeleteConfirm}
	{@const ownSessions = sessions.filter(s => s.folder_id === doomed.id).length}
	{@const ownFolders = folders.filter(f => f.parent_id === doomed.id).length}
	<ConfirmAction
		open={!!folderDeleteConfirm}
		title={t('folder.delete_title', { name: doomed.name })}
		message={t('folder.delete_message')}
		items={[folderPath(doomed.id, folders)]}
		where={doomed.parent_id ? folderPath(doomed.parent_id, folders) : t('folder.top_level')}
		environment="none"
		note={ownSessions || ownFolders ? t('folder.delete_note', { sessions: String(ownSessions), folders: String(ownFolders), parent: doomed.parent_id ? folderPath(doomed.parent_id, folders) : t('folder.top_level') }) : ''}
		confirmLabel={t('common.delete')}
		typeToConfirm=""
		onconfirm={() => { folderDeleteConfirm = null; void handleDeleteFolder(doomed.id); }}
		onclose={() => (folderDeleteConfirm = null)}
	/>
{/if}

<QuickConnect bind:open={showQuickConnect} prefill={quickPrefill} />
<SessionEditor bind:open={showEditor} editSession={editingSession} vaultId={newSessionFolder ? (newSessionFolder.vault_id ?? null) : targetVaultId} folderId={newSessionFolder?.id ?? null} cloneSession={cloningSession} {folders} onsave={handleEditorSave} />
<SshConfigImport bind:open={() => importOpen('ssh_config'), (v) => setImportOpen('ssh_config', v)} vaultId={targetVaultId} onsave={handleEditorSave} onsource={openImport} />
<RdpImport bind:open={() => importOpen('rdp'), (v) => setImportOpen('rdp', v)} vaultId={targetVaultId} onsave={handleEditorSave} onsource={openImport} />
<ManagerImport
	bind:open={() => importOpen('manager'), (v) => setImportOpen('manager', v)}
	source={importSource !== null && importSource !== 'ssh_config' && importSource !== 'rdp' ? importSource : 'rdm'}
	vaultId={targetVaultId}
	onsave={handleEditorSave}
	onsource={openImport}
/>
<UntrustedRefsDialog
	asked={askedRefs}
	busy={approvingRefs}
	error={approveRefsError}
	onallow={() => void allowAskedRefs()}
	oncancel={() => { if (!approvingRefs) finishAsking(false); }}
/>

{#if pickAddressFor}
	{@const pickSession = pickAddressFor}
	<AddressPicker session={pickSession} onpick={(addr) => handleConnect(pickSession, addr)} oncancel={() => (pickAddressFor = undefined)} />
{/if}

{#if showConnectPrompt && connectSession}
	<!-- No click-outside dismissal, as in Modal: a drag that selects text in
	     the password field and ends past the box closed the prompt. Cancel and
	     Escape still close it. -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="prompt-overlay" onkeydown={(e) => { if (e.key === 'Escape') cancelConnect(); }}>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="prompt-box" onkeydown={() => {}}>
			<div class="prompt-header">
				<span class="prompt-title">{t('session.connect_to', { name: connectSession.name })}</span>
				<span class="prompt-detail">{connectSession.username}@{(tryingAddress ?? connectAddress)?.host ?? connectSession.host}:{(tryingAddress ?? connectAddress)?.port ?? connectSession.port}</span>
				{#if tryingAddress}
					<span class="prompt-detail prompt-via">{t('session.trying_address', { name: addressName(tryingAddress, connectSession.port) })}</span>
				{/if}
			</div>

			<form class="prompt-form" onsubmit={(e) => { e.preventDefault(); doConnect(); }}>
				{#if passwordFallback}
					<div class="fallback-hint">{t('session.fallback_to_password')}</div>
					<PasswordField disabled={connecting}>
						{#snippet children(type)}
							<input
								class="prompt-input"
								{type}
								placeholder={t('session.password')}
								bind:value={fallbackPassword}
								bind:this={fallbackInputEl}
								disabled={connecting}
							/>
						{/snippet}
					</PasswordField>
				{:else if !askSecret}
					<!-- Connecting with what the session has: nothing to type. -->
				{:else if connectSession.auth_method.type === 'Password'}
					<PasswordField disabled={connecting}>
						{#snippet children(type)}
							<input
								class="prompt-input"
								{type}
								placeholder={t('session.password')}
								bind:value={connectPassword}
								disabled={connecting}
							/>
						{/snippet}
					</PasswordField>
				{:else if connectSession.auth_method.type === 'Key'}
					<PasswordField disabled={connecting}>
						{#snippet children(type)}
							<input
								class="prompt-input"
								{type}
								placeholder={t('session.key_passphrase_optional')}
								bind:value={connectKeyPassphrase}
								disabled={connecting}
							/>
						{/snippet}
					</PasswordField>
				{/if}

				{#if askSecret && !passwordFallback && promptRef(connectSession)}
					{@const ref = promptRef(connectSession)!}
					<div class="fallback-hint">{t('pwmanager.prompt_uses_ref', { name: `${providerName(ref.provider)} · ${ref.label?.trim() || ref.entry_id}` })}</div>
				{/if}

				{#if askSecret && connectSession.auth_method.type !== 'Agent' && !passwordFallback && !oneOffCredential(connectSession)}
					<label class="remember-label">
						<input type="checkbox" class="remember-check" bind:checked={rememberPassword} disabled={connecting} />
						<span class="remember-text">{hasSavedPassword ? t('session.password_saved') : t('session.remember_password')}</span>
					</label>
				{/if}

				<!-- Where the connection has got to, and on a failure, which step broke. -->
				<ConnectSteps id={connectingId} active={connecting} failed={!connecting && !!connectError} />

				{#if connectError}
					<div class="prompt-error">{connectError}</div>
				{/if}

				<div class="prompt-actions">
					<button type="button" class="prompt-btn prompt-cancel" onclick={cancelConnect}>{t('common.cancel')}</button>
					<button type="submit" class="prompt-btn prompt-connect" disabled={connecting}>
						{#if connecting}{t('session.connecting')}{:else if passwordFallback}{t('session.try_with_password')}{:else}{t('session.connect')}{/if}
					</button>
				</div>
			</form>
		</div>
	</div>
{/if}


<style>
	/* A segmented switch: the chosen side is raised out of a sunken track,
	   so which list is showing reads at a glance in both themes. */
	.list-tabs {
		display: flex;
		gap: 2px;
		padding: 2px;
		background: var(--color-surface-active);
		border: 1px solid var(--color-border);
		border-radius: 6px;
	}

	.list-tab {
		flex: 1;
		padding: 4px 8px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		font-size: var(--text-xs);
		cursor: pointer;
	}

	.list-tab.active {
		background-color: var(--color-bg-elevated);
		color: var(--color-text-primary);
		font-weight: 600;
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2), 0 0 0 1px var(--color-border);
	}

	.context-current {
		font-weight: 600;
		color: var(--color-accent);
	}

	.session-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 4px 0;
		height: 100%;
		overflow: hidden;
	}

	/* Three equal columns forced "Import SSH Config" to wrap onto three lines and
	   pushed all three buttons to 61px tall. Quick Connect is the primary action,
	   so it gets its own full-width row and the two secondary actions share the
	   next one — the sidebar has vertical room to spare. */
	.tool-row {
		display: flex;
		align-items: stretch;
		gap: 4px;
	}

	.tool-row .search-row {
		flex: 1;
		min-width: 0;
	}

	.add-wrap {
		position: relative;
		flex-shrink: 0;
	}

	.add-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 30px;
		height: 100%;
		min-height: 30px;
		padding: 0;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: transparent;
		color: var(--color-accent);
		cursor: pointer;
		transition:
			background-color var(--duration-default) var(--ease-default),
			border-color var(--duration-default) var(--ease-default);
	}

	.add-btn:hover,
	.add-btn.open {
		background-color: rgba(0, 122, 255, 0.1);
		border-color: color-mix(in srgb, var(--color-accent) 55%, var(--color-border));
	}

	.add-btn:focus-visible {
		outline: 2px solid var(--color-accent);
		outline-offset: 1px;
	}

	.add-menu {
		position: fixed;
		z-index: 1000;
		min-width: 200px;
		/* Scrolls within a short window, as a phone on its side is. */
		max-height: calc(100dvh - 120px);
		overflow-y: auto;
		padding: 4px;
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: 8px;
		box-shadow: var(--shadow-elevated, 0 8px 32px rgba(0, 0, 0, 0.35));
	}

	.menu-item {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		padding: 7px 10px;
		border: none;
		border-radius: 5px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		text-align: left;
		font-family: inherit;
		font-size: var(--text-sm);
		white-space: nowrap;
	}

	.menu-item:hover,
	.menu-item:focus-visible {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
		outline: none;
	}

	.menu-item span {
		flex: 1;
	}

	.menu-item kbd,
	.connect-suggestion kbd {
		font-family: var(--font-mono, monospace);
		font-size: var(--text-2xs);
		color: var(--color-text-tertiary);
	}

	.menu-sep {
		height: 1px;
		margin: 4px 6px;
		background: var(--color-border);
	}

	/* "Connect to root@10.0.0.7": offered, never assumed. */
	.connect-suggestion {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 7px 10px;
		border: 1px solid color-mix(in srgb, var(--color-accent) 40%, var(--color-border));
		border-radius: 6px;
		background: rgba(0, 122, 255, 0.08);
		color: var(--color-accent);
		cursor: pointer;
		text-align: left;
		font-family: inherit;
		font-size: var(--text-sm);
	}

	.connect-suggestion:hover {
		background: rgba(0, 122, 255, 0.14);
	}

	.suggestion-text {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.search-row {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 4px 8px;
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: 6px;
	}

	.search-row:focus-within {
		border-color: var(--color-accent);
	}

	.search-icon {
		flex-shrink: 0;
		color: var(--color-text-secondary);
		opacity: 0.5;
	}

	.search-input {
		flex: 1;
		min-width: 0;
		padding: 3px 0;
		border: none;
		background: transparent;
		color: var(--color-text-primary);
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		outline: none;
	}

	.search-input::placeholder {
		color: var(--color-text-secondary);
		opacity: 0.5;
	}

	.search-clear {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		flex-shrink: 0;
	}

	.search-clear:hover {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}


	.new-folder-row {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 6px 8px;
		background: var(--color-surface-hover);
		border-radius: 6px;
		border: 1px solid var(--color-border);
	}

	.new-folder-icon {
		flex-shrink: 0;
		color: var(--color-warning, var(--color-warning));
		opacity: 0.7;
	}

	.new-folder-parent {
		max-width: 35%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--text-xs);
		color: var(--color-text-secondary);
	}

	/* The input gives way, not the row: at its own width it pushed Save and
	   Cancel off a phone's screen. */
	.new-folder-form {
		display: flex;
		align-items: center;
		gap: 4px;
		flex: 1;
		min-width: 0;
	}

	.new-folder-input {
		flex: 1;
		width: 0;
		min-width: 0;
		padding: 5px 10px;
		border: 1px solid var(--color-border-strong);
		border-radius: 5px;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		outline: none;
	}

	.new-folder-input:focus {
		border-color: var(--color-accent);
	}

	.new-folder-save,
	.new-folder-cancel {
		padding: 4px 8px;
		font-family: var(--font-sans);
		font-size: var(--text-xs);
		font-weight: 500;
		border: none;
		border-radius: 4px;
		cursor: pointer;
	}

	.new-folder-save {
		background: var(--color-accent-fill);
		color: #fff;
	}

	.new-folder-save:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.new-folder-cancel {
		background: transparent;
		color: var(--color-text-secondary);
	}

	.folder-node {
		min-width: 0;
		flex-shrink: 0;
	}

	.folder-header {
		display: flex;
		align-items: center;
		gap: 2px;
		padding: 5px 6px;
		border-radius: 6px;
		-webkit-touch-callout: none;
		transition: background-color 0.15s ease, outline 0.15s ease;
	}

	.folder-toggle {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 2px 4px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		cursor: pointer;
		pointer-events: auto;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.folder-toggle:hover {
		background-color: var(--color-surface-hover);
	}

	.folder-chevron {
		transition: transform 0.15s ease;
		transform: rotate(90deg);
	}

	.folder-chevron.collapsed {
		transform: rotate(0deg);
	}

	.folder-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-weight: 500;
		color: var(--color-text-primary);
	}

	.folder-count {
		font-size: var(--text-2xs);
		color: var(--color-text-secondary);
		opacity: 0.6;
	}

	.folder-menu-btn,
	.folder-delete-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 20px;
		height: 20px;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
		opacity: 0.15;
		transition: opacity 0.15s ease, background-color 0.15s ease;
	}

	.folder-header:hover .folder-menu-btn,
	.folder-header:hover .folder-delete-btn,
	.folder-header:focus-within .folder-menu-btn,
	.folder-header:focus-within .folder-delete-btn {
		opacity: 1;
	}

	/* With a mouse, a folder's buttons are not shown until it is pointed at:
	   faint on every row they were noise down the whole list. */
	@media (pointer: fine) {
		.folder-menu-btn,
		.folder-delete-btn {
			opacity: 0;
		}
	}

	.folder-menu-btn:hover {
		background: var(--color-surface-hover);
	}

	.folder-delete-btn:hover {
		background: rgba(255, 69, 58, 0.12);
		color: var(--color-danger);
	}

	/* Where a dragged session will land: a line above or below the row. */
	.drop-before {
		box-shadow: inset 0 2px 0 var(--color-accent);
	}
	.drop-after {
		box-shadow: inset 0 -2px 0 var(--color-accent);
	}

	.folder-session {
		padding-left: 12px;
	}

	/* A session just located from its tab: marked for a moment. */
	.located {
		border-radius: var(--radius-sm, 4px);
		animation: located 1.8s ease-out;
	}
	@keyframes located {
		0%, 50% {
			background-color: var(--color-accent-subtle);
			box-shadow: inset 0 0 0 1px var(--color-accent);
		}
		100% {
			background-color: transparent;
			box-shadow: inset 0 0 0 1px transparent;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.located {
			animation: none;
			box-shadow: inset 0 0 0 1px var(--color-accent);
		}
	}

	.context-menu {
		position: fixed;
		min-width: 180px;
		max-width: 280px;
		padding: 4px 0;
		background-color: var(--color-bg-elevated, var(--color-bg-elevated));
		border: 1px solid var(--color-border);
		border-radius: 8px;
		box-shadow: var(--shadow-elevated);
		z-index: 1000;
	}

	/* Phone: a sheet along the bottom edge. positionMenu still writes left
	   and top inline, so they are overridden here. */
	.context-menu.context-sheet {
		left: 0 !important;
		right: 0;
		top: auto !important;
		bottom: 0;
		min-width: 0;
		max-width: none;
		/* Half the screen at most: the list stays in view, the rest scrolls. */
		max-height: 55dvh;
		overflow-y: auto;
		overscroll-behavior: contain;
		padding: 6px max(4px, var(--safe-right)) max(10px, var(--safe-bottom)) max(4px, var(--safe-left));
		border-width: 1px 0 0;
		border-radius: 14px 14px 0 0;
		animation: sheet-up 0.18s var(--ease-default, ease-out);
	}

	.context-sheet-title {
		position: sticky;
		top: -6px;
		z-index: 1;
		padding: 6px 12px 5px;
		background-color: var(--color-bg-elevated);
		font-size: var(--text-sm, 0.8125rem);
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		border-bottom: 1px solid var(--color-border);
		margin-bottom: 4px;
	}

	.context-sheet .context-item {
		min-height: 38px;
		padding-top: 4px;
		padding-bottom: 4px;
	}

	.context-sheet .context-folders {
		max-height: 152px;
	}

	@keyframes sheet-up {
		from { transform: translateY(100%); }
		to { transform: translateY(0); }
	}

	@media (prefers-reduced-motion: reduce) {
		.context-menu.context-sheet { animation: none; }
	}

	/* Folder list scrolls so a huge number of folders can't blow out the menu
	   height — ~5 rows visible, the rest reachable by scrolling. */
	.context-folders {
		max-height: 150px;
		overflow-y: auto;
	}

	/* A choice opened from the folder menu has the menu to itself. */
	.context-folders-tall {
		max-height: 320px;
	}

	.context-sub {
		gap: 8px;
	}

	.context-sub-label {
		flex-shrink: 0;
	}

	.context-sub-value {
		flex: 1;
		min-width: 0;
		text-align: right;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--color-text-secondary);
		font-size: 0.75rem;
	}

	.context-back {
		font-weight: 600;
	}

	.context-item.context-go,
	.context-item.context-via.context-current {
		font-weight: 600;
		color: var(--color-accent);
	}

	.context-go {
		justify-content: center;
	}

	.context-kbd {
		margin-left: auto;
		font-family: var(--font-mono);
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.context-item:disabled,
	.context-item:disabled:hover {
		opacity: 0.45;
		cursor: default;
		background: transparent;
	}

	.folder-rename {
		flex: 1;
		min-width: 0;
		display: flex;
		padding: 2px 0;
	}

	.folder-rename .new-folder-input {
		flex: 1;
		min-width: 0;
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
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		cursor: pointer;
		text-align: left;
		transition: background-color 0.1s ease;
	}

	.context-item:hover {
		background-color: var(--color-surface-hover);
	}

	.context-folder-item {
		padding-left: 20px;
	}

	.context-danger {
		color: var(--color-danger, var(--color-danger));
	}

	.context-danger:hover {
		background-color: rgba(255, 69, 58, 0.12);
	}

	.context-label {
		padding: 4px 12px 2px;
		font-size: var(--text-xs);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--color-text-secondary);
		opacity: 0.6;
	}

	.context-icon-item {
		justify-content: space-between;
	}

	.ctx-folder-icon {
		color: var(--color-warning, var(--color-warning));
		opacity: 0.7;
		flex-shrink: 0;
	}

	.context-sep {
		height: 1px;
		margin: 4px 0;
		background-color: var(--color-border);
	}


	.no-folder-drop {
		display: flex;
		align-items: center;
		gap: 7px;
		min-height: 38px;
		padding: 7px 12px;
		border: 1px dashed var(--color-border);
		border-radius: 6px;
		color: var(--color-text-secondary);
		font-size: var(--text-sm);
		flex-shrink: 0;
	}

	.ungrouped-drop {
		min-height: 2px;
		flex-shrink: 0;
	}

	.sessions-scroll {
		display: flex;
		flex-direction: column;
		gap: 2px;
		overflow-y: auto;
		/* Nothing here scrolls sideways; a 3px overflow must not grow a bar. */
		overflow-x: hidden;
		flex: 1;
		min-height: 60px;
	}

	/* A short window (a phone on its side): the whole panel scrolls as one.
	   Split between a fixed header and a list that would not go under 60px,
	   the sessions ran off the bottom where nothing could reach them. */
	@media (max-height: 480px) {
		.session-list {
			overflow-y: auto;
		}

		.sessions-scroll {
			flex: none;
			min-height: auto;
			overflow: visible;
		}
	}

	.drop-active {
		outline: 2px dashed rgba(100, 160, 255, 0.5);
		outline-offset: -2px;
		background-color: rgba(100, 160, 255, 0.04);
		border-radius: 4px;
	}

	.loading-state {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		padding: 16px 0;
	}

	.loading-text {
		font-size: var(--text-sm);
		color: var(--color-text-secondary);
	}

	.empty-state-area {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 10px;
		padding: 16px 4px;
		flex: 1;
	}

	.empty-state {
		margin: 0;
		padding: 0;
		font-size: var(--text-sm);
		color: var(--color-text-secondary);
		opacity: 0.7;
		text-align: center;
	}

	.delete-confirm {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 6px 8px;
		background-color: rgba(255, 69, 58, 0.06);
		border: 1px solid rgba(255, 69, 58, 0.15);
		border-radius: var(--radius-card, 8px);
	}

	.delete-confirm-text {
		flex: 1;
		font-size: var(--text-sm);
		color: var(--color-danger);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.delete-confirm-btn,
	.delete-cancel-btn {
		flex-shrink: 0;
		padding: 3px 8px;
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		font-weight: 500;
		border: none;
		border-radius: 4px;
		cursor: pointer;
		transition:
			background-color var(--duration-default) var(--ease-default),
			opacity var(--duration-default) var(--ease-default);
	}

	.delete-confirm-btn {
		background-color: var(--color-danger);
		color: #fff;
	}

	.delete-confirm-btn:hover {
		opacity: 0.85;
	}

	.delete-cancel-btn {
		background-color: var(--color-surface-hover);
		color: var(--color-text-secondary);
	}

	.delete-cancel-btn:hover {
		background-color: var(--color-surface-active);
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

	/* Connect prompt overlay */
	.prompt-overlay {
		position: fixed;
		inset: 0;
		/* Clear of the phone's status and navigation bars (0 on a desktop). */
		padding: var(--safe-top) var(--safe-right) var(--safe-bottom) var(--safe-left);
		z-index: 200;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-surface-sunken);
		backdrop-filter: blur(4px);
	}

	.prompt-box {
		width: 320px;
		background-color: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-card);
		box-shadow: var(--shadow-elevated);
		overflow: hidden;
	}

	.prompt-header {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 14px 16px 10px;
		border-bottom: 1px solid var(--color-border);
	}

	.prompt-title {
		font-size: var(--text-md);
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.prompt-detail {
		font-size: var(--text-sm);
		color: var(--color-text-secondary);
		font-family: var(--font-mono);
	}

	.prompt-via {
		font-family: inherit;
		font-size: var(--text-xs);
	}

	.context-hint {
		margin-left: auto;
		padding-left: 12px;
		font-family: var(--font-mono);
		font-size: var(--text-xs);
		color: var(--color-text-secondary);
	}

	.prompt-form {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 12px 16px 14px;
	}

	.prompt-input {
		width: 100%;
		padding: 8px 10px;
		font-family: var(--font-sans);
		font-size: var(--text-md);
		color: var(--color-text-primary);
		background-color: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: 6px;
		outline: none;
		box-sizing: border-box;
		transition: border-color var(--duration-default) var(--ease-default);
	}

	.prompt-input:focus {
		border-color: var(--color-accent);
	}

	.remember-label {
		display: flex;
		align-items: center;
		gap: 6px;
		cursor: pointer;
	}

	.remember-check {
		width: 14px;
		height: 14px;
		accent-color: var(--color-accent);
		cursor: pointer;
	}

	.remember-check:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.remember-text {
		font-size: var(--text-sm);
		color: var(--color-text-secondary);
	}

	.fallback-hint {
		padding: 6px 10px;
		font-size: var(--text-sm);
		color: var(--color-text-secondary);
		background-color: rgba(10, 132, 255, 0.08);
		border: 1px solid rgba(10, 132, 255, 0.2);
		border-radius: 4px;
		line-height: 1.4;
	}

	.prompt-error {
		padding: 6px 10px;
		font-size: var(--text-sm);
		color: var(--color-danger);
		background-color: rgba(255, 69, 58, 0.08);
		border-radius: 4px;
	}

	.prompt-actions {
		display: flex;
		justify-content: flex-end;
		gap: 6px;
	}

	.prompt-btn {
		padding: 6px 14px;
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		font-weight: 500;
		border: none;
		border-radius: 6px;
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.prompt-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.prompt-cancel {
		background: transparent;
		color: var(--color-text-secondary);
	}

	.prompt-cancel:hover:not(:disabled) {
		background-color: var(--color-surface-hover);
	}

	.prompt-connect {
		background-color: var(--color-accent-fill);
		color: #fff;
	}

	.prompt-connect:hover:not(:disabled) {
		background-color: color-mix(in srgb, var(--color-accent-fill) 88%, black);
	}

	/* Init section (first run) */
	.init-section {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 12px;
		padding: 24px 16px;
		text-align: center;
	}

	.init-icon {
		color: var(--color-text-secondary);
		opacity: 0.6;
	}

	.init-icon.error {
		color: var(--color-danger);
		opacity: 1;
	}

	.import-input {
		width: 100%;
		max-width: 240px;
		padding: 8px 12px;
		font-family: var(--font-mono);
		font-size: var(--text-sm);
		color: var(--color-text-primary);
		background: var(--color-bg-primary);
		border: 1px solid var(--color-border-strong);
		border-radius: 6px;
		outline: none;
	}

	.import-input:focus {
		border-color: var(--color-accent);
	}

	.recovery-buttons {
		display: flex;
		gap: 8px;
	}

	.reset-btn {
		padding: 8px 16px;
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		font-weight: 500;
		color: var(--color-text-secondary);
		background: transparent;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.reset-btn:hover:not(:disabled) {
		background-color: var(--color-surface-hover);
	}

	.reset-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.init-title {
		margin: 0;
		font-size: var(--text-md);
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.init-desc {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--color-text-secondary);
		max-width: 200px;
		line-height: 1.4;
	}

	.init-error {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--color-danger);
	}

	.init-btn {
		padding: 8px 20px;
		font-family: var(--font-sans);
		font-size: var(--text-sm);
		font-weight: 500;
		color: #fff;
		background-color: var(--color-accent-fill);
		border: none;
		border-radius: 6px;
		cursor: pointer;
		transition: background-color var(--duration-default) var(--ease-default);
	}

	.init-btn:hover:not(:disabled) {
		background-color: color-mix(in srgb, var(--color-accent-fill) 88%, black);
	}

	.init-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.search-error {
		margin: 4px 4px 0;
		font-size: 0.6875rem;
		color: var(--color-danger, #ff453a);
		overflow-wrap: anywhere;
	}

	.match-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		margin: 6px 4px 2px;
		font-size: 0.6875rem;
		color: var(--color-text-secondary);
	}

	.match-actions {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.match-select {
		border: none;
		background: none;
		padding: 2px 4px;
		font: inherit;
		color: var(--color-text-tertiary);
		cursor: pointer;
		transition: color var(--duration-default) var(--ease-default);
	}

	.match-select:hover,
	.match-select:focus-visible {
		color: var(--color-accent);
	}

	/* The selection's actions on one row, in place of the toolbar. */
	.bulk-bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 2px 6px;
		margin: 2px 0 4px;
		padding: 3px 4px 3px 8px;
		border-radius: 8px;
		background: var(--color-accent-subtle);
		border: 1px solid color-mix(in srgb, var(--color-accent) 35%, transparent);
	}

	.bulk-count {
		font-size: 0.75rem;
		font-weight: 600;
		white-space: nowrap;
	}

	.bulk-actions {
		display: flex;
		align-items: center;
		gap: 2px;
		margin-left: auto;
	}

	/* The toolbar above the list: small icon buttons, labelled by tooltip. */
	.list-toolbar {
		display: flex;
		align-items: center;
		gap: 2px;
		margin: 2px 0 4px;
		padding-bottom: 4px;
		border-bottom: 1px solid var(--color-border);
	}

	.tb-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 26px;
		padding: 0;
		border: 1px solid transparent;
		border-radius: 6px;
		background: none;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.tb-btn:hover:not(:disabled) {
		background: var(--color-surface-hover);
		border-color: var(--color-border);
		color: var(--color-text-primary);
	}

	.tb-btn.on {
		background: var(--color-accent-subtle);
		color: var(--color-accent);
	}

	.tb-btn.danger {
		color: var(--color-danger);
	}

	.tb-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.tb-sep {
		width: 1px;
		height: 16px;
		margin: 0 3px;
		background: var(--color-border);
	}

	.tb-grow {
		flex: 1;
	}

	.filter-note {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 6px;
		margin: 0 0 4px;
		padding: 0 4px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	.check-item {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.check {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 14px;
		height: 14px;
		color: var(--color-accent);
	}

	.bulk-link {
		border: none;
		background: none;
		padding: 2px 4px;
		font: inherit;
		font-size: 0.6875rem;
		color: var(--color-accent);
		cursor: pointer;
	}

	/* Finger-sized where there is no mouse. */
	@media (pointer: coarse) {
		.tb-btn {
			width: 36px;
			height: 36px;
		}
	}

	.move-body {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.move-filter {
		width: 100%;
		box-sizing: border-box;
		min-height: 32px;
		padding: 5px 9px;
		border-radius: 6px;
		border: 1px solid var(--color-border);
		background: var(--color-bg-secondary);
		color: var(--color-text-primary);
		font: inherit;
		font-size: 0.8125rem;
	}

	.move-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
		max-height: min(320px, 45vh);
		overflow-y: auto;
	}

	.move-extra {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding-top: 6px;
		border-top: 1px solid var(--color-border);
	}

	.move-option {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		min-height: 34px;
		padding: 6px 10px;
		border: 1px solid transparent;
		border-radius: 6px;
		background: none;
		color: var(--color-text-primary);
		font: inherit;
		font-size: 0.8125rem;
		text-align: left;
		cursor: pointer;
	}

	.move-option:hover {
		background: var(--color-surface-hover);
	}

	.move-option.chosen {
		border-color: var(--color-accent);
		background: color-mix(in srgb, var(--color-accent) 14%, transparent);
	}

	.move-name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.move-count {
		font-size: 0.6875rem;
		color: var(--color-text-tertiary);
	}

	.move-empty {
		margin: 4px 2px;
		font-size: 0.75rem;
		color: var(--color-text-secondary);
	}

	@media (pointer: coarse) {
		.move-option { min-height: 44px; }
		.folder-header { min-height: 44px; }
		.folder-menu-btn { width: 36px; height: 36px; opacity: 1; }
		.folder-delete-btn { display: none; }
	}
	.sftp-help {
		display: flex;
		flex-direction: column;
		gap: 10px;
		font-size: 13px;
		line-height: 1.5;
		color: var(--color-text-primary);
	}
	.sftp-help p,
	.sftp-help ul {
		margin: 0;
	}
	.sftp-help ul {
		padding-left: 18px;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.sftp-code {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		margin-top: 6px;
	}
	.sftp-code pre {
		flex: 1;
		min-width: 0;
		margin: 0;
		padding: 8px 10px;
		overflow-x: auto;
		font-family: var(--font-mono, monospace);
		font-size: 12px;
		border-radius: 6px;
		background: var(--color-surface-hover);
		user-select: text;
	}
	.sftp-error {
		font-size: 12px;
		color: var(--color-text-secondary);
		word-break: break-word;
	}
</style>
