/**
 * The transfer queue of the full-pane file manager: copies and moves wait
 * their turn and run one at a time for each connection, so more can be
 * added while one is running. A queued item can be dropped, the running one
 * cancelled, and a failed or cancelled one tried again.
 */
import { getTransfers } from '#lib/state/transfers.svelte.js';
import { sftpDelete } from '#lib/ipc/sftp.js';
import { localDelete } from '#lib/ipc/localfs.js';
import type { FileEntry } from '#lib/state/explorer.svelte.js';
import { copyEntry } from './dual-copy.js';
import type { PaneKind } from './pane.svelte.js';

export type JobStatus = 'queued' | 'running' | 'done' | 'failed' | 'cancelled';

export interface QueueJob {
	id: string;
	conn: string;
	entry: FileEntry;
	/** The side it is copied from. */
	from: PaneKind;
	toDir: string;
	/** The source is deleted once the copy is done. */
	move: boolean;
	status: JobStatus;
	error?: string;
	/** The transfer running for it now (a folder download has one per file). */
	transferId?: string;
}

/** Told as each job ends, and when the queue of a connection runs dry. */
export interface QueueEvents {
	onJobEnd?: (job: QueueJob) => void;
	onIdle?: () => void;
}

let jobs = $state<QueueJob[]>([]);
/** The connections whose queue is being worked through. */
const pumping = new Set<string>();
/** The newest listener of each connection (its file manager). */
const listeners = new Map<string, QueueEvents>();

/** The jobs of one connection, in the order they were added. */
export function getQueue(conn: string): QueueJob[] {
	return jobs.filter((j) => j.conn === conn);
}

/** How far the running job's current transfer is, 0 to 100. */
export function jobPercent(job: QueueJob): number {
	if (job.status === 'done') return 100;
	if (job.status !== 'running' || !job.transferId) return 0;
	return getTransfers().find((tr) => tr.id === job.transferId)?.percent ?? 0;
}

export function listenQueue(conn: string, events: QueueEvents): () => void {
	listeners.set(conn, events);
	return () => {
		if (listeners.get(conn) === events) listeners.delete(conn);
	};
}

/** Add copies (or moves) of `entries` from one side to `toDir` on the other. */
export function enqueue(conn: string, entries: FileEntry[], from: PaneKind, toDir: string, move: boolean): void {
	for (const entry of entries) {
		jobs.push({ id: crypto.randomUUID(), conn, entry, from, toDir, move, status: 'queued' });
	}
	void pump(conn);
}

function find(id: string): QueueJob | undefined {
	return jobs.find((j) => j.id === id);
}

async function pump(conn: string): Promise<void> {
	if (pumping.has(conn)) return;
	pumping.add(conn);
	try {
		for (;;) {
			const job = jobs.find((j) => j.conn === conn && j.status === 'queued');
			if (!job) break;
			await run(job.id);
		}
	} finally {
		pumping.delete(conn);
	}
	listeners.get(conn)?.onIdle?.();
}

async function run(id: string): Promise<void> {
	const start = find(id);
	if (!start) return;
	start.status = 'running';
	start.error = undefined;
	const { conn, entry, from, toDir, move } = start;
	const outcome = await copyEntry(conn, entry, from, toDir, (transferId) => {
		const job = find(id);
		if (job) job.transferId = transferId;
	});
	const job = find(id);
	// Dropped while running (the connection closed).
	if (!job) return;
	job.transferId = undefined;
	if (outcome === 'cancelled') {
		job.status = 'cancelled';
	} else if (outcome !== 'done') {
		job.status = 'failed';
		job.error = outcome.error;
	} else {
		job.status = 'done';
		if (move) {
			try {
				await (from === 'local' ? localDelete(entry.path) : sftpDelete(conn, entry.path));
			} catch (err) {
				job.status = 'failed';
				job.error = err instanceof Error ? err.message : String(err);
			}
		}
	}
	listeners.get(conn)?.onJobEnd?.(job);
}

/** Drop a waiting job, or call off the running one. */
export function cancelJob(id: string): void {
	const job = find(id);
	if (!job) return;
	if (job.status === 'queued') {
		job.status = 'cancelled';
		return;
	}
	if (job.status === 'running' && job.transferId) {
		getTransfers()
			.find((tr) => tr.id === job.transferId)
			?.cancel?.();
	}
}

/** Put a failed or cancelled job back at the end of the queue. */
export function retryJob(id: string): void {
	const job = find(id);
	if (!job || (job.status !== 'failed' && job.status !== 'cancelled')) return;
	jobs.splice(jobs.indexOf(job), 1);
	jobs.push({ ...job, status: 'queued', error: undefined, transferId: undefined });
	void pump(job.conn);
}

export function removeJob(id: string): void {
	const i = jobs.findIndex((j) => j.id === id && j.status !== 'running');
	if (i >= 0) jobs.splice(i, 1);
}

/** Take away the jobs of a connection that have ended. */
export function clearFinished(conn: string): void {
	jobs = jobs.filter((j) => j.conn !== conn || j.status === 'queued' || j.status === 'running');
}

/** The connection closed: its queue goes, the running copy is called off. */
export function dropQueue(conn: string): void {
	for (const job of getQueue(conn)) if (job.status === 'running') cancelJob(job.id);
	jobs = jobs.filter((j) => j.conn !== conn);
	listeners.delete(conn);
}
