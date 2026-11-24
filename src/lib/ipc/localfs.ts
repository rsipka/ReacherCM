import { invoke } from '@tauri-apps/api/core';
import type { FileEntry } from './sftp.js';

/** A folder on this computer: its full path, its parent (none at a root)
 *  and what is in it. Desktop only. */
export interface LocalListing {
	path: string;
	parent: string | null;
	entries: FileEntry[];
}

/** An empty path lists the home folder. */
export async function localListDir(path: string): Promise<LocalListing> {
	return invoke<LocalListing>('local_list_dir', { path });
}

/** Make `name` in `dir`; with `existOk`, a folder already there is fine. */
export async function localMkdir(dir: string, name: string, existOk = false): Promise<string> {
	return invoke<string>('local_mkdir', { dir, name, existOk });
}

/** Rename in place; a name already taken is refused. */
export async function localRename(path: string, newName: string): Promise<string> {
	return invoke<string>('local_rename', { path, newName });
}

/** Delete a file, or a folder with everything in it. */
export async function localDelete(path: string): Promise<void> {
	return invoke('local_delete', { path });
}

/** `dir` + `name` as this computer writes a path. */
export async function localJoin(dir: string, name: string): Promise<string> {
	return invoke<string>('local_join', { dir, name });
}

/** The home folder, then the drives (Windows) or `/`. */
export async function localRoots(): Promise<string[]> {
	return invoke<string[]>('local_roots');
}

/** Create a tree beneath the local pane's selected folder without following links. */
export async function localPrepareDownloadDir(root: string, names: string[]): Promise<void> {
	return invoke('local_prepare_download_dir', { root, names });
}

/** A local file as text (UTF-8, up to 5 MB), for the viewer and Compare. */
export async function localReadText(path: string): Promise<string> {
	return invoke<string>('local_read_text', { path });
}

export interface LocalStat {
	modifiedMs: number;
	size: number;
}

export async function localStat(path: string): Promise<LocalStat> {
	return invoke<LocalStat>('local_stat', { path });
}

/** Where to put a server file opened in another program: a fresh
 *  temporary folder, under the file's own name. */
export async function localEditCopyPath(name: string): Promise<string> {
	return invoke<string>('local_edit_copy_path', { name });
}

/** Open a local file in the program the system uses for it. */
export async function localOpen(path: string): Promise<void> {
	return invoke('local_open', { path });
}

/** A text editor found on this computer: run `program`, then `args`, then
 *  the file; a terminal editor runs in a terminal tab. */
export interface EditorInfo {
	id: string;
	name: string;
	program: string;
	args: string[];
	terminal: boolean;
}

export async function localDetectEditors(): Promise<EditorInfo[]> {
	return invoke<EditorInfo[]>('local_detect_editors');
}

/** Start a graphical program on a local file. */
export async function localOpenWith(program: string, args: string[], path: string): Promise<void> {
	return invoke('local_open_with', { program, args, path });
}

/** Make an empty file `name` in `dir`; a name already taken is refused. */
export async function localTouch(dir: string, name: string): Promise<string> {
	return invoke<string>('local_touch', { dir, name });
}

/** Show a local file or folder in the system's file manager. */
export async function localReveal(path: string): Promise<void> {
	return invoke('local_reveal', { path });
}
