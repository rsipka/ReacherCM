<script lang="ts">
	/**
	 * A RemoteApp window of its own (seamless RemoteApp): the window this
	 * computer opened for one of the program's windows. It shows that
	 * window's part of the session's picture and sends mouse and keys to
	 * the session the tab holds.
	 */
	import RdpPanel from './RdpPanel.svelte';
	import type { RdpConnectParams } from '#lib/ipc/rdp.js';

	const query = new URLSearchParams(window.location.search);
	const tab = query.get('railview') ?? '';
	const win = Number(query.get('win'));
	// The panel connects nothing in a view; it needs only the session's id.
	const params = { host: '' } as RdpConnectParams;
</script>

<div class="rail-view">
	{#if tab && Number.isFinite(win)}
		<RdpPanel id={tab} {params} active railView={win} />
	{/if}
</div>

<style>
	.rail-view {
		position: fixed;
		inset: 0;
		background: #000;
	}
</style>
