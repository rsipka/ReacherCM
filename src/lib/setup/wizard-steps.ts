/**
 * Which screens the setup wizard shows, in order.
 *
 * No Svelte here, so a Node test can check the order
 * (scripts/setup-wizard-test.mjs).
 *
 * The first run asks everything. Run again from Settings, it leaves out
 * plugins, which only make sense once (Settings > Plugins has them). Where
 * the vault syncs, Turso included, is chosen on the Sync page.
 */

export type WizardStep = 'personalize' | 'sessions' | 'logging' | 'security' | 'plugins' | 'account' | 'complete';

export interface WizardOptions {
	/** Opened from Settings, after the first run. */
	rerun: boolean;
}

export function wizardSteps({ rerun }: WizardOptions): WizardStep[] {
	return rerun
		? ['personalize', 'sessions', 'logging', 'security', 'account', 'complete']
		: ['personalize', 'sessions', 'logging', 'security', 'plugins', 'account', 'complete'];
}

/** Where "Skip the rest" goes: the last screen, which only sums up and
 *  finishes. Everything not yet seen keeps its default. */
export function skipTarget(steps: WizardStep[]): number {
	return steps.length - 1;
}
