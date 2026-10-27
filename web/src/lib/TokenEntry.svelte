<script lang="ts">
	/**
	 * Token entry, shown over everything once the Hub refuses a call (see
	 * authStore). The token is checked with the Hub before it's kept, then
	 * the page reloads so every view fetches again with it.
	 */
	import { checkToken } from './api';
	import { authStore } from './authStore.svelte';

	let { onaccepted = () => location.reload() }: { onaccepted?: () => void } = $props();

	let token = $state('');
	let error = $state<string | null>(null);
	let checking = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		const candidate = token.trim();
		if (!candidate || checking) return;
		checking = true;
		error = null;
		const check = await checkToken(candidate);
		checking = false;
		if (check.ok) {
			authStore.accept(candidate, check.tenant);
			onaccepted();
		} else {
			error = check.error;
		}
	}
</script>

<div class="backdrop">
	<div class="modal" role="dialog" aria-modal="true" aria-labelledby="token-entry-title">
		<h3 id="token-entry-title">Token needed</h3>
		<p>
			This Hub needs a token before it shows any data. Paste the one you were given; Lens
			keeps it in this browser, for this Hub only.
		</p>
		<form onsubmit={submit}>
			<label>
				Token
				<!-- svelte-ignore a11y_autofocus -->
				<input
					type="password"
					bind:value={token}
					autocomplete="off"
					spellcheck="false"
					autofocus
					data-testid="token-input"
				/>
			</label>
			{#if error}
				<p class="error" role="alert" data-testid="token-error">{error}</p>
			{/if}
			<div class="actions">
				<button type="submit" class="primary" disabled={checking || !token.trim()}>
					{checking ? 'Checking…' : 'Continue'}
				</button>
			</div>
		</form>
	</div>
</div>

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 200;
		background: rgba(0, 0, 0, 0.7);
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.modal {
		background: var(--lens-overlay);
		border: 1px solid var(--lens-border-strong);
		border-radius: var(--lens-radius-md);
		box-shadow: var(--lens-shadow-lg);
		width: min(420px, 92vw);
		padding: var(--lens-space-4);
	}
	h3 {
		margin: 0 0 var(--lens-space-2);
		font-size: var(--lens-font-size-md);
		color: var(--lens-text-strong);
	}
	p {
		margin: 0 0 var(--lens-space-3);
		font-size: var(--lens-font-size-sm);
		color: var(--lens-text-secondary);
	}
	form {
		display: flex;
		flex-direction: column;
		gap: var(--lens-space-3);
	}
	label {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		font-size: var(--lens-font-size-2xs);
		text-transform: uppercase;
		letter-spacing: var(--lens-tracking-caps);
		color: var(--lens-muted);
	}
	input {
		background: var(--lens-bg);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: 0.4rem 0.55rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-sm);
		text-transform: none;
		letter-spacing: normal;
	}
	.error {
		margin: 0;
		color: var(--lens-danger);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
	}
	.actions button {
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-accent-hover);
		font-size: var(--lens-font-size-xs);
		padding: 0.35rem 0.8rem;
		cursor: pointer;
	}
	.actions button:hover:not(:disabled) {
		background: var(--lens-accent-surface-hi);
	}
	.actions button:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
</style>
