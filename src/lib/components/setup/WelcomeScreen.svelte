<script lang="ts">
	import PersonalizeStep from './PersonalizeStep.svelte';
	import SessionsStep from './SessionsStep.svelte';
	import LoggingStep from './LoggingStep.svelte';
	import SecurityStep from './SecurityStep.svelte';
	import PluginsStep from './PluginsStep.svelte';
	import TursoStep from './TursoStep.svelte';
	import AccountStep from './AccountStep.svelte';
	import CompleteStep from './CompleteStep.svelte';
	import { t } from '#lib/state/i18n.svelte.js';
	import { updateSetting } from '#lib/state/settings.svelte.js';
	import { setupWizard, closeSetupWizard } from '#lib/state/setup-wizard.svelte.js';
	import { wizardSteps, skipTarget } from '#lib/setup/wizard-steps.js';

	/** Fixed when the wizard opens: a first run, or again from Settings. */
	const rerun = setupWizard.rerun;
	const steps = wizardSteps({ rerun });

	let currentStep = $state(0);
	let accountBusy = $state(false);
	let step = $derived(steps[currentStep]);
	let last = $derived(currentStep === steps.length - 1);

	function next() {
		if (currentStep < steps.length - 1) {
			currentStep++;
		}
	}

	function back() {
		if (currentStep > 0) {
			currentStep--;
		}
	}

	/** Everything not yet seen keeps its default; the summary still shows,
	 *  so it is clear what was chosen and how to finish. */
	function skipRest() {
		currentStep = skipTarget(steps);
	}

	function complete() {
		updateSetting('setupComplete', true);
		closeSetupWizard();
	}
</script>

<div class="welcome-overlay">
	<div class="welcome-container">
		<div class="top-row">
			<!-- Step indicator dots -->
			<div class="step-dots">
				{#each steps as id, i (id)}
					<div class="dot" class:active={i === currentStep} class:completed={i < currentStep}></div>
				{/each}
			</div>
			{#if !last}
				<button type="button" class="skip-rest" disabled={accountBusy} onclick={skipRest}>{t('setup.skip_rest')}</button>
			{/if}
		</div>

		<!-- Welcome header (only on first step) -->
		{#if currentStep === 0}
			<div class="welcome-header">
				<h1 class="welcome-title">{rerun ? t('setup.rerun_title') : t('setup.welcome_title')}</h1>
				<p class="welcome-subtitle">{rerun ? t('setup.rerun_subtitle') : t('setup.welcome_subtitle')}</p>
			</div>
		{/if}

		<!-- Step content -->
		<div class="step-content">
			{#if step === 'personalize'}
				<PersonalizeStep onNext={next} />
			{:else if step === 'sessions'}
				<SessionsStep onNext={next} onBack={back} />
			{:else if step === 'logging'}
				<LoggingStep onNext={next} onBack={back} />
			{:else if step === 'security'}
				<SecurityStep onNext={next} onBack={back} />
			{:else if step === 'plugins'}
				<PluginsStep onNext={next} onBack={back} />
			{:else if step === 'storage'}
				<TursoStep onNext={next} onBack={back} />
			{:else if step === 'account'}
				<AccountStep onNext={next} onBack={back} bind:busy={accountBusy} />
			{:else if step === 'complete'}
				<CompleteStep onComplete={complete} onBack={back} {rerun} />
			{/if}
		</div>
	</div>
</div>

<style>
	.welcome-overlay {
		position: fixed;
		inset: 0;
		z-index: 200;
		display: flex;
		/* Scroll when the wizard is taller than the screen (small/mobile
		   viewports) so the action buttons stay reachable, and keep clear of
		   the status/navigation bars via safe-area insets. `margin: auto` on the
		   child centers it when it fits without clipping the top when it doesn't. */
		overflow-y: auto;
		background: var(--color-surface-sunken);
		backdrop-filter: blur(24px);
		-webkit-backdrop-filter: blur(24px);
		padding: max(20px, var(--safe-top)) 16px max(20px, var(--safe-bottom));
	}

	.welcome-container {
		margin: auto;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 24px;
		padding: 40px clamp(20px, 5vw, 48px);
		max-width: 520px;
		width: 100%;
		/*
		 * Never taller than the window. A step's content is not fixed — it grows
		 * with the number of themes on offer and with translated strings that
		 * run longer than the English — so on a short laptop or a tablet in
		 * landscape the card could exceed the viewport and put the primary
		 * action below the fold. Capping here and scrolling the step body (not
		 * the page) keeps the wizard usable at any height. `dvh` rather than
		 * `vh` so mobile browser chrome is accounted for.
		 */
		max-height: calc(100dvh - 2 * max(20px, var(--safe-top)));
		min-height: 0;
		background: var(--color-bg-secondary);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-modal);
		box-shadow: var(--shadow-elevated);
	}

	/* The dots stay centred; Skip sits at the right edge without pushing them. */
	.top-row {
		position: relative;
		display: flex;
		justify-content: center;
		align-items: center;
		width: 100%;
		min-height: 20px;
	}

	.step-dots {
		display: flex;
		gap: 8px;
	}

	.skip-rest {
		position: absolute;
		right: 0;
		padding: 0;
		font-family: inherit;
		font-size: var(--text-xs);
		color: var(--color-text-secondary);
		background: none;
		border: none;
		cursor: pointer;
	}

	.skip-rest:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.skip-rest:hover:not(:disabled) {
		color: var(--color-accent);
		text-decoration: underline;
	}

	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--color-border);
		transition: background-color var(--duration-default) var(--ease-default),
			transform var(--duration-default) var(--ease-default);
	}

	.dot.active {
		background: var(--color-accent-fill);
		transform: scale(1.25);
	}

	.dot.completed {
		background: var(--color-success);
	}

	.welcome-header {
		text-align: center;
	}

	.welcome-title {
		margin: 0;
		font-size: 1.5rem;
		font-weight: 700;
		color: var(--color-text-primary);
	}

	.welcome-subtitle {
		margin: 6px 0 0;
		font-size: 0.875rem;
		color: var(--color-text-secondary);
	}

	.step-content {
		width: 100%;
		/* Takes the leftover height and scrolls its own overflow, so the step
		   dots stay pinned at the top and each step's actions can stick to the
		   bottom rather than sliding off. */
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}
</style>
