/**
 * The Kubernetes workspace's shared state: actions in flight, so Settings
 * waits for them before switching the tool off.
 */

let inFlight = $state(0);

/** Something is changing on a cluster right now (a rollback, a scale). */
export function isBusy(): boolean {
	return inFlight > 0;
}

/** Run `work` counted as in flight. */
export async function busy<T>(work: () => Promise<T>): Promise<T> {
	inFlight += 1;
	try {
		return await work();
	} finally {
		inFlight -= 1;
	}
}

let clustersVersion = $state(0);

/** Bumped when a cluster is saved or deleted, so lists of them read again
 *  (the Sessions list's Kubernetes group). */
export function getClustersVersion(): number {
	return clustersVersion;
}

export function clustersChanged(): void {
	clustersVersion += 1;
}
