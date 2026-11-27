export function syncAccountErrorKey(cause: unknown): string {
	return typeof cause === 'string' && cause.startsWith('sync_account.')
		? cause
		: 'sync_account.error_server';
}
