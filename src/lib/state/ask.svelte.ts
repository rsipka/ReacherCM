/**
 * A yes/no question in Reach's own dialog, in place of the browser's
 * `confirm()`: that one is an unstyled system box with a generic title, out
 * of the theme and the phone's safe areas, and it stops the page while open.
 * Shown by AskHost, mounted once in the layout.
 */

export interface AskOptions {
	/** The confirming button's label; Confirm by default. */
	confirmLabel?: string;
	/** The confirming button's look: red (danger, the default: most
	 *  questions come before deleting something) or the accent colour. */
	confirmVariant?: 'danger' | 'primary';
	/** A third button between Cancel and the confirming one; see `askChoice`. */
	otherLabel?: string;
}

export type Choice = 'confirm' | 'other' | 'cancel';

interface Pending {
	message: string;
	options: AskOptions;
	resolve: (choice: Choice) => void;
}

let pending = $state<Pending | null>(null);

/** Resolves true when the person confirms, false when they cancel. A new
 *  question cancels one still open. */
export function ask(message: string, options: AskOptions = {}): Promise<boolean> {
	return askChoice(message, { ...options, otherLabel: undefined }).then((c) => c === 'confirm');
}

/** As `ask`, with a third answer when `otherLabel` is given. */
export function askChoice(message: string, options: AskOptions = {}): Promise<Choice> {
	pending?.resolve('cancel');
	return new Promise((resolve) => {
		pending = { message, options, resolve };
	});
}

export function currentQuestion(): Pending | null {
	return pending;
}

export function answer(choice: boolean | Choice): void {
	const p = pending;
	pending = null;
	p?.resolve(choice === true ? 'confirm' : choice === false ? 'cancel' : choice);
}
