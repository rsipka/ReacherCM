<script lang="ts">
	/**
	 * The keys on the bar above a phone's keyboard, and their order. Tap a key
	 * on the bar to pick it, then move it or take it off; tap one below to add
	 * it at the end.
	 */
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSettings, updateSetting } from '#lib/state/settings.svelte.js';
	import { ALL_BAR_KEYS, DEFAULT_BAR_KEYS, barKeyLabel, barKeys } from '#lib/terminal/bar-keys.js';

	const settings = getSettings();
	let keys = $derived(barKeys(settings.mobileBarKeys));
	let available = $derived(ALL_BAR_KEYS.filter((k) => !keys.includes(k)));
	let picked = $state<string | null>(null);
	let isDefault = $derived(keys.join() === DEFAULT_BAR_KEYS.join());

	function save(next: string[]): void {
		updateSetting('mobileBarKeys', next.join() === DEFAULT_BAR_KEYS.join() ? null : next);
	}

	function move(by: number): void {
		if (!picked) return;
		const i = keys.indexOf(picked);
		const j = i + by;
		if (i < 0 || j < 0 || j >= keys.length) return;
		const next = [...keys];
		[next[i], next[j]] = [next[j], next[i]];
		save(next);
	}

	function remove(): void {
		if (!picked || keys.length <= 1) return;
		save(keys.filter((k) => k !== picked));
		picked = null;
	}

	function add(id: string): void {
		save([...keys, id]);
	}
</script>

<div class="setting-row">
	<div class="setting-info">
		<span class="setting-label">{t('phonekeys.show')}</span>
		<span class="setting-description">{t('phonekeys.show_desc')}</span>
	</div>
	<div class="setting-control">
		<select class="show-field" value={settings.mobileBarShow} aria-label={t('phonekeys.show')} onchange={(e) => updateSetting('mobileBarShow', (e.currentTarget as HTMLSelectElement).value as 'auto' | 'always' | 'never')}>
			<option value="auto">{t('phonekeys.show_auto')}</option>
			<option value="always">{t('phonekeys.show_always')}</option>
			<option value="never">{t('phonekeys.show_never')}</option>
		</select>
	</div>
</div>

<div class="setting-section phone-keys">
	<div class="setting-info">
		<span class="setting-label">{t('phonekeys.title')}</span>
		<span class="setting-description">{t('phonekeys.desc')}</span>
	</div>

	<div class="bar" role="listbox" aria-label={t('phonekeys.on_bar')}>
		{#each keys as id (id)}
			<button
				type="button"
				class="key"
				class:picked={picked === id}
				role="option"
				aria-selected={picked === id}
				onclick={() => (picked = picked === id ? null : id)}>{barKeyLabel(id)}</button
			>
		{/each}
	</div>

	{#if picked}
		<div class="picked-actions">
			<button type="button" class="act" onclick={() => move(-1)} disabled={keys.indexOf(picked) === 0}>‹ {t('phonekeys.left')}</button>
			<button type="button" class="act" onclick={() => move(1)} disabled={keys.indexOf(picked) === keys.length - 1}>{t('phonekeys.right')} ›</button>
			<button type="button" class="act danger" onclick={remove} disabled={keys.length <= 1}>{t('phonekeys.remove')}</button>
		</div>
	{/if}

	{#if available.length > 0}
		<span class="sub-label">{t('phonekeys.add')}</span>
		<div class="bar available">
			{#each available as id (id)}
				<button type="button" class="key add" onclick={() => add(id)}>+ {barKeyLabel(id)}</button>
			{/each}
		</div>
	{/if}

	{#if !isDefault}
		<button type="button" class="reset" onclick={() => { picked = null; updateSetting('mobileBarKeys', null); }}>{t('phonekeys.reset')}</button>
	{/if}
</div>

<style>
	.show-field {
		width: 100%;
		min-width: 0;
		padding: 7px 10px;
		border-radius: var(--radius-btn);
		border: 1px solid var(--color-border-strong);
		background: var(--color-bg-primary);
		color: var(--color-text-primary);
		font-family: inherit;
		font-size: 0.8125rem;
	}

	.phone-keys {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 14px 0;
	}

	.bar {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.key {
		min-width: 40px;
		height: 34px;
		padding: 0 9px;
		font-family: var(--font-mono, monospace);
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		background: var(--color-surface-hover);
		border: 1px solid var(--color-border-strong, var(--color-border));
		border-radius: 8px;
		cursor: pointer;
	}

	.key.picked {
		color: var(--color-bg-primary);
		background: var(--color-accent-fill, var(--color-accent));
		border-color: var(--color-accent);
	}

	.key.add {
		color: var(--color-text-secondary);
		background: transparent;
		border-style: dashed;
	}

	.picked-actions {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
	}

	.act,
	.reset {
		padding: 6px 12px;
		font-family: inherit;
		font-size: 0.8125rem;
		color: var(--color-text-primary);
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border-strong, var(--color-border));
		border-radius: var(--radius-btn, 6px);
		cursor: pointer;
	}

	.act:disabled {
		opacity: 0.45;
		cursor: default;
	}

	.act.danger {
		color: var(--color-danger, #e5484d);
	}

	.reset {
		align-self: flex-start;
		color: var(--color-accent);
		background: none;
		border: none;
		padding: 0;
	}

	.sub-label {
		font-size: 0.75rem;
		font-weight: 600;
		color: var(--color-text-secondary);
	}
</style>
