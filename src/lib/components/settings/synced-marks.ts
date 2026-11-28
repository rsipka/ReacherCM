/**
 * A small cloud after the label of every Settings row whose value syncs
 * through the Reacher CM account.
 *
 * Rows are found by the text of their label, as Settings search finds them
 * (settings-index.ts), so a page needs no markup of its own: the label keys
 * in preference-catalog.json are the same keys the rows show. The Sync
 * page's own list of preferences is left alone, since it already says
 * which ones sync.
 */

import { tooltip } from '#lib/utils/tooltip.js';

export interface SyncedMarks {
	/** Label texts (in the current language) of the rows that sync. */
	labels: ReadonlySet<string>;
	/** Tooltip and accessible name of the mark. */
	tip: string;
}

const CLOUD = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" '
	+ 'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">'
	+ '<path d="M17.5 19H7a5 5 0 1 1 .9-9.92A6 6 0 0 1 19.5 11 4 4 0 0 1 17.5 19z"/></svg>';

export function syncedMarks(node: HTMLElement, initial: SyncedMarks) {
	let options = initial;
	const tips = new Map<HTMLElement, ReturnType<typeof tooltip>>();
	let queued = false;

	function labelText(label: HTMLElement): string {
		// The mark has no text of its own, so the label reads as before.
		return label.textContent?.trim() ?? '';
	}

	function apply(): void {
		queued = false;
		for (const label of node.querySelectorAll<HTMLElement>('.setting-label')) {
			const mark = label.querySelector<HTMLElement>(':scope > .synced-mark');
			const wanted = !label.closest('.preference-group') && options.labels.has(labelText(label));
			if (wanted && !mark) {
				const el = document.createElement('span');
				el.className = 'synced-mark';
				el.setAttribute('role', 'img');
				el.setAttribute('aria-label', options.tip);
				el.innerHTML = CLOUD;
				label.append(el);
				tips.set(el, tooltip(el, options.tip));
			} else if (!wanted && mark) {
				tips.get(mark)?.destroy();
				tips.delete(mark);
				mark.remove();
			} else if (mark && mark.getAttribute('aria-label') !== options.tip) {
				mark.setAttribute('aria-label', options.tip);
				tips.get(mark)?.update(options.tip);
			}
		}
		// Marks whose row went away with a page change.
		for (const [el, tip] of tips) {
			if (!el.isConnected) {
				tip.destroy();
				tips.delete(el);
			}
		}
	}

	function queue(): void {
		if (queued) return;
		queued = true;
		queueMicrotask(apply);
	}

	// Pages, Advanced sections and the language change under us.
	const observer = new MutationObserver(queue);
	observer.observe(node, { childList: true, subtree: true, characterData: true });
	apply();

	return {
		update(next: SyncedMarks) {
			options = next;
			queue();
		},
		destroy() {
			observer.disconnect();
			for (const tip of tips.values()) tip.destroy();
			tips.clear();
		}
	};
}
