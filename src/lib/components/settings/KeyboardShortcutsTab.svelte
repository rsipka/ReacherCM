<script lang="ts">
	import { onDestroy, tick } from 'svelte';
	import Button from '#lib/components/shared/Button.svelte';
	import SettingsCard from './SettingsCard.svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSettings, updateSetting } from '#lib/state/settings.svelte.js';
	import { setRecordingShortcut } from '#lib/state/shortcuts.svelte.js';
	import { isMac } from '#lib/platform.js';
	import {
		ACTIONS,
		GROUP_LABELS,
		assignKey,
		chordFromEvent,
		conflictFor,
		conflicts,
		displayChord,
		isDefault,
		keysFor,
		rejectReason,
		removeKey,
		replaceKey,
		resetAction,
		type ActionGroup,
		type ActionId,
		type KeyOverrides
	} from '#lib/state/keybindings.js';

	const settings = getSettings();
	const mac = isMac();
	const groups: ActionGroup[] = ['tabs', 'app', 'terminal', 'split'];

	let overrides = $derived<KeyOverrides>(settings.keyboardShortcuts ?? {});
	let shared = $derived(conflicts(overrides, mac));

	/** The action taking a new key, and which of its keys it replaces. */
	let recording = $state<{ id: ActionId; replacing?: string } | null>(null);
	/** A key pressed that another action has: asked before it moves. */
	let pending = $state<{ id: ActionId; replacing?: string; chord: string; owner: ActionId } | null>(null);
	let problem = $state<{ id: ActionId; text: string } | null>(null);
	let recorderEl = $state<HTMLElement | null>(null);

	function label(id: ActionId): string {
		const a = ACTIONS.find((x) => x.id === id);
		return a ? t(a.label, a.labelParams) : id;
	}

	function save(next: KeyOverrides): void {
		updateSetting('keyboardShortcuts', next);
	}

	async function startRecording(id: ActionId, replacing?: string): Promise<void> {
		pending = null;
		problem = null;
		recording = { id, replacing };
		setRecordingShortcut(true);
		window.addEventListener('keydown', onRecordKey, { capture: true });
		await tick();
		recorderEl?.focus();
	}

	function stopRecording(): void {
		recording = null;
		setRecordingShortcut(false);
		window.removeEventListener('keydown', onRecordKey, { capture: true });
	}

	function onRecordKey(e: KeyboardEvent): void {
		if (!recording) return;
		// Nothing else hears the key: not the app's shortcuts, not the
		// dialog's Escape, not the terminal behind it.
		e.preventDefault();
		e.stopImmediatePropagation();
		const plain = !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey;
		if (plain && e.key === 'Escape') {
			stopRecording();
			return;
		}
		const chord = chordFromEvent(e, mac);
		if (!chord) return; // a modifier on its own: wait for the key
		const { id, replacing } = recording;
		if (rejectReason(chord)) {
			problem = { id, text: t('shortcuts.needs_modifier', { key: displayChord(chord, mac) }) };
			return;
		}
		stopRecording();
		const owner = conflictFor(chord, overrides, mac, id);
		if (owner) {
			pending = { id, replacing, chord, owner };
			return;
		}
		apply(id, chord, replacing);
	}

	function apply(id: ActionId, chord: string, replacing?: string): void {
		save(replacing ? replaceKey(overrides, id, replacing, chord, mac) : assignKey(overrides, id, chord, mac));
		pending = null;
		problem = null;
	}

	function resetAll(): void {
		stopRecording();
		pending = null;
		problem = null;
		save({});
	}

	onDestroy(stopRecording);
</script>

<div class="tab-content">
	<SettingsCard>
		<div class="setting-row">
			<div class="setting-info">
				<span class="setting-label">{t('shortcuts.title')}</span>
				<span class="setting-description">{t('shortcuts.desc')}</span>
			</div>
			<div class="setting-control">
				<Button variant="secondary" size="sm" disabled={Object.keys(overrides).length === 0} onclick={resetAll}>
					{t('shortcuts.reset_all')}
				</Button>
			</div>
		</div>
		{#if shared.size > 0}
			<p class="warn" role="alert">
				{t('shortcuts.shared_warning', {
					list: [...shared].map(([k, ids]) => `${displayChord(k, mac)} (${ids.map(label).join(', ')})`).join('; ')
				})}
			</p>
		{/if}
	</SettingsCard>

	{#each groups as group (group)}
		<SettingsCard title={t(GROUP_LABELS[group])}>
			{#each ACTIONS.filter((a) => a.group === group) as action (action.id)}
				{@const keys = keysFor(action.id, overrides, mac)}
				{@const changed = !isDefault(action.id, overrides, mac)}
				<div class="shortcut-row" class:changed>
					<span class="setting-label">{t(action.label, action.labelParams)}</span>
					<div class="keys">
						{#if keys.length === 0 && recording?.id !== action.id}
							<span class="none">{t('shortcuts.none')}</span>
						{/if}
						{#each keys as key (key)}
							{#if recording?.id === action.id && recording.replacing === key}
								<span class="chip recording" tabindex="-1" bind:this={recorderEl} onblur={stopRecording}>{t('shortcuts.press_keys')}</span>
							{:else}
								<span class="chip">
									<button class="chip-key" title={t('shortcuts.change')} onclick={() => startRecording(action.id, key)}>
										<kbd>{displayChord(key, mac)}</kbd>
									</button>
									<button
										class="chip-remove"
										aria-label={t('shortcuts.remove', { key: displayChord(key, mac) })}
										title={t('shortcuts.remove', { key: displayChord(key, mac) })}
										onclick={() => save(removeKey(overrides, action.id, key, mac))}
									>×</button>
								</span>
							{/if}
						{/each}
						{#if recording?.id === action.id && !recording.replacing}
							<span class="chip recording" tabindex="-1" bind:this={recorderEl} onblur={stopRecording}>{t('shortcuts.press_keys')}</span>
						{:else}
							<button class="icon-btn" title={t('shortcuts.add')} aria-label={t('shortcuts.add')} onclick={() => startRecording(action.id)}>+</button>
						{/if}
						<button
							class="icon-btn reset"
							class:hidden={!changed}
							disabled={!changed}
							title={t('shortcuts.reset')}
							aria-label={t('shortcuts.reset')}
							onclick={() => save(resetAction(overrides, action.id, mac))}
						>↺</button>
					</div>
					{#if problem?.id === action.id}
						<p class="note">{problem.text}</p>
					{/if}
					{#if pending?.id === action.id}
						{@const p = pending}
						<div class="note conflict" role="alert">
							<span>{t('shortcuts.in_use', { key: displayChord(p.chord, mac), action: label(p.owner) })}</span>
							<Button variant="ghost" size="sm" onclick={() => (pending = null)}>{t('common.cancel')}</Button>
							<Button variant="primary" size="sm" onclick={() => apply(p.id, p.chord, p.replacing)}>{t('shortcuts.reassign')}</Button>
						</div>
					{/if}
				</div>
			{/each}
		</SettingsCard>
	{/each}
</div>

<style>
	.shortcut-row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		column-gap: 16px;
		padding: 8px 0;
		border-bottom: 1px solid var(--color-border);
	}

	.shortcut-row:last-child {
		border-bottom: none;
	}

	.shortcut-row.changed .setting-label::after {
		content: ' •';
		color: var(--color-accent);
	}

	.keys {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		align-items: center;
		gap: 6px;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		border: 1px solid var(--color-border);
		border-radius: 6px;
		background: var(--color-bg-tertiary, var(--color-bg-secondary));
		font-size: 0.75rem;
		line-height: 1;
	}

	.chip.recording {
		padding: 6px 10px;
		border-color: var(--color-accent);
		color: var(--color-accent);
		outline: none;
		animation: pulse 1.2s ease-in-out infinite;
	}

	@keyframes pulse {
		50% {
			opacity: 0.55;
		}
	}

	.chip-key,
	.chip-remove {
		background: none;
		border: none;
		color: inherit;
		cursor: pointer;
		font: inherit;
	}

	.chip-key {
		padding: 5px 4px 5px 8px;
	}

	.chip-key kbd {
		font-family: var(--font-mono, monospace);
		font-size: 0.75rem;
		color: var(--color-text-primary);
	}

	.chip-remove {
		padding: 5px 7px 5px 4px;
		color: var(--color-text-tertiary);
	}

	.chip-key:hover kbd,
	.chip-remove:hover {
		color: var(--color-accent);
	}

	.icon-btn {
		width: 24px;
		height: 24px;
		border: 1px dashed var(--color-border);
		border-radius: 6px;
		background: none;
		color: var(--color-text-secondary);
		cursor: pointer;
		font-size: 0.875rem;
		line-height: 1;
	}

	.icon-btn:hover:not(:disabled) {
		color: var(--color-accent);
		border-color: var(--color-accent);
	}

	.icon-btn.reset {
		border-style: solid;
		border-color: transparent;
	}

	.icon-btn.hidden {
		visibility: hidden;
	}

	.none {
		font-size: 0.75rem;
		color: var(--color-text-tertiary);
	}

	.note {
		grid-column: 1 / -1;
		margin: 6px 0 0;
		font-size: 0.75rem;
		color: var(--color-warning);
	}

	.note.conflict {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: flex-end;
		gap: 8px;
		color: var(--color-text-secondary);
	}

	.note.conflict span {
		flex: 1;
		min-width: 12rem;
	}

	.warn {
		margin: 8px 0 4px;
		font-size: 0.75rem;
		color: var(--color-warning);
	}
</style>
