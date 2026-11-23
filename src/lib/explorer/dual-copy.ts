/**
 * Copying between the two sides of the full-pane file manager. Uploads take
 * a whole folder in one transfer (the backend walks it); a server folder is
 * walked here and fetched file by file, since a download is one file.
 */
import { sftpDownload, sftpListDir, sftpUpload } from '#lib/ipc/sftp.js';
import { localJoin, localMkdir } from '#lib/ipc/localfs.js';
import { runTransfer } from './run-transfer.js';
import { remoteJoin, type PaneKind } from './pane.svelte.js';
import type { FileEntry } from '#lib/state/explorer.svelte.js';

/** How one item's copy ended. */
export type CopyOutcome = 'done' | 'cancelled' | { error: string };

function errorText(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

/** Fetch a server folder into `localParent`, merging into a folder of the
 *  same name already there. False when the person cancelled a file. */
async function downloadTree(conn: string, remoteDir: string, localParent: string, name: string, onId?: (id: string) => void): Promise<boolean> {
	const localDir = await localMkdir(localParent, name, true);
	const children = await sftpListDir(conn, remoteDir);
	for (const child of children) {
		if (child.isDirectory) {
			if (!(await downloadTree(conn, child.path, localDir, child.name, onId))) return false;
			continue;
		}
		const target = await localJoin(localDir, child.name);
		const done = await runTransfer({
			kind: 'downloading',
			connectionId: conn,
			filename: child.name,
			totalBytes: child.size,
			start: (id) => sftpDownload(conn, child.path, target, id),
			onId
		});
		if (!done) return false;
	}
	return true;
}

/** Copy one entry of the `from` side into `toDir` on the other side.
 *  `onId` hears the id of each transfer as it starts. */
export async function copyEntry(conn: string, entry: FileEntry, from: PaneKind, toDir: string, onId?: (id: string) => void): Promise<CopyOutcome> {
	try {
		if (from === 'local') {
			const done = await runTransfer({
				kind: 'uploading',
				connectionId: conn,
				filename: entry.name,
				totalBytes: entry.isDirectory ? 0 : entry.size,
				start: (id) => sftpUpload(conn, entry.path, remoteJoin(toDir, entry.name), id),
				onId
			});
			return done ? 'done' : 'cancelled';
		}
		if (entry.isDirectory) {
			return (await downloadTree(conn, entry.path, toDir, entry.name, onId)) ? 'done' : 'cancelled';
		}
		const target = await localJoin(toDir, entry.name);
		const done = await runTransfer({
			kind: 'downloading',
			connectionId: conn,
			filename: entry.name,
			totalBytes: entry.size,
			start: (id) => sftpDownload(conn, entry.path, target, id),
			onId
		});
		return done ? 'done' : 'cancelled';
	} catch (err) {
		return { error: errorText(err) };
	}
}
