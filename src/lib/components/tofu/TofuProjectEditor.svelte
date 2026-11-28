<script lang="ts">
	import type { TofuProject } from '#lib/ipc/tofu.js';
	import { createProject } from '#lib/state/tofu.svelte.js';
	import { t } from '#lib/state/i18n.svelte.js';
	import { open } from '@tauri-apps/plugin-dialog';
	import Modal from '#lib/components/shared/Modal.svelte';
	import Input from '#lib/components/shared/Input.svelte';
	import Button from '#lib/components/shared/Button.svelte';

	interface Props {
		/** Drawn in place (New session's DevOps and Databases tabs), not as a dialog. */
		inline?: boolean;
		open: boolean;
		templateId?: string | null;
		onclose: () => void;
		oncreate: (project: TofuProject) => void;
	}

	let { open: isOpen, templateId = null, inline = false, onclose, oncreate }: Props = $props();

	let name = $state('');
	let path = $state('');
	let description = $state('');
	let loading = $state(false);

	let canSubmit = $derived(name.trim().length > 0 && path.trim().length > 0 && !loading);

	function resetForm() {
		name = '';
		path = '';
		description = '';
		loading = false;
	}

	function handleClose() {
		resetForm();
		onclose();
	}

	async function browsePath() {
		try {
			const selected = await open({ directory: true });
			if (selected) {
				path = selected as string;
			}
		} catch {
			// User cancelled the dialog
		}
	}

	async function handleSubmit() {
		if (!canSubmit) return;

		loading = true;
		try {
			const project = await createProject(name.trim(), path.trim(), description.trim());
			resetForm();
			oncreate(project);
		} catch {
			loading = false;
		}
	}
</script>

{#snippet body()}
	<form class="form" onsubmit={handleSubmit}>
		<Input label={t('tofu.project_name')} bind:value={name} placeholder={t('tofu.project_name')} />

		<div class="path-row">
			<div class="path-input">
				<Input label={t('tofu.project_path')} bind:value={path} placeholder={t('tofu.project_path')} />
			</div>
			<Button variant="secondary" size="sm" onclick={browsePath}>
				{t('tofu.browse')}
			</Button>
		</div>

		<Input
			label={t('tofu.project_description')}
			bind:value={description}
			placeholder={t('tofu.project_description')}
		/>
	</form>

{/snippet}

{#snippet acts()}
	{#if !inline}
		<Button variant="secondary" size="sm" onclick={handleClose} disabled={loading}>
			{t('tofu.cancel')}
		</Button>
	{/if}
	<Button variant="primary" size="sm" onclick={handleSubmit} disabled={!canSubmit}>
		{loading ? t('tofu.creating') : t('tofu.create')}
	</Button>
{/snippet}

{#if inline}
	<div class="inline-editor">
		{@render body()}
		<div class="inline-actions">{@render acts()}</div>
	</div>
{:else}
<Modal open={isOpen} onclose={handleClose} title={t('tofu.new_project')}>
	{@render body()}
	{#snippet actions()}{@render acts()}{/snippet}
</Modal>
{/if}

<style>
	.inline-actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 8px;
		margin-top: 16px;
	}

	.form {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.path-row {
		display: flex;
		align-items: flex-end;
		gap: 8px;
	}

	.path-input {
		flex: 1;
	}
</style>
