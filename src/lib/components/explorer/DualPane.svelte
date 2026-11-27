<script lang="ts">
	/**
	 * The full-pane file manager of an SSH tab: this computer on the left,
	 * the server on the right, worked from the keyboard as in Midnight
	 * Commander (Tab switches sides, F5 copies to the other side, F6 moves,
	 * F7 makes a folder, F8 deletes, Insert marks). It covers the tab's
	 * terminal, which keeps running behind it.
	 *
	 * Narrower than SINGLE_BELOW (a phone, a folded foldable) it shows one
	 * side at a time with a switch between them; wider (a tablet, Samsung
	 * DeX, an unfolded foldable) both, as on a computer. On a phone or
	 * tablet the local side is the app's own folder there, and fingers work
	 * it: a tap opens a folder or a file's menu, a long press opens the
	 * menu, and once a row is marked taps mark more.
	 */
	import { onMount, tick } from 'svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { ask } from '#lib/state/ask.svelte.js';
	import { addToast } from '#lib/state/toasts.svelte.js';
	import { openEditor } from '#lib/state/editor.svelte.js';
	import { getCurrentPath, type FileEntry, type SortKey } from '#lib/state/explorer.svelte.js';
	import { sftpDelete, sftpHome, sftpListDir, sftpMkdir, sftpReadFile, sftpRename, sftpTerminalCwd, sftpTouch } from '#lib/ipc/sftp.js';
	import { sshFilesScp } from '#lib/ipc/ftp.js';
	import { localDelete, localListDir, localMkdir, localReadText, localRename, localReveal, localTouch } from '#lib/ipc/localfs.js';
	import { sshSend } from '#lib/ipc/ssh.js';
	import { shellQuote } from '#lib/utils/shell.js';
	import DbMenu, { type MenuItem } from '#lib/components/db/DbMenu.svelte';
	import { openInEditor } from '#lib/explorer/editor-launch.js';
	import { editExternally } from '#lib/explorer/external-edit.js';
	import { compareFolders } from '#lib/explorer/compare.js';
	import TextPanel from './TextPanel.svelte';
	import { Pane, findByPrefix, remoteJoin, remoteParent, type Row } from '#lib/explorer/pane.svelte.js';
	import { enqueue, getQueue, jobPercent, cancelJob, retryJob, removeJob, clearFinished, listenQueue, type QueueJob } from '#lib/explorer/transfer-queue.svelte.js';
	import { formatModified } from '#lib/explorer/format.js';
	import { formatBytes } from '#lib/utils/formatters.js';
	import { actionLabel } from '#lib/state/shortcuts.svelte.js';
	import { isMobile } from '#lib/platform.js';
	import { longPress } from '#lib/utils/longPress.js';
	import { localRoots } from '#lib/ipc/localfs.js';
	import { canAskStorage, requestStorage, storageGranted } from '#lib/explorer/device-storage.js';

	interface Props {
		connectionId: string;
		/** Shown and focused: it takes the keyboard. */
		active: boolean;
		/** Back to the terminal. */
		onclose?: () => void;
		/** There is a terminal behind it (SSH): off for FTP, which has none. */
		terminal?: boolean;
		/** An SCP session's connection: copies go through scp. */
		scp?: boolean;
	}

	let { connectionId, active, onclose = () => {}, terminal = true, scp = false }: Props = $props();

	const local = new Pane('local', (path) => localListDir(path));
	const remote = new Pane('remote', async (path) => {
		const entries = await sftpListDir(connectionId, path);
		return { path, parent: remoteParent(path), entries };
	});

	/** A phone or tablet: no external editor, no file manager to reveal in. */
	const mobile = isMobile();
	/** Below this width one side shows at a time. */
	const SINGLE_BELOW = 720;
	let width = $state(0);
	let single = $derived(width > 0 && width < SINGLE_BELOW);
	/** The last press was a finger: taps open, and buttons are larger. */
	let touch = $state(isMobile());
	/** Where the local side can start besides its home (Android's shared storage). */
	let roots = $state<string[]>([]);
	/** Android: whether the shared storage can be listed (All files access). */
	let storageOk = $state(storageGranted());

	function localName(): string {
		return mobile ? t('files.this_device') : t('files.this_computer');
	}

	function sideName(kind: 'local' | 'remote'): string {
		return kind === 'local' ? localName() : t('files.server');
	}

	let side = $state<'local' | 'remote'>('remote');
	let pane = $derived(side === 'local' ? local : remote);
	let other = $derived(side === 'local' ? remote : local);

	let rootEl: HTMLDivElement | undefined = $state();
	const listEls: Record<'local' | 'remote', HTMLDivElement | undefined> = $state({ local: undefined, remote: undefined });

	/** A copy, move or delete running: one at a time. */
	let busy = $state(false);
	/** The name asked for by New folder or Rename. */
	let prompt = $state<{ kind: 'mkdir' | 'touch' | 'rename'; pane: Pane; value: string; entry?: FileEntry } | undefined>();
	/** A pane's path being typed in. */
	let editingPath = $state<{ pane: Pane; value: string } | undefined>();

	/** This connection's transfer queue, and whether its list is folded. */
	let queue = $derived(getQueue(connectionId));
	let queueOpen = $state(true);
	let queueLeft = $derived(queue.filter((j) => j.status === 'queued' || j.status === 'running').length);

	onMount(() => {
		void local.load('');
		if (mobile) void localRoots().then((r) => (roots = r.slice(1))).catch(() => {});
		if (!canAskStorage()) return;
		// Back from the system screen that grants All files access.
		const recheck = () => {
			if (document.visibilityState !== 'visible') return;
			const ok = storageGranted();
			if (ok !== storageOk) {
				storageOk = ok;
				void local.refresh();
			}
		};
		document.addEventListener('visibilitychange', recheck);
		window.addEventListener('focus', recheck);
		return () => {
			document.removeEventListener('visibilitychange', recheck);
			window.removeEventListener('focus', recheck);
		};
	});

	// As each queued copy ends, the side it went to (and the side a move
	// came from) shows it.
	$effect(() => {
		return listenQueue(connectionId, {
			onJobEnd: (job: QueueJob) => {
				const to = job.from === 'local' ? remote : local;
				const from = job.from === 'local' ? local : remote;
				if (to.path === job.toDir) void to.refresh();
				if (job.move) void from.refresh();
				if (job.status === 'failed') addToast(t('files.copy_failed', { name: job.entry.name, error: job.error ?? '' }), 'error');
			}
		});
	});

	// The server side starts where the sidebar's explorer is, and opens the
	// same folder again on a new connection after a reconnect.
	let loadedFor: string | undefined;
	$effect(() => {
		const conn = connectionId;
		if (conn === loadedFor) return;
		loadedFor = conn;
		void (async () => {
			if (scp) await sshFilesScp(conn).catch(() => {});
			// Where it was, where the sidebar is, else the login folder.
			// The sidebar's folder; it says `/` when it has none of its own.
			const sidebar = getCurrentPath(conn);
			const start = remote.path || (sidebar !== '/' ? sidebar : '') || (await sftpHome(conn).catch(() => null)) || '/';
			if (conn !== loadedFor) return;
			const ok = await remote.load(start);
			if (!ok && start !== '/') void remote.load('/');
		})();
	});

	$effect(() => {
		if (active) void tick().then(() => rootEl?.focus());
	});

	// Keep each cursor in view.
	$effect(() => {
		for (const [key, p] of [['local', local], ['remote', remote]] as const) {
			const i = p.cursor;
			listEls[key]?.querySelector(`[data-row="${i}"]`)?.scrollIntoView({ block: 'nearest' });
		}
	});

	/** A field that appears takes the keyboard, its text selected. */
	function takeFocus(node: HTMLInputElement): void {
		void tick().then(() => {
			node.focus();
			node.select();
		});
	}

	function errorText(err: unknown): string {
		return err instanceof Error ? err.message : String(err);
	}

	function describe(items: FileEntry[]): string {
		return items.length === 1 ? t('files.one_item', { name: items[0].name }) : t('files.n_items', { count: items.length });
	}

	// ---- Actions ---------------------------------------------------------

	/** F5/F6: the selection joins the transfer queue. */
	async function copy(move = false): Promise<void> {
		const src = pane;
		const dst = other;
		const items = src.selection();
		if (items.length === 0 || !dst.path) return;
		const taken = items.filter((i) => dst.entries.some((e) => e.name === i.name));
		const what = describe(items);
		// One side at a time, the other side's folder is out of sight: say where.
		const to = single ? `${sideName(dst.kind)}: ${dst.path}` : dst.path;
		let question = move ? t('files.move_confirm', { what, to }) : single ? t('files.copy_confirm', { what, to }) : '';
		if (taken.length > 0) {
			question = [question, t('files.overwrite_confirm', { count: taken.length, to: dst.path })].filter(Boolean).join(' ');
		}
		if (question && !(await ask(question, { confirmLabel: move ? t('files.move') : taken.length > 0 ? t('files.overwrite') : t('files.copy') }))) return;

		enqueue(connectionId, items, src.kind, dst.path, move);
		for (const item of items) src.marked.delete(item.path);
		queueOpen = true;
	}

	function removeEntry(p: Pane, entry: FileEntry): Promise<void> {
		return p.kind === 'local' ? localDelete(entry.path) : sftpDelete(connectionId, entry.path);
	}

	async function remove(): Promise<void> {
		if (busy) return;
		const p = pane;
		const items = p.selection();
		if (items.length === 0) return;
		const where = p.kind === 'local' ? localName() : t('files.server');
		if (!(await ask(t('files.delete_confirm', { what: describe(items), where }), { confirmLabel: t('common.delete') }))) return;
		busy = true;
		try {
			for (const item of items) {
				try {
					await removeEntry(p, item);
					p.marked.delete(item.path);
				} catch (err) {
					addToast(t('files.delete_failed', { name: item.name, error: errorText(err) }), 'error');
				}
			}
		} finally {
			busy = false;
			void p.refresh();
		}
	}

	function startMkdir(): void {
		if (pane.path) prompt = { kind: 'mkdir', pane, value: '' };
	}

	function startTouch(): void {
		if (pane.path) prompt = { kind: 'touch', pane, value: '' };
	}

	function startRename(): void {
		const row = pane.current;
		if (!row || row.up) return;
		prompt = { kind: 'rename', pane, value: row.name, entry: row };
	}

	async function commitPrompt(): Promise<void> {
		const pr = prompt;
		if (!pr) return;
		const name = pr.value.trim();
		prompt = undefined;
		void tick().then(() => rootEl?.focus());
		if (!name || (pr.kind === 'rename' && name === pr.entry?.name)) return;
		const p = pr.pane;
		try {
			if (pr.kind === 'mkdir') {
				if (p.kind === 'local') await localMkdir(p.path, name);
				else await sftpMkdir(connectionId, remoteJoin(p.path, name));
			} else if (pr.kind === 'touch') {
				if (p.kind === 'local') await localTouch(p.path, name);
				else await sftpTouch(connectionId, remoteJoin(p.path, name));
			} else if (pr.entry) {
				if (p.kind === 'local') await localRename(pr.entry.path, name);
				else await sftpRename(connectionId, pr.entry.path, remoteJoin(p.path, name));
			}
			await p.load(p.path, name);
		} catch (err) {
			addToast(errorText(err), 'error');
		}
	}

	function cancelPrompt(): void {
		prompt = undefined;
		void tick().then(() => rootEl?.focus());
	}

	/** F4: the file opens in the editor chosen in Settings (the system's
	 *  default program unless another is picked). A
	 *  server file is fetched to a temporary copy, and each save there is
	 *  uploaded back. */
	async function edit(row: Row | undefined = pane.current, p: Pane = pane): Promise<void> {
		if (!row || row.isDirectory) return;
		// No other programs to open it in on a phone: the app's own editor.
		if (mobile) {
			if (p.kind === 'remote') await editInApp(row, p);
			return;
		}
		try {
			if (p.kind === 'local') {
				await openInEditor(row.path);
				return;
			}
			const remotePath = row.path;
			await editExternally(connectionId, remotePath, row.name, row.size, {
				onUploaded: (name) => {
					addToast(t('files.edit_uploaded', { name }), 'success');
					if (remote.path === remoteParent(remotePath)) void remote.refresh();
				},
				onError: (error) => addToast(t('files.edit_upload_failed', { name: row.name, error }), 'error')
			});
			addToast(t('files.edit_opened', { name: row.name }), 'info');
		} catch (err) {
			addToast(t('explorer.open_file_error', { error: errorText(err) }), 'error');
		}
	}

	/** Shift+F4: a server file in Reacher CM's own editor, as the sidebar's
	 *  Edit does. */
	async function editInApp(row: Row | undefined = pane.current, p: Pane = pane): Promise<void> {
		if (!row || row.isDirectory || p.kind !== 'remote') return;
		try {
			const content = await sftpReadFile(connectionId, row.path);
			await openEditor(connectionId, row.path, row.name, content);
		} catch (err) {
			addToast(t('explorer.open_file_error', { error: errorText(err) }), 'error');
		}
	}

	// ---- View and Compare ---------------------------------------------------

	type PanelContent =
		| { kind: 'view'; title: string; text: string }
		| { kind: 'diff'; leftTitle: string; left: string; rightTitle: string; right: string };
	let panel = $state<PanelContent | undefined>();

	function readText(p: Pane, row: FileEntry): Promise<string> {
		return p.kind === 'local' ? localReadText(row.path) : sftpReadFile(connectionId, row.path);
	}

	/** F3: the file under the cursor as text. */
	async function view(row: Row | undefined = pane.current, p: Pane = pane): Promise<void> {
		if (!row || row.isDirectory) return;
		try {
			panel = { kind: 'view', title: row.path, text: await readText(p, row) };
		} catch (err) {
			addToast(t('files.view_failed', { name: row.name, error: errorText(err) }), 'error');
		}
	}

	/** F9: mark the files that differ between the two folders: those the
	 *  other side lacks, and the newer copy of those on both. */
	function compareDirs(): void {
		const diff = compareFolders(local.entries, remote.entries);
		local.marked.clear();
		remote.marked.clear();
		for (const path of diff.left) local.marked.add(path);
		for (const path of diff.right) remote.marked.add(path);
		const differ = diff.left.length + diff.right.length;
		addToast(differ === 0 ? t('files.folders_same', { count: diff.same }) : t('files.folders_differ', { left: diff.left.length, right: diff.right.length }), differ === 0 ? 'success' : 'info');
	}

	type Side = { pane: Pane; entry: FileEntry };

	function fileAt(p: Pane): FileEntry | undefined {
		const row = p.current;
		return row && !row.up && !row.isDirectory ? row : undefined;
	}

	function sameName(p: Pane, name: string): FileEntry | undefined {
		return p.entries.find((e) => !e.isDirectory && e.name === name);
	}

	/** What Compare files works on: two files marked in one pane; else the
	 *  file under the cursor and the file of the same name on the other
	 *  side, or else the file under the other side's cursor. */
	function comparePair(p: Pane = pane): [Side, Side] | undefined {
		const o = p === local ? remote : local;
		const sel = p.selection();
		if (p.marked.size === 2 && sel.length === 2 && sel.every((e) => !e.isDirectory)) {
			return [{ pane: p, entry: sel[0] }, { pane: p, entry: sel[1] }];
		}
		let a = fileAt(p);
		let b = a ? (sameName(o, a.name) ?? fileAt(o)) : fileAt(o);
		if (!a && b) a = sameName(p, b.name);
		if (!a || !b) return undefined;
		const mine: Side = { pane: p, entry: a };
		const theirs: Side = { pane: o, entry: b };
		return p === local ? [mine, theirs] : [theirs, mine];
	}

	/** Shift+F9: two files side by side with their differences. */
	async function compareFiles(pair = comparePair()): Promise<void> {
		if (!pair) {
			addToast(t('files.compare_need_files'), 'info');
			return;
		}
		const [a, b] = pair;
		try {
			const [left, right] = await Promise.all([readText(a.pane, a.entry), readText(b.pane, b.entry)]);
			panel = { kind: 'diff', leftTitle: a.entry.path, left, rightTitle: b.entry.path, right };
		} catch (err) {
			addToast(t('files.view_failed', { name: `${a.entry.name} / ${b.entry.name}`, error: errorText(err) }), 'error');
		}
	}

	function closePanel(): void {
		panel = undefined;
		void tick().then(() => rootEl?.focus());
	}

	async function goHome(): Promise<void> {
		const home = await sftpHome(connectionId).catch(() => null);
		void remote.load(home || '/');
	}

	async function goToTerminalFolder(): Promise<void> {
		try {
			const cwd = await sftpTerminalCwd(connectionId);
			if (cwd) void remote.load(cwd);
			else addToast(t('explorer.terminal_folder_unknown'), 'info');
		} catch (err) {
			addToast(errorText(err), 'error');
		}
	}

	function commitPathEdit(): void {
		const ed = editingPath;
		editingPath = undefined;
		void tick().then(() => rootEl?.focus());
		const value = ed?.value.trim();
		if (ed && value && value !== ed.pane.path) {
			void ed.pane.load(value).then((ok) => {
				if (!ok) addToast(ed.pane.error ?? value, 'error');
			});
		}
	}

	// ---- Keyboard --------------------------------------------------------

	let typed = '';
	let typedAt = 0;

	function pageSize(): number {
		const list = listEls[side];
		const row = list?.querySelector<HTMLElement>('[data-row]');
		if (!list || !row) return 10;
		return Math.max(1, Math.floor(list.clientHeight / row.offsetHeight) - 1);
	}

	// ---- Right-click menu --------------------------------------------------

	let menu = $state<{ x: number; y: number; items: MenuItem[] } | undefined>();

	async function copyText(text: string): Promise<void> {
		try {
			await navigator.clipboard.writeText(text);
			addToast(t('explorer.copied'), 'success');
		} catch {
			addToast(t('toast.copy_failed'), 'error');
		}
	}

	/** Type `cd <dir>` into the tab's terminal. */
	function cdTerminal(dir: string): void {
		const cmd = `cd ${shellQuote(dir)}\n`;
		void sshSend(connectionId, Array.from(new TextEncoder().encode(cmd)));
		addToast(t('explorer.cd_sent', { path: dir }), 'success');
	}

	async function reveal(path: string): Promise<void> {
		try {
			await localReveal(path);
		} catch (err) {
			addToast(errorText(err), 'error');
		}
	}

	/** The menu of a row: for the marked rows when it is one of them, else
	 *  for that row alone. */
	function rowMenu(p: Pane): MenuItem[] {
		const sel = p.selection();
		if (sel.length === 0) return backgroundMenu(p);
		const one = sel.length === 1 ? sel[0] : undefined;
		const file = one && !one.isDirectory ? one : undefined;
		const otherSide = p.kind === 'local' ? t('files.server') : localName();
		const pair = sel.length <= 2 && sel.every((e) => !e.isDirectory) ? comparePair(p) : undefined;
		const items: MenuItem[] = [];
		if (one?.isDirectory) items.push({ label: t('files.open'), hint: 'Enter', action: () => p.enter(one) });
		if (file) {
			items.push({ label: t('files.view'), hint: 'F3', action: () => void view(file, p) });
			if (!mobile) items.push({ label: t('explorer.edit'), hint: 'F4', action: () => void edit(file, p) });
			if (p.kind === 'remote') items.push({ label: t('files.edit_in_app'), hint: '⇧F4', action: () => void editInApp(file, p) });
		}
		// Fingers have no Insert or Ctrl+click: marking is in the menu.
		if (touch && one) {
			const marked = p.marked.has(one.path);
			items.push({ label: marked ? t('files.unmark') : t('files.mark'), action: () => p.toggleMark(one) });
		}
		if (items.length) items.push({ label: '', separator: true });
		items.push(
			{ label: t('files.copy_to', { where: otherSide }), hint: 'F5', action: () => void copy() },
			{ label: t('files.move_to', { where: otherSide }), hint: 'F6', action: () => void copy(true) }
		);
		if (one) items.push({ label: t('explorer.rename'), hint: 'F2', action: startRename });
		items.push({ label: sel.length > 1 ? t('files.delete_n', { count: sel.length }) : t('common.delete'), hint: 'F8', danger: true, disabled: busy, action: () => void remove() });
		if (pair) {
			const with_ = pair[0].entry === file ? pair[1] : pair[0];
			const label = sel.length === 2 ? t('files.compare_files') : with_.pane === p ? t('files.compare_with', { name: with_.entry.name }) : t('files.compare_with', { name: `${with_.pane === local ? localName() : t('files.server')}: ${with_.entry.name}` });
			items.push({ label: '', separator: true });
			items.push({ label, hint: '⇧F9', action: () => void compareFiles(pair) });
		}
		items.push({ label: '', separator: true });
		if (one) {
			items.push({ label: t('explorer.copy_path'), action: () => void copyText(one.path) });
			items.push({ label: t('explorer.copy_filename'), action: () => void copyText(one.name) });
		} else {
			items.push({ label: t('explorer.copy_path'), action: () => void copyText(sel.map((e) => e.path).join('\n')) });
		}
		if (one?.isDirectory && p.kind === 'remote' && terminal) items.push({ label: t('explorer.cd_here'), action: () => cdTerminal(one.path) });
		if (one && p.kind === 'local' && !mobile) items.push({ label: t('files.reveal'), action: () => void reveal(one.path) });
		return items;
	}

	/** The menu of a pane's empty space: the folder itself. */
	function backgroundMenu(p: Pane): MenuItem[] {
		const items: MenuItem[] = [
			{ label: t('explorer.new_folder'), hint: 'F7', disabled: !p.path, action: startMkdir },
			{ label: t('explorer.new_file'), disabled: !p.path, action: startTouch },
			{ label: '', separator: true },
			{ label: t('explorer.refresh'), hint: actionHint('R'), action: () => void p.refresh() },
			{ label: t('files.select_all'), hint: actionHint('A'), disabled: p.entries.length === 0, action: () => p.markAll() },
			{ label: t('files.compare_folders'), hint: 'F9', action: compareDirs },
			{ label: t('files.compare_files'), hint: '⇧F9', action: () => void compareFiles() },
			{ label: '', separator: true },
			{ label: t('files.copy_folder_path'), disabled: !p.path, action: () => void copyText(p.path) }
		];
		if (p.kind === 'remote' && !terminal) {
			items.push({ label: t('files.home'), action: () => void goHome() });
		} else if (p.kind === 'remote') {
			items.push({ label: t('explorer.go_terminal_folder'), action: () => void goToTerminalFolder() });
			if (p.path) items.push({ label: t('files.cd_terminal_here'), action: () => cdTerminal(p.path) });
		} else {
			items.push({ label: t('files.home'), action: () => void p.load('') });
			for (const root of roots) items.push({ label: root, action: () => void p.load(root) });
			if (p.path && !mobile) items.push({ label: t('files.reveal'), action: () => void reveal(p.path) });
		}
		if (p.marked.size > 0) items.push({ label: t('files.unmark_all'), action: () => p.marked.clear() });
		return items;
	}

	function actionHint(letter: string): string {
		return /Mac|iPhone|iPad/.test(navigator.platform) ? `⌘${letter}` : `Ctrl+${letter}`;
	}

	/** Right-click on a row: it joins the marked rows' menu when marked,
	 *  else it alone is what the menu acts on. */
	function rowContextMenu(e: MouseEvent, p: Pane, i: number): void {
		e.preventDefault();
		e.stopPropagation();
		side = p.kind;
		const row = p.rows[i];
		if (!row.up && !p.marked.has(row.path)) p.marked.clear();
		p.cursor = i;
		rootEl?.focus();
		menu = { x: e.clientX, y: e.clientY, items: row.up ? backgroundMenu(p) : rowMenu(p) };
	}

	function listContextMenu(e: MouseEvent, p: Pane): void {
		e.preventDefault();
		side = p.kind;
		rootEl?.focus();
		menu = { x: e.clientX, y: e.clientY, items: backgroundMenu(p) };
	}

	/** The menu key or Shift+F10: the cursor row's menu, beside the row. */
	function keyboardMenu(): void {
		const p = pane;
		const el = listEls[side]?.querySelector<HTMLElement>(`[data-row="${p.cursor}"]`);
		const r = el?.getBoundingClientRect() ?? listEls[side]?.getBoundingClientRect();
		if (!r) return;
		const row = p.current;
		menu = { x: r.left + 40, y: r.top + (el ? r.height : 20), items: !row || row.up ? backgroundMenu(p) : rowMenu(p) };
	}

	function closeMenu(): void {
		menu = undefined;
		void tick().then(() => rootEl?.focus());
	}

	function onKeydown(e: KeyboardEvent): void {
		if (prompt || editingPath || panel || menu) return;
		if (e.target instanceof HTMLInputElement) return;
		const mod = e.ctrlKey || e.metaKey;
		const p = pane;
		let handled = true;
		switch (e.key) {
			case 'ArrowDown':
				if (e.shiftKey) p.toggleMark();
				p.move(p.cursor + 1);
				break;
			case 'ArrowUp':
				if (e.shiftKey) p.toggleMark();
				p.move(p.cursor - 1);
				break;
			case 'PageDown':
				p.move(p.cursor + pageSize());
				break;
			case 'PageUp':
				p.move(p.cursor - pageSize());
				break;
			case 'Home':
				p.move(0);
				break;
			case 'End':
				p.move(p.rows.length - 1);
				break;
			case 'Tab':
				side = side === 'local' ? 'remote' : 'local';
				break;
			case 'Enter':
				if (!p.enter()) void view();
				break;
			case 'Backspace':
				p.up();
				break;
			case 'Insert':
				p.toggleMark();
				p.move(p.cursor + 1);
				break;
			case 'F2':
				startRename();
				break;
			case 'F3':
				void view();
				break;
			case 'F4':
				if (e.shiftKey) void editInApp();
				else void edit();
				break;
			case 'F9':
				if (e.shiftKey) void compareFiles();
				else compareDirs();
				break;
			case 'F5':
				void copy();
				break;
			case 'F6':
				if (e.shiftKey) startRename();
				else void copy(true);
				break;
			case 'F7':
				startMkdir();
				break;
			case 'F8':
			case 'Delete':
				void remove();
				break;
			case 'F10':
				if (e.shiftKey) keyboardMenu();
				else if (terminal) onclose();
				break;
			case 'ContextMenu':
				keyboardMenu();
				break;
			case 'Escape':
				if (p.marked.size > 0) p.marked.clear();
				else handled = false;
				break;
			default:
				handled = false;
		}
		if (!handled && mod && !e.altKey && e.key.toLowerCase() === 'a') {
			p.markAll();
			handled = true;
		} else if (!handled && mod && !e.altKey && e.code === 'KeyR') {
			void p.refresh();
			handled = true;
		} else if (!handled && e.key === ' ' && !mod) {
			p.toggleMark();
			p.move(p.cursor + 1);
			handled = true;
		} else if (!handled && e.key.length === 1 && !mod && !e.altKey) {
			// Type the start of a name to go to it.
			const now = Date.now();
			typed = now - typedAt > 900 ? e.key : typed + e.key;
			typedAt = now;
			const at = findByPrefix(p.rows, typed);
			if (at >= 0) p.move(at);
			handled = true;
		}
		if (handled) {
			e.preventDefault();
			e.stopPropagation();
		}
	}

	// ---- Mouse -----------------------------------------------------------

	let anchor = 0;

	function clickRow(e: MouseEvent, p: Pane, i: number): void {
		side = p.kind;
		const row = p.rows[i];
		if (touch) {
			tapRow(e, p, i);
			return;
		}
		if (e.ctrlKey || e.metaKey) {
			p.toggleMark(row);
			anchor = i;
		} else if (e.shiftKey) {
			const [a, b] = anchor < i ? [anchor, i] : [i, anchor];
			for (const r of p.rows.slice(a, b + 1)) if (!r.up) p.marked.add(r.path);
		} else {
			anchor = i;
		}
		p.cursor = i;
		rootEl?.focus();
	}

	/** A finger's tap: while rows are marked it marks more (or unmarks);
	 *  else a folder opens and a file's menu shows. */
	function tapRow(e: MouseEvent, p: Pane, i: number): void {
		const row = p.rows[i];
		p.cursor = i;
		if (p.marked.size > 0 && !row.up) {
			p.toggleMark(row);
			return;
		}
		if (row.isDirectory) {
			p.enter(row);
			return;
		}
		menu = { x: e.clientX, y: e.clientY, items: rowMenu(p) };
	}

	function onPointerDown(e: PointerEvent): void {
		touch = e.pointerType === 'touch' || (mobile && e.pointerType !== 'mouse');
	}

	function openRow(p: Pane, i: number): void {
		const row = p.rows[i];
		if (!p.enter(row)) void view(row, p);
	}

	function sizeText(row: Row): string {
		if (row.up) return t('files.up');
		if (row.isDirectory) return t('files.folder');
		if (row.permissions?.startsWith('l')) return '→';
		return formatBytes(row.size);
	}

	function statusText(p: Pane): string {
		if (p.marked.size > 0) {
			const sel = p.selection();
			const bytes = sel.reduce((n, e) => n + (e.isDirectory ? 0 : e.size), 0);
			return t('files.marked', { count: sel.length, size: formatBytes(bytes) });
		}
		const row = p.current;
		if (!row || row.up) return t('files.n_items', { count: p.entries.length });
		return [row.name, row.isDirectory ? '' : formatBytes(row.size), row.permissions ?? ''].filter(Boolean).join('  ·  ');
	}

	const SORTS: { key: SortKey; label: string }[] = [
		{ key: 'name', label: 'explorer.col_name' },
		{ key: 'size', label: 'explorer.col_size' },
		{ key: 'modified', label: 'explorer.col_modified' }
	];

	const ALL_KEYS: { key: string; label: string; run: () => void; remoteOnly?: boolean }[] = [
		{ key: 'F2', label: 'explorer.rename', run: startRename },
		{ key: 'F3', label: 'files.view', run: () => void view() },
		// On a phone only a server file can be edited, in the app's editor.
		{ key: 'F4', label: 'explorer.edit', run: () => void edit(), remoteOnly: mobile },
		{ key: 'F5', label: 'files.copy', run: () => void copy() },
		{ key: 'F6', label: 'files.move', run: () => void copy(true) },
		{ key: 'F7', label: 'explorer.new_folder', run: startMkdir },
		{ key: 'F8', label: 'common.delete', run: () => void remove() },
		{ key: 'F9', label: 'files.compare', run: compareDirs },
		{ key: '⇧F9', label: 'files.compare_files', run: () => void compareFiles() },
		{ key: 'F10', label: 'files.terminal', run: () => onclose() }
	];
	/** No way back to a terminal where there is none. */
	let KEYS = $derived(terminal ? ALL_KEYS : ALL_KEYS.filter((k) => k.key !== 'F10'));

	/** A key's label: Copy and Move name the other side when it is out of sight. */
	function keyLabel(k: { key: string; label: string }): string {
		if (single && k.key === 'F5') return t('files.copy_to', { where: sideName(other.kind) });
		if (single && k.key === 'F6') return t('files.move_to', { where: sideName(other.kind) });
		return t(k.label);
	}
</script>

{#snippet icon(d: string, size = 14)}
	<svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
		<path {d} stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
	</svg>
{/snippet}

{#snippet paneView(p: Pane)}
	{@const focused = side === p.kind}
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_no_noninteractive_element_interactions -->
	<section class="pane" class:focused={focused && !single} aria-label={sideName(p.kind)} onclick={() => { side = p.kind; if (!editingPath) rootEl?.focus(); }}>
		<header class="pane-head">
			{#if !single}<span class="side-label">{sideName(p.kind)}</span>{/if}
			<button class="ib" type="button" onclick={() => p.up()} disabled={p.parent === null} title={`${t('explorer.go_parent')} (Backspace)`} aria-label={t('explorer.go_parent')}>{@render icon('M15 18l-6-6 6-6')}</button>
			{#if p.kind === 'local' || !terminal}
				<button class="ib" type="button" onclick={() => void (p.kind === 'local' ? p.load('') : goHome())} title={t('files.home')} aria-label={t('files.home')}>{@render icon('M3 11l9-7 9 7M5 10v10h5v-6h4v6h5V10')}</button>
			{:else}
				<button class="ib" type="button" onclick={() => void goToTerminalFolder()} title={t('explorer.go_terminal_folder')} aria-label={t('explorer.go_terminal_folder')}>{@render icon('M4 17l6-5-6-5M12 19h8')}</button>
			{/if}
			<button class="ib" type="button" onclick={() => void p.refresh()} title={`${t('explorer.refresh_dir')} (Ctrl+R)`} aria-label={t('explorer.refresh_dir')}>{@render icon('M21 12a9 9 0 1 1-2.6-6.4M21 4v5h-5')}</button>
			{#if editingPath?.pane === p}
				<input
					class="path-input"
					type="text"
					bind:value={editingPath.value}
					use:takeFocus
					spellcheck="false"
					aria-label={t('files.path')}
					onkeydown={(e) => {
						if (e.key === 'Enter') { e.preventDefault(); commitPathEdit(); }
						else if (e.key === 'Escape') { e.preventDefault(); editingPath = undefined; rootEl?.focus(); }
					}}
					onblur={() => (editingPath = undefined)}
				/>
			{:else}
				<button class="path" type="button" title={t('files.edit_path')} onclick={() => (editingPath = { pane: p, value: p.path })}><span class="path-text">{p.path || '…'}</span></button>
			{/if}
		</header>
		<div class="cols" role="row">
			{#each SORTS as s (s.key)}
				<button class="col col-{s.key}" type="button" onclick={() => p.sortBy(s.key)} aria-pressed={p.sort === s.key}>
					{t(s.label)}{#if p.sort === s.key}<span class="arrow">{p.desc ? '▾' : '▴'}</span>{/if}
				</button>
			{/each}
		</div>
		<div class="list" bind:this={listEls[p.kind]} role="listbox" tabindex="-1" aria-multiselectable="true" aria-label={p.path} oncontextmenu={(e) => listContextMenu(e, p)} use:longPress={mobile}>
			{#if p.kind === 'local' && !storageOk}
				<div class="storage-note">
					<span>{t('files.storage_access_hint')}</span>
					<button class="btn small" type="button" onclick={(e) => { e.stopPropagation(); requestStorage(); }}>{t('files.storage_access_allow')}</button>
				</div>
			{/if}
			{#if p.error}
				<div class="note error">{p.error}</div>
			{:else if p.rows.length === 0 && !p.loading}
				<div class="note">{t('explorer.empty')}</div>
			{/if}
			{#each p.rows as row, i (row.up ? '..' : row.path)}
				<!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
				<div
					class="row"
					class:cursor={i === p.cursor}
					class:marked={p.marked.has(row.path) && !row.up}
					class:dir={row.isDirectory}
					data-row={i}
					role="option"
					aria-selected={p.marked.has(row.path) && !row.up}
					onclick={(e) => { e.stopPropagation(); clickRow(e, p, i); }}
					ondblclick={() => openRow(p, i)}
					oncontextmenu={(e) => rowContextMenu(e, p, i)}
				>
					<span class="name">
						{#if row.isDirectory}
							<svg class="kind" width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" /></svg>
						{:else}
							<svg class="kind" width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" /></svg>
						{/if}
						<span class="text">{row.name}</span>
					</span>
					<span class="size">{sizeText(row)}</span>
					<span class="modified">{row.up ? '' : formatModified(row.modified)}</span>
				</div>
			{/each}
		</div>
		<footer class="pane-status">
			{#if p.loading}<span class="spinner" aria-hidden="true"></span>{/if}
			<span class="status-text">{statusText(p)}</span>
		</footer>
	</section>
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div class="dual" class:touch class:single bind:this={rootEl} bind:clientWidth={width} tabindex="0" role="application" aria-label={t('files.title')} onkeydown={onKeydown} onpointerdowncapture={onPointerDown}>
	{#if single}
		<div class="sides" role="tablist" aria-label={t('files.title')}>
			{#each [local, remote] as p (p.kind)}
				<button class="side-tab" class:on={side === p.kind} type="button" role="tab" aria-selected={side === p.kind} onclick={() => (side = p.kind)}>
					<span>{sideName(p.kind)}</span>
					{#if p.marked.size > 0}<span class="side-count">{p.marked.size}</span>{/if}
				</button>
			{/each}
		</div>
	{/if}
	<div class="panes">
		{#if !single || side === 'local'}{@render paneView(local)}{/if}
		{#if !single}
		<div class="mid" aria-hidden="true">
			<button class="arrow-btn" type="button" tabindex="-1" title={`${t('files.copy')} (F5)`} onclick={() => { side = 'local'; void copy(); }}>{@render icon('M5 12h14M13 6l6 6-6 6', 16)}</button>
			<button class="arrow-btn" type="button" tabindex="-1" title={`${t('files.copy')} (F5)`} onclick={() => { side = 'remote'; void copy(); }}>{@render icon('M19 12H5M11 6l-6 6 6 6', 16)}</button>
			<span class="mid-gap"></span>
			<button class="arrow-btn" type="button" tabindex="-1" title={`${t('files.compare_files')} (⇧F9)`} onclick={() => void compareFiles()}>{@render icon('M8 4v16M16 4v16M4 8h4M4 12h4M16 12h4M16 16h4', 16)}</button>
			<button class="arrow-btn" type="button" tabindex="-1" title={`${t('files.compare_folders')} (F9)`} onclick={compareDirs}>{@render icon('M3 6h6l2 2h10v11H3zM12 11v5M9.5 13.5h5', 16)}</button>
		</div>
		{/if}
		{#if !single || side === 'remote'}{@render paneView(remote)}{/if}
	</div>

	{#if queue.length > 0}
		<div class="queue" class:folded={!queueOpen}>
			<div class="queue-head">
				<button class="queue-toggle" type="button" tabindex="-1" onclick={() => (queueOpen = !queueOpen)} aria-expanded={queueOpen}>
					<span class="chev" class:open={queueOpen}>{@render icon('M9 6l6 6-6 6', 12)}</span>
					{t('files.queue')}
					<span class="queue-count">{queueLeft > 0 ? t('files.queue_left', { count: queueLeft }) : t('files.queue_finished')}</span>
				</button>
				{#if queue.length > queueLeft}
					<button class="queue-clear" type="button" tabindex="-1" onclick={() => clearFinished(connectionId)}>{t('files.queue_clear')}</button>
				{/if}
			</div>
			{#if queueOpen}
				<div class="queue-list">
					{#each queue as job (job.id)}
						{@const cancelLabel = t(job.status === 'running' && job.from === 'local' ? 'files.stop_upload' : 'common.cancel')}
						<div class="job {job.status}">
							<span class="tr-dir" title={job.from === 'local' ? t('files.upload') : t('files.download')}>{job.from === 'local' ? '↑' : '↓'}</span>
							<span class="tr-name" title={`${job.entry.path} → ${job.toDir}`}>{job.entry.name}{#if job.move}<span class="job-move"> · {t('files.move')}</span>{/if}</span>
							{#if job.status === 'running'}
								<span class="tr-bar"><span style:width="{Math.min(100, jobPercent(job))}%"></span></span>
							{:else}
								<span class="job-status" title={job.error ?? ''}>{job.status === 'failed' ? (job.error ?? t('files.job_failed')) : t(`files.job_${job.status}`)}</span>
							{/if}
							<span class="job-actions">
								{#if job.status === 'queued' || job.status === 'running'}
									<button class="ib small" type="button" tabindex="-1" disabled={job.status === 'running' && !job.transferId} onclick={() => cancelJob(job.id)} title={cancelLabel} aria-label={cancelLabel}>{@render icon('M6 6l12 12M18 6L6 18', 12)}</button>
								{:else}
									{#if job.status !== 'done'}
										<button class="ib small" type="button" tabindex="-1" onclick={() => retryJob(job.id)} title={t('files.retry')} aria-label={t('files.retry')}>{@render icon('M4 12a8 8 0 1 0 2.3-5.7M4 4v5h5', 12)}</button>
									{/if}
									<button class="ib small" type="button" tabindex="-1" onclick={() => removeJob(job.id)} title={t('files.queue_remove')} aria-label={t('files.queue_remove')}>{@render icon('M5 12h14', 12)}</button>
								{/if}
							</span>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	{/if}

	{#if menu}
		<DbMenu x={menu.x} y={menu.y} items={menu.items} onclose={closeMenu} />
	{/if}

	{#if panel}
		<TextPanel content={panel} onclose={closePanel} />
	{/if}

	{#if prompt}
		<form class="prompt" onsubmit={(e) => { e.preventDefault(); void commitPrompt(); }}>
			<label for="dual-prompt">{prompt.kind === 'mkdir' ? t('explorer.folder_name') : prompt.kind === 'touch' ? t('explorer.file_name') : t('files.new_name')}</label>
			<input
				id="dual-prompt"
				type="text"
				bind:value={prompt.value}
				use:takeFocus
				spellcheck="false"
				onkeydown={(e) => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); cancelPrompt(); } }}
			/>
			<button class="btn primary" type="submit">{t('common.ok')}</button>
			<button class="btn" type="button" onclick={cancelPrompt}>{t('common.cancel')}</button>
		</form>
	{/if}

	<div class="keys" role="toolbar" aria-label={t('files.actions')}>
		{#each KEYS as k (k.key)}
			{#if !k.remoteOnly || side === 'remote'}
				<button class="key" type="button" disabled={busy && k.key !== 'F10'} onclick={() => { k.run(); }}>
					{#if !touch}<kbd>{k.key}</kbd>{/if}<span>{keyLabel(k)}</span>
				</button>
			{/if}
		{/each}
		{#if touch && pane.marked.size === 0}
			<button class="key" type="button" disabled={pane.entries.length === 0} onclick={() => pane.markAll()}><span>{t('files.select_all')}</span></button>
		{:else if touch}
			<button class="key" type="button" onclick={() => pane.marked.clear()}><span>{t('files.unmark_all')}</span></button>
		{/if}
		{#if !touch}<span class="hint">{terminal ? t('files.keys_hint', { key: actionLabel('files.toggle') }) : t('files.keys_hint_files')}</span>{/if}
	</div>
</div>

<style>
	.dual {
		position: absolute;
		inset: 0;
		z-index: 5;
		display: flex;
		flex-direction: column;
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-size: 0.8125rem;
		outline: none;
	}

	.panes {
		flex: 1 1 auto;
		display: flex;
		min-height: 0;
		gap: 0;
		padding: 8px 8px 0;
	}

	.pane {
		flex: 1 1 0;
		min-width: 0;
		display: flex;
		flex-direction: column;
		border: 1px solid var(--color-border);
		border-radius: 8px;
		background: var(--color-bg-secondary);
		overflow: hidden;
	}

	.pane.focused {
		border-color: var(--color-accent);
		box-shadow: 0 0 0 1px var(--color-accent-subtle);
	}

	.mid {
		flex: 0 0 34px;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
	}

	.arrow-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 26px;
		height: 26px;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: var(--color-bg-secondary);
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.arrow-btn:hover:not(:disabled) {
		color: var(--color-accent);
		border-color: var(--color-accent);
	}

	.arrow-btn:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.pane-head {
		display: flex;
		align-items: center;
		gap: 2px;
		padding: 4px 6px;
		border-bottom: 1px solid var(--color-border);
		min-width: 0;
	}

	.side-label {
		font-size: 0.6875rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--color-text-tertiary);
		padding: 0 6px 0 2px;
		white-space: nowrap;
	}

	.pane.focused .side-label {
		color: var(--color-accent);
	}

	.ib {
		flex: 0 0 auto;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		border: none;
		border-radius: 5px;
		background: transparent;
		color: var(--color-text-secondary);
		cursor: pointer;
	}

	.ib.small {
		width: 20px;
		height: 20px;
	}

	.ib:hover:not(:disabled) {
		background: var(--color-surface-hover);
		color: var(--color-text-primary);
	}

	.ib:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.path,
	.path-input {
		flex: 1 1 auto;
		min-width: 0;
		height: 24px;
		margin-left: 4px;
		padding: 0 8px;
		border: 1px solid transparent;
		border-radius: 5px;
		background: var(--color-surface-sunken);
		color: var(--color-text-primary);
		font-family: var(--font-mono, monospace);
		font-size: 0.75rem;
		text-align: left;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		direction: rtl;
		cursor: text;
	}

	/* Cut from the left, so the end of a long path stays in sight; the
	   text itself stays left to right, or "/home/user" shows as "home/user/". */
	.path-text {
		direction: ltr;
		unicode-bidi: isolate;
	}

	.path-input {
		direction: ltr;
		border-color: var(--color-accent);
		outline: none;
	}

	.cols,
	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 84px 124px;
		column-gap: 10px;
		padding: 0 10px;
	}

	.cols {
		border-bottom: 1px solid var(--color-border);
	}

	.col {
		height: 22px;
		border: none;
		background: transparent;
		color: var(--color-text-tertiary);
		font-size: 0.6875rem;
		text-align: left;
		cursor: pointer;
		padding: 0;
	}

	.col-size {
		text-align: right;
	}

	.col:hover {
		color: var(--color-text-primary);
	}

	.arrow {
		margin-left: 3px;
	}

	.list {
		flex: 1 1 auto;
		overflow-y: auto;
		overflow-x: hidden;
		min-height: 0;
		padding: 2px 0;
	}

	.row {
		align-items: center;
		height: 24px;
		cursor: default;
		user-select: none;
		color: var(--color-text-primary);
	}

	.row:hover {
		background: var(--color-surface-hover);
	}

	.row.marked {
		color: var(--color-warning);
		font-weight: 600;
	}

	.row.cursor {
		background: var(--color-surface-active);
	}

	.pane.focused .row.cursor {
		background: var(--color-accent-subtle);
		box-shadow: inset 2px 0 0 var(--color-accent);
	}

	.name {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
	}

	.kind {
		flex: 0 0 auto;
		color: var(--color-text-tertiary);
	}

	.row.dir .kind {
		color: var(--color-accent);
	}

	.text {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.size,
	.modified {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		white-space: nowrap;
		overflow: hidden;
		font-variant-numeric: tabular-nums;
	}

	.size {
		text-align: right;
	}

	.row.marked .size,
	.row.marked .modified {
		color: inherit;
	}

	.note {
		padding: 12px;
		color: var(--color-text-tertiary);
	}

	.note.error {
		color: var(--color-danger);
		white-space: pre-wrap;
		word-break: break-word;
	}

	.pane-status {
		display: flex;
		align-items: center;
		gap: 6px;
		height: 24px;
		padding: 0 10px;
		border-top: 1px solid var(--color-border);
		font-size: 0.71875rem;
		color: var(--color-text-secondary);
		min-width: 0;
	}

	.status-text {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.spinner {
		flex: 0 0 auto;
		width: 10px;
		height: 10px;
		border: 2px solid var(--color-border-strong);
		border-top-color: var(--color-accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	.queue {
		margin: 6px 8px 0;
		border: 1px solid var(--color-border);
		border-radius: 8px;
		background: var(--color-bg-secondary);
		font-size: 0.75rem;
	}

	.queue-head {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 2px 8px 2px 4px;
	}

	.queue-toggle {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1 1 auto;
		min-width: 0;
		padding: 3px 4px;
		border: none;
		background: transparent;
		color: var(--color-text-primary);
		font: inherit;
		font-weight: 600;
		cursor: pointer;
		text-align: left;
	}

	.chev {
		display: flex;
		color: var(--color-text-tertiary);
		transition: transform 0.12s ease;
	}

	.chev.open {
		transform: rotate(90deg);
	}

	.queue-count {
		font-weight: 400;
		color: var(--color-text-secondary);
	}

	.queue-clear {
		border: none;
		background: transparent;
		color: var(--color-accent);
		font: inherit;
		cursor: pointer;
		padding: 2px 4px;
		white-space: nowrap;
	}

	.queue-clear:hover {
		text-decoration: underline;
	}

	.queue-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
		max-height: 132px;
		overflow-y: auto;
		padding: 2px 8px 6px;
		border-top: 1px solid var(--color-border);
	}

	.job {
		display: grid;
		grid-template-columns: 14px minmax(0, 1fr) minmax(80px, 160px) 44px;
		align-items: center;
		gap: 8px;
		min-height: 22px;
	}

	.tr-dir {
		color: var(--color-text-secondary);
		text-align: center;
	}

	.tr-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.job-move {
		color: var(--color-text-tertiary);
	}

	.job.done .tr-name,
	.job.cancelled .tr-name {
		color: var(--color-text-secondary);
	}

	.job-status {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--color-text-secondary);
	}

	.job.done .job-status {
		color: var(--color-success);
	}

	.job.failed .job-status {
		color: var(--color-danger);
	}

	.job-actions {
		display: flex;
		justify-content: flex-end;
		gap: 2px;
	}

	.tr-bar {
		height: 4px;
		border-radius: 2px;
		background: var(--color-surface-strong);
		overflow: hidden;
	}

	.tr-bar span {
		display: block;
		height: 100%;
		background: var(--color-accent);
		transition: width 0.2s linear;
	}

	.mid-gap {
		height: 6px;
	}

	.prompt {
		display: flex;
		align-items: center;
		gap: 8px;
		margin: 6px 8px 0;
		padding: 6px 8px;
		border: 1px solid var(--color-accent);
		border-radius: 8px;
		background: var(--color-bg-elevated);
	}

	.prompt label {
		white-space: nowrap;
		color: var(--color-text-secondary);
	}

	.prompt input {
		flex: 1 1 auto;
		min-width: 0;
		height: 26px;
		padding: 0 8px;
		border: 1px solid var(--color-border);
		border-radius: 5px;
		background: var(--color-surface-sunken);
		color: var(--color-text-primary);
		outline: none;
	}

	.prompt input:focus {
		border-color: var(--color-accent);
	}

	.btn {
		height: 26px;
		padding: 0 12px;
		border: 1px solid var(--color-border);
		border-radius: 5px;
		background: transparent;
		color: var(--color-text-primary);
		cursor: pointer;
	}

	.btn.primary {
		border-color: var(--color-accent);
		background: var(--color-accent-fill, var(--color-accent));
		color: #fff;
	}

	.keys {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 6px 8px 8px;
		flex-wrap: wrap;
	}

	.key {
		display: flex;
		align-items: center;
		gap: 5px;
		height: 26px;
		padding: 0 9px 0 4px;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: var(--color-bg-secondary);
		color: var(--color-text-primary);
		font-size: 0.75rem;
		cursor: pointer;
	}

	.key:hover:not(:disabled) {
		border-color: var(--color-accent);
	}

	.key:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.key kbd {
		padding: 1px 4px;
		border-radius: 4px;
		background: var(--color-surface-strong);
		color: var(--color-text-secondary);
		font-family: inherit;
		font-size: 0.6875rem;
		font-weight: 600;
	}

	.hint {
		margin-left: auto;
		color: var(--color-text-tertiary);
		font-size: 0.71875rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}

	/* One side at a time: a switch between them on top. */
	.sides {
		display: flex;
		gap: 4px;
		padding: 8px 8px 0;
	}

	.side-tab {
		flex: 1 1 0;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		min-width: 0;
		height: 32px;
		border: 1px solid var(--color-border);
		border-radius: 8px;
		background: var(--color-bg-secondary);
		color: var(--color-text-secondary);
		font: inherit;
		font-weight: 600;
		cursor: pointer;
	}

	.side-tab.on {
		border-color: var(--color-accent);
		color: var(--color-accent);
		background: var(--color-accent-subtle);
	}

	.side-count {
		padding: 0 6px;
		border-radius: 9px;
		background: var(--color-warning);
		color: var(--color-bg-primary);
		font-size: 0.6875rem;
	}

	.storage-note {
		display: flex;
		align-items: center;
		gap: 8px;
		margin: 6px 8px;
		padding: 6px 8px;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: var(--color-bg-elevated);
		color: var(--color-text-secondary);
		font-size: 0.75rem;
	}

	.storage-note span {
		flex: 1 1 auto;
		min-width: 0;
	}

	.btn.small {
		flex: 0 0 auto;
		height: 28px;
	}

	/* Fingers: larger rows and buttons, and no keyboard hints. */
	.touch .row {
		height: 40px;
	}

	.touch .ib {
		width: 34px;
		height: 34px;
	}

	.touch .path,
	.touch .path-input {
		height: 32px;
	}

	.touch .key {
		height: 36px;
		padding: 0 12px;
	}

	.touch .keys {
		flex-wrap: nowrap;
		overflow-x: auto;
		padding-bottom: calc(8px + var(--safe-bottom, 0px));
	}

	.touch .key {
		flex: 0 0 auto;
	}

	@media (max-width: 760px) {
		.cols,
		.row {
			grid-template-columns: minmax(0, 1fr) 70px;
		}

		.modified,
		.col-modified,
		.hint {
			display: none;
		}
	}
</style>
