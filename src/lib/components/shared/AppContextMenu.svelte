<script lang="ts">
	// Right-click anywhere the app has no menu of its own.
	//
	// Without this, the webview's own menu came up: Back, Reload and, in a
	// debug build, Inspect Element. Reload throws away every open terminal,
	// file list and tab (the reload keys are blocked for the same reason, see
	// isReloadKey), and none of it belongs in an app. So the webview's menu
	// never opens. A text field gets Undo, Cut, Copy, Paste and Select all;
	// selected text elsewhere gets Copy; everything else gets nothing.
	//
	// Components with a menu of their own (sessions, tabs, files, terminal)
	// call preventDefault on the event, and this leaves those alone: Svelte's
	// delegated handlers and listeners on elements run before this one on the
	// window.
	//
	// In a dev build, Shift+right-click still opens the webview's menu, for
	// Inspect Element.
	import DbMenu, { type MenuItem } from '#lib/components/db/DbMenu.svelte';
	import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager';
	import { isMac, isMobile } from '#lib/platform.js';
	import { t } from '#lib/state/i18n.svelte.js';

	let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

	const mod = isMac() ? '⌘' : 'Ctrl+';

	/** What a text field's menu acts on. */
	interface TextTarget {
		readOnly: boolean;
		/** False for a password: its text is never copied out. */
		canCopy: boolean;
		selected: string;
		canUndo: boolean;
		undo(): void;
		replaceSelection(text: string): void;
		selectAll(): void;
	}

	/** Inputs that hold typed text. Buttons, checkboxes and the like do not. */
	const TEXT_INPUTS = new Set(['text', 'search', 'url', 'tel', 'email', 'password', 'number']);

	function fieldTarget(el: HTMLInputElement | HTMLTextAreaElement): TextTarget {
		// Some input types (email, number) have no selection API: null here.
		const start = el.selectionStart;
		const end = el.selectionEnd;
		const restore = () => {
			el.focus();
			if (start !== null && end !== null) el.setSelectionRange(start, end);
		};
		return {
			readOnly: el.readOnly || el.disabled,
			canCopy: !(el instanceof HTMLInputElement && el.type === 'password'),
			selected: start !== null && end !== null ? el.value.slice(start, end) : '',
			canUndo: true,
			undo() {
				el.focus();
				document.execCommand('undo');
			},
			replaceSelection(text) {
				restore();
				// execCommand keeps the field's own undo history and fires the
				// input event that bind:value listens for.
				const done = text ? document.execCommand('insertText', false, text) : document.execCommand('delete');
				if (!done && start !== null && end !== null) {
					el.setRangeText(text, start, end, 'end');
					el.dispatchEvent(new Event('input', { bubbles: true }));
				}
			},
			selectAll() {
				el.focus();
				el.select();
			}
		};
	}

	/** A CodeMirror editor (SQL, YAML, the file editor), through its own API. */
	async function editorTarget(dom: HTMLElement): Promise<TextTarget | null> {
		const [{ EditorView }, commands] = await Promise.all([import('@codemirror/view'), import('@codemirror/commands')]);
		const view = EditorView.findFromDOM(dom);
		if (!view) return null;
		const { from, to } = view.state.selection.main;
		return {
			readOnly: view.state.readOnly,
			canCopy: true,
			selected: view.state.sliceDoc(from, to),
			canUndo: commands.undoDepth(view.state) > 0,
			undo() {
				commands.undo(view);
				view.focus();
			},
			replaceSelection(text) {
				view.dispatch(view.state.replaceSelection(text), { userEvent: text ? 'input.paste' : 'delete.cut', scrollIntoView: true });
				view.focus();
			},
			selectAll() {
				commands.selectAll(view);
				view.focus();
			}
		};
	}

	function textMenu(f: TextTarget): MenuItem[] {
		const copyable = f.canCopy && f.selected !== '';
		return [
			{ label: t('textmenu.undo'), hint: `${mod}Z`, disabled: f.readOnly || !f.canUndo, action: () => f.undo() },
			{ label: '', separator: true },
			{
				label: t('textmenu.cut'),
				hint: `${mod}X`,
				disabled: f.readOnly || !copyable,
				action: () => {
					void writeText(f.selected);
					f.replaceSelection('');
				}
			},
			{ label: t('textmenu.copy'), hint: `${mod}C`, disabled: !copyable, action: () => void writeText(f.selected) },
			{
				label: t('textmenu.paste'),
				hint: `${mod}V`,
				disabled: f.readOnly,
				action: async () => {
					const text = await readText().catch(() => '');
					if (text) f.replaceSelection(text);
				}
			},
			{ label: '', separator: true },
			{ label: t('textmenu.select_all'), hint: `${mod}A`, action: () => f.selectAll() }
		];
	}

	/** How the last press was made: a right click on a phone or tablet comes
	 *  from a mouse (Samsung DeX), where the long-press handles don't. */
	let lastPointer = '';
	function onpointerdown(e: PointerEvent): void {
		lastPointer = e.pointerType;
	}

	async function oncontextmenu(e: MouseEvent): Promise<void> {
		// A component opened its own menu, or the platform's long-press
		// handles text on a phone; a mouse there gets this menu, as on a desktop.
		const pointer = (e as PointerEvent).pointerType || lastPointer;
		if (e.defaultPrevented || (isMobile() && pointer !== 'mouse')) return;
		if (import.meta.env.DEV && e.shiftKey) return;
		e.preventDefault();
		menu = null;

		const { clientX: x, clientY: y } = e;
		const target = e.target instanceof Element ? e.target : null;

		const field = target?.closest('input, textarea');
		if (field instanceof HTMLTextAreaElement || (field instanceof HTMLInputElement && TEXT_INPUTS.has(field.type))) {
			menu = { x, y, items: textMenu(fieldTarget(field)) };
			return;
		}

		const editor = target?.closest<HTMLElement>('.cm-editor');
		if (editor) {
			const f = await editorTarget(editor);
			if (f) menu = { x, y, items: textMenu(f) };
			return;
		}

		const selected = window.getSelection()?.toString() ?? '';
		if (selected.trim()) {
			menu = { x, y, items: [{ label: t('textmenu.copy'), hint: `${mod}C`, action: () => void writeText(selected) }] };
		}
	}
</script>

<svelte:window {oncontextmenu} onpointerdowncapture={onpointerdown} />

{#if menu}
	<DbMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />
{/if}
