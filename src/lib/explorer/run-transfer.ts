import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { sftpTransferCancel } from '#lib/ipc/sftp.js';
import {
	addTransfer,
	completeTransfer,
	failTransfer,
	removeTransfer,
	updateTransferProgress
} from '#lib/state/transfers.svelte.js';

interface Progress {
	bytesTransferred: number;
	totalBytes: number;
	percent: number;
}

type TransferResult = 'done' | 'cancelled' | { error: string };

export interface TransferRun {
	kind: 'uploading' | 'downloading';
	/** The connection it runs on: its row shows only there. */
	connectionId: string;
	filename: string;
	totalBytes: number;
	/** Starts the work under the id given. */
	start: (transferId: string) => Promise<unknown>;
	/** Whether it gets a row in the transfer list. */
	visible?: boolean;
	/** Also told when the user cancels, after the backend is asked to stop. */
	onCancel?: () => void;
	/** Told the transfer's id once its row exists (the queue cancels and
	 *  follows it by that id). */
	onId?: (transferId: string) => void;
}

/**
 * Run one upload or download and settle when the backend says it ended.
 *
 * The id is made here and its events are listened to before the work
 * starts: a failure that comes at once, such as a folder the user may not
 * write (#161), used to be reported before anyone listened, and the
 * transfer then showed as running forever.
 *
 * Resolves `true` when done and `false` when the user cancelled it; rejects
 * with the backend's own message when it failed.
 */
export async function runTransfer(run: TransferRun): Promise<boolean> {
	const id = crypto.randomUUID();
	const visible = run.visible ?? true;
	let cancelled = false;
	let registered = false;
	let progress: Progress | undefined;

	let settle!: (result: TransferResult) => void;
	const ended = new Promise<TransferResult>((resolve) => (settle = resolve));
	const unlisten: UnlistenFn[] = await Promise.all([
		listen<Progress>(`transfer-progress-${id}`, (event) => {
			if (!visible) return;
			const p = event.payload;
			progress = p;
			if (registered) updateTransferProgress(id, p.bytesTransferred, p.totalBytes, p.percent);
		}),
		listen(`transfer-complete-${id}`, () => settle('done')),
		listen(`transfer-cancelled-${id}`, () => settle('cancelled')),
		listen<string>(`transfer-error-${id}`, (event) => {
			settle({ error: typeof event.payload === 'string' ? event.payload : 'Transfer failed' });
		})
	]);

	try {
		try {
			await run.start(id);
			registered = true;
		} catch (err) {
			settle({ error: String(err) });
		}

		// Expose cancellation only after the backend acknowledges the ID.
		if (visible) {
			addTransfer(id, run.filename, run.totalBytes, run.kind, registered ? () => {
				if (cancelled) return;
				cancelled = true;
				// An upload already writing a file finishes it before stopping.
				if (run.kind === 'downloading') removeTransfer(id);
				void sftpTransferCancel(id).catch((err) => settle({ error: String(err) }));
				run.onCancel?.();
			} : undefined, run.connectionId);
			if (progress) updateTransferProgress(id, progress.bytesTransferred, progress.totalBytes, progress.percent);
		}
		if (registered) run.onId?.(id);
		const result = await ended;
		if (typeof result === 'object') {
			if (visible) failTransfer(id, result.error);
			throw new Error(result.error);
		}
		// A stopped move must keep its source even if its last write finished.
		if (result === 'cancelled' || cancelled) {
			if (visible) removeTransfer(id);
			return false;
		}
		if (visible) completeTransfer(id);
		return true;
	} finally {
		for (const u of unlisten) u();
	}
}
