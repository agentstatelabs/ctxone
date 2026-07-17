<!--
	QuickCapture — the "remember a fact" form, restyled on the lens token
	system. Feature-identical to the old dashboard form: fact text,
	importance, optional context, POSTs /api/memory/remember and reports
	the saved path. Calls `onSaved` so the page can refresh its stats.
-->
<script lang="ts">
	import { remember } from '$lib/api';

	let { connected = true, onSaved }: { connected?: boolean; onSaved?: () => void } = $props();

	let factText = $state('');
	let factImportance = $state<'high' | 'medium' | 'low'>('medium');
	let factContext = $state('');
	let saving = $state(false);
	let saveMessage = $state<string | null>(null);
	let saveFailed = $state(false);

	async function handleSubmit(e: SubmitEvent) {
		e.preventDefault();
		if (!factText.trim()) return;
		saving = true;
		saveMessage = null;
		saveFailed = false;
		try {
			const result = await remember({
				fact: factText,
				importance: factImportance,
				context: factContext.trim() || undefined
			});
			saveMessage = `Saved: ${result.path}`;
			factText = '';
			factContext = '';
			onSaved?.();
		} catch (err) {
			saveFailed = true;
			saveMessage = err instanceof Error ? err.message : 'Save failed';
		} finally {
			saving = false;
		}
	}
</script>

<form class="capture" onsubmit={handleSubmit}>
	<textarea
		bind:value={factText}
		placeholder="e.g., We use BSL-1.1 for all projects"
		rows="3"
		disabled={saving || !connected}
	></textarea>
	<div class="capture-row">
		<select bind:value={factImportance} disabled={saving || !connected} aria-label="Importance">
			<option value="high">High</option>
			<option value="medium">Medium</option>
			<option value="low">Low</option>
		</select>
		<input
			type="text"
			bind:value={factContext}
			placeholder="context (e.g., licensing)"
			disabled={saving || !connected}
		/>
	</div>
	<button type="submit" disabled={saving || !connected || !factText.trim()}>
		{saving ? 'Saving…' : 'Remember'}
	</button>
	{#if !connected}
		<p class="capture-msg failed">Hub unreachable — capture is paused.</p>
	{:else if saveMessage}
		<p class="capture-msg" class:failed={saveFailed}>{saveMessage}</p>
	{/if}
</form>

<style>
	.capture {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-2);
	}

	.capture textarea,
	.capture input,
	.capture select {
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: var(--lens-space-2) var(--lens-space-3);
		font-size: var(--lens-font-size-sm);
		font-family: var(--lens-font-sans);
		box-sizing: border-box;
	}

	.capture textarea {
		width: 100%;
		resize: vertical;
	}

	.capture textarea:focus,
	.capture input:focus,
	.capture select:focus {
		outline: none;
		border-color: var(--lens-border-strong);
	}

	.capture-row {
		display: flex;
		gap: var(--lens-space-2);
	}

	.capture-row input {
		flex: 1;
		min-width: 0;
	}

	.capture button {
		align-self: flex-start;
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent-border, var(--lens-border));
		border-radius: var(--lens-radius-sm);
		color: var(--lens-accent-hover, var(--lens-accent));
		padding: var(--lens-space-2) var(--lens-space-4);
		font-size: var(--lens-font-size-sm);
		font-weight: 600;
		cursor: pointer;
		transition: border-color var(--lens-dur-fast, 120ms) var(--lens-ease, ease);
	}

	.capture button:hover:not(:disabled) {
		border-color: var(--lens-accent);
	}

	.capture button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.capture-msg {
		margin: 0;
		font-size: var(--lens-font-size-xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-ok, #4ade80);
		overflow-wrap: anywhere;
	}

	.capture-msg.failed {
		color: var(--lens-danger);
	}
</style>
