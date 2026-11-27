<script lang="ts">
	import Button from '#lib/components/shared/Button.svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { getSettings } from '#lib/state/settings.svelte.js';
	import { getSyncAccount } from '#lib/state/preference-sync.svelte.js';
	import { isMobile } from '#lib/platform.js';

	interface Props {
		onComplete: () => void;
		onBack: () => void;
		/** Opened again from Settings. */
		rerun?: boolean;
	}

	let { onComplete, onBack, rerun = false }: Props = $props();

	const settings = getSettings();
	const mobile = isMobile();
	let account = $derived(getSyncAccount());

	const LOG_LABEL = { off: 'settings.log_off', printable: 'settings.log_printable', all: 'settings.log_all' } as const;
	const HISTORY_LABEL = { off: 'history.mode_off', commands: 'history.mode_commands', output: 'history.mode_output' } as const;

	/** What the new steps chose, beside language. */
	let rows = $derived(
		[
			{ label: t('settings.language'), value: settings.locale === 'en' ? 'English' : settings.locale, show: true },
			{
				label: t('setup.open_with'),
				value: t(settings.doubleClickToOpen ? 'setup.open_double' : 'setup.open_single'),
				show: !mobile
			},
			{ label: t('settings.log_section'), value: t(LOG_LABEL[settings.sessionLogMode] ?? 'settings.log_off'), show: true },
			{
				label: t('history.settings_section'),
				value: t(HISTORY_LABEL[settings.historyMode] ?? 'history.mode_off'),
				show: !mobile
			},
			{
				label: t('security.auto_lock'),
				value: settings.autoLockMinutes
					? t('security.auto_lock_minutes', { count: settings.autoLockMinutes })
					: t('security.auto_lock_never'),
				show: true
			},
			{
				label: t('settings.check_updates'),
				value: t(settings.checkForUpdates ? 'setup.updates_auto' : 'setup.updates_manual'),
				show: true
			},
			{
				label: t('setup.account_title'),
				value: account?.username ?? '',
				show: account !== null
			}
		].filter((r) => r.show)
	);
</script>

<div class="step">
	<div class="step-header">
		<div class="checkmark">
			<svg width="48" height="48" viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg">
				<circle cx="24" cy="24" r="24" fill="var(--color-success)" opacity="0.15"/>
				<circle cx="24" cy="24" r="18" fill="var(--color-success)" opacity="0.25"/>
				<path d="M16 24L22 30L32 18" stroke="var(--color-success)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
			</svg>
		</div>
		<h2 class="step-title">{t('setup.complete_title')}</h2>
		<p class="step-subtitle">{t('setup.complete_subtitle')}</p>
		<p class="step-hint">{t('setup.complete_rerun_hint')}</p>
	</div>

	<div class="summary">
		{#each rows as row (row.label)}
			<div class="summary-item">
				<span class="summary-icon">
					<svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M13.3 4.7L6.5 11.5L2.7 7.7" stroke="var(--color-success)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
				</span>
				<span class="summary-label">{row.label}</span>
				<span class="summary-value">{row.value}</span>
			</div>
		{/each}
	</div>

	<div class="step-actions">
		<Button variant="ghost" size="md" onclick={onBack}>
			{t('setup.back')}
		</Button>
		<Button variant="primary" size="md" onclick={onComplete}>
			{rerun ? t('setup.done') : t('setup.get_started')}
		</Button>
	</div>
</div>

<style>
	.step {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 28px;
		width: 100%;
	}

	.step-header {
		display: flex;
		flex-direction: column;
		align-items: center;
		text-align: center;
		gap: 12px;
	}

	.checkmark {
		margin-bottom: 4px;
	}

	.step-title {
		margin: 0;
		font-size: 1.25rem;
		font-weight: 600;
		color: var(--color-text-primary);
	}

	.step-subtitle {
		margin: 0;
		font-size: 0.8125rem;
		color: var(--color-text-secondary);
	}

	.step-hint {
		margin: 0;
		font-size: var(--text-xs);
		color: var(--color-text-tertiary);
	}

	.summary {
		display: flex;
		flex-direction: column;
		gap: 8px;
		width: 100%;
		max-width: 320px;
	}

	.summary-item {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 14px;
		background: var(--color-bg-elevated);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-btn);
	}

	.summary-icon {
		display: flex;
		align-items: center;
		flex-shrink: 0;
	}

	.summary-label {
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-text-primary);
		flex: 1;
	}

	.summary-value {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		overflow-wrap: anywhere;
	}

	.step-actions {
		display: flex;
		justify-content: center;
		gap: 12px;
		width: 100%;
	}
</style>
