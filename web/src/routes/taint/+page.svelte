<script lang="ts">
	import { onMount } from 'svelte';
	import {
		listTaints,
		checkTaint,
		applyTaint,
		removeTaint
	} from '$lib/teamApi';
	import type { TaintRecord, TaintCheck } from '$lib/teamApi';

	// ── Active taints ────────────────────────────────────────────────────────
	let taints: TaintRecord[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	async function refresh() {
		loading = true;
		error = null;
		try {
			taints = await listTaints();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load taints';
		} finally {
			loading = false;
		}
	}

	onMount(refresh);

	// ── Taint check ──────────────────────────────────────────────────────────
	let checkPath = $state('');
	let checkAgentId = $state('');
	let checkConfidence = $state(0.95);
	let checkResult: TaintCheck | null = $state(null);
	let checkError: string | null = $state(null);
	let checkLoading = $state(false);

	async function runCheck() {
		if (!checkPath.trim() || !checkAgentId.trim()) return;
		checkLoading = true;
		checkError = null;
		checkResult = null;
		try {
			checkResult = await checkTaint(checkPath.trim(), checkAgentId.trim(), checkConfidence);
		} catch (e) {
			checkError = e instanceof Error ? e.message : 'Check failed';
		} finally {
			checkLoading = false;
		}
	}

	// ── Apply taint form ─────────────────────────────────────────────────────
	let applyOpen = $state(false);
	let applyPath = $state('');
	let applyName = $state('');
	let applyKind: TaintRecord['kind'] = $state('taint');
	let applyEffect: TaintRecord['effect'] = $state('warn');
	let applySeverity: TaintRecord['severity'] = $state('medium');
	let applyReason = $state('');
	let applyAgentId = $state('');
	let applySubmitting = $state(false);
	let applyError: string | null = $state(null);
	let applySuccess: string | null = $state(null);

	async function submitApply(e: Event) {
		e.preventDefault();
		if (!applyPath.trim() || !applyName.trim() || !applyReason.trim() || !applyAgentId.trim()) return;
		applySubmitting = true;
		applyError = null;
		applySuccess = null;
		try {
			const res = await applyTaint({
				path: applyPath.trim(),
				name: applyName.trim(),
				kind: applyKind,
				effect: applyEffect,
				severity: applySeverity,
				reason: applyReason.trim(),
				agent_id: applyAgentId.trim()
			});
			applySuccess = `Taint applied: ${res.taint_id}`;
			applyPath = '';
			applyName = '';
			applyReason = '';
			applyAgentId = '';
			await refresh();
		} catch (e) {
			applyError = e instanceof Error ? e.message : 'Failed to apply taint';
		} finally {
			applySubmitting = false;
		}
	}

	// ── Remove taint ─────────────────────────────────────────────────────────
	let removeId: string | null = $state(null);
	let removeReason = $state('');
	let removeAgentId = $state('');
	let removeError: string | null = $state(null);
	let removeSubmitting = $state(false);

	function startRemove(id: string) {
		removeId = id;
		removeReason = '';
		removeAgentId = '';
		removeError = null;
	}

	function cancelRemove() {
		removeId = null;
		removeReason = '';
		removeAgentId = '';
		removeError = null;
	}

	async function submitRemove(e: Event) {
		e.preventDefault();
		if (!removeId || !removeReason.trim() || !removeAgentId.trim()) return;
		removeSubmitting = true;
		removeError = null;
		try {
			await removeTaint(removeId, removeReason.trim(), removeAgentId.trim());
			removeId = null;
			removeReason = '';
			removeAgentId = '';
			await refresh();
		} catch (e) {
			removeError = e instanceof Error ? e.message : 'Failed to remove taint';
		} finally {
			removeSubmitting = false;
		}
	}

	// ── Helpers ──────────────────────────────────────────────────────────────
	function fmtDate(iso: string): string {
		return iso.slice(0, 16).replace('T', ' ');
	}
</script>

<h2>Taint / Quarantine / Watch</h2>

<!-- ── Panel 1: Active Taints ──────────────────────────────────────────────── -->
<section class="panel">
	<div class="panel-header">
		<h3>Active Taints</h3>
		<button class="btn-refresh" onclick={refresh} disabled={loading}>
			{loading ? 'Loading…' : 'Refresh'}
		</button>
	</div>

	{#if error}
		<p class="msg-error">{error}</p>
	{/if}

	{#if loading && taints.length === 0}
		<p class="msg-dim">Loading taints…</p>
	{:else if taints.length === 0}
		<p class="empty">No active taints</p>
	{:else}
		<div class="table-wrap">
			<table>
				<thead>
					<tr>
						<th>Path</th>
						<th>Name</th>
						<th>Kind</th>
						<th>Effect</th>
						<th>Severity</th>
						<th>Agent</th>
						<th>Created</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each taints as t}
						<tr class:removing={removeId === t.id}>
							<td class="mono path-cell" title={t.path}>{t.path}</td>
							<td class="mono">{t.name}</td>
							<td>
								<span class="badge kind-{t.kind}">{t.kind}</span>
							</td>
							<td>
								<span class="badge effect-{t.effect}">{t.effect}</span>
							</td>
							<td>
								<span class="sev sev-{t.severity}">{t.severity}</span>
							</td>
							<td class="mono dim">{t.agent_id}</td>
							<td class="mono dim">{fmtDate(t.created_at)}</td>
							<td>
								{#if removeId !== t.id}
									<button class="btn-remove" onclick={() => startRemove(t.id)}>Remove</button>
								{/if}
							</td>
						</tr>
						{#if removeId === t.id}
							<tr class="remove-row">
								<td colspan="8">
									<form class="remove-form" onsubmit={submitRemove}>
										<input
											type="text"
											bind:value={removeAgentId}
											placeholder="Agent ID"
											required
										/>
										<input
											type="text"
											bind:value={removeReason}
											placeholder="Reason for removal"
											required
										/>
										<button type="submit" class="btn-primary" disabled={removeSubmitting}>
											{removeSubmitting ? 'Removing…' : 'Confirm Remove'}
										</button>
										<button type="button" class="btn-secondary" onclick={cancelRemove}>Cancel</button>
										{#if removeError}
											<span class="msg-error">{removeError}</span>
										{/if}
									</form>
								</td>
							</tr>
						{/if}
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<!-- ── Panel 2: Taint Check ───────────────────────────────────────────────── -->
<div class="two-col">
	<section class="panel">
		<h3>Taint Check</h3>
		<form class="check-form" onsubmit={(e) => { e.preventDefault(); runCheck(); }}>
			<label for="check-path">Path</label>
			<input
				id="check-path"
				type="text"
				bind:value={checkPath}
				placeholder="/ctx/..."
			/>

			<label for="check-agent">Agent ID</label>
			<input
				id="check-agent"
				type="text"
				bind:value={checkAgentId}
				placeholder="agent-id"
			/>

			<label for="check-conf">
				Confidence <span class="conf-val">{checkConfidence.toFixed(2)}</span>
			</label>
			<input
				id="check-conf"
				type="range"
				min="0"
				max="1"
				step="0.05"
				bind:value={checkConfidence}
			/>

			<button type="submit" class="btn-primary" disabled={checkLoading}>
				{checkLoading ? 'Checking…' : 'Check'}
			</button>
		</form>
	</section>

	<section class="panel">
		<h3>Check Result</h3>
		{#if checkError}
			<p class="msg-error">{checkError}</p>
		{:else if checkResult === null}
			<p class="empty">Run a check to see results</p>
		{:else}
			<div class="check-result">
				{#if checkResult.can_write}
					<p class="result-allow">&#x2713; Write allowed</p>
				{:else if checkResult.effect === 'review' || checkResult.effect === 'warn'}
					<p class="result-review">
						&#x26a0; Review required
						{#if checkResult.required_confidence !== undefined}
							(min confidence: {checkResult.required_confidence.toFixed(2)})
						{/if}
					</p>
				{:else}
					<p class="result-block">&#x2715; Blocked</p>
				{/if}

				{#if checkResult.isolated}
					<p class="result-isolated">Isolated write path active</p>
				{/if}

				{#if checkResult.effect}
					<p class="result-meta">Effect: <span class="mono">{checkResult.effect}</span></p>
				{/if}

				{#if checkResult.matching_taint_id}
					<p class="result-meta">Matched taint: <span class="mono">{checkResult.matching_taint_id}</span></p>
				{/if}

				{#if checkResult.warnings.length > 0}
					<div class="warnings">
						<p class="warnings-label">Warnings</p>
						<ul>
							{#each checkResult.warnings as w}
								<li>{w}</li>
							{/each}
						</ul>
					</div>
				{/if}
			</div>
		{/if}
	</section>
</div>

<!-- ── Panel 3: Apply Taint (collapsible) ────────────────────────────────── -->
<section class="panel">
	<button
		class="collapsible-header"
		type="button"
		onclick={() => (applyOpen = !applyOpen)}
		aria-expanded={applyOpen}
	>
		<h3>Apply Taint</h3>
		<span class="chevron">{applyOpen ? '▲' : '▼'}</span>
	</button>

	{#if applyOpen}
		<form class="apply-form" onsubmit={submitApply}>
			<div class="form-row">
				<div class="form-field">
					<label for="ap-path">Path</label>
					<input id="ap-path" type="text" bind:value={applyPath} placeholder="/ctx/..." required />
				</div>
				<div class="form-field">
					<label for="ap-name">Name</label>
					<input id="ap-name" type="text" bind:value={applyName} placeholder="short identifier" required />
				</div>
			</div>

			<div class="form-row">
				<div class="form-field">
					<label for="ap-kind">Kind</label>
					<select id="ap-kind" bind:value={applyKind}>
						<option value="taint">taint</option>
						<option value="quarantine">quarantine</option>
						<option value="watch">watch</option>
					</select>
				</div>
				<div class="form-field">
					<label for="ap-effect">Effect</label>
					<select id="ap-effect" bind:value={applyEffect}>
						<option value="warn">warn</option>
						<option value="block">block</option>
						<option value="review">review</option>
						<option value="isolate">isolate</option>
						<option value="advisory">advisory</option>
					</select>
				</div>
				<div class="form-field">
					<label for="ap-sev">Severity</label>
					<select id="ap-sev" bind:value={applySeverity}>
						<option value="low">low</option>
						<option value="medium">medium</option>
						<option value="high">high</option>
						<option value="critical">critical</option>
					</select>
				</div>
			</div>

			<div class="form-row">
				<div class="form-field">
					<label for="ap-agent">Agent ID</label>
					<input id="ap-agent" type="text" bind:value={applyAgentId} placeholder="agent-id" required />
				</div>
			</div>

			<div class="form-field">
				<label for="ap-reason">Reason</label>
				<textarea id="ap-reason" bind:value={applyReason} rows="3" placeholder="Describe why this taint is being applied…" required></textarea>
			</div>

			<div class="form-actions">
				<button type="submit" class="btn-primary" disabled={applySubmitting}>
					{applySubmitting ? 'Applying…' : 'Apply Taint'}
				</button>
				{#if applyError}
					<span class="msg-error">{applyError}</span>
				{/if}
				{#if applySuccess}
					<span class="msg-success">{applySuccess}</span>
				{/if}
			</div>
		</form>
	{/if}
</section>

<style>
	h2 {
		margin: 0 0 1.5rem 0;
		font-size: 1.4rem;
		font-weight: 600;
		color: #fff;
	}

	.panel {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 1.5rem;
	}

	.panel h3 {
		margin: 0 0 1rem 0;
		font-size: 1rem;
		font-weight: 600;
		color: #fff;
	}

	.panel-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 1rem;
	}

	.panel-header h3 {
		margin: 0;
	}

	/* Collapsible */
	.collapsible-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		width: 100%;
		background: none;
		border: none;
		padding: 0;
		cursor: pointer;
		color: inherit;
		margin-bottom: 0;
	}

	.collapsible-header h3 {
		margin: 0;
	}

	.collapsible-header[aria-expanded='true'] {
		margin-bottom: 1rem;
	}

	.chevron {
		color: #555;
		font-size: 0.75rem;
	}

	/* Two-column layout */
	.two-col {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1.5rem;
		margin-bottom: 1.5rem;
	}

	.two-col .panel {
		margin-bottom: 0;
	}

	/* Table */
	.table-wrap {
		overflow-x: auto;
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.85rem;
	}

	thead th {
		text-align: left;
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: #555;
		font-weight: 600;
		padding: 0.4rem 0.6rem;
		border-bottom: 1px solid #222;
	}

	tbody td {
		padding: 0.55rem 0.6rem;
		border-bottom: 1px solid #1a1a1a;
		color: #ccc;
		vertical-align: middle;
	}

	tbody tr:last-child td {
		border-bottom: none;
	}

	tbody tr.removing td {
		background: #1a0a0a;
	}

	.path-cell {
		max-width: 200px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	/* Remove inline form */
	.remove-row td {
		padding: 0.5rem 0.6rem;
		background: #150a0a;
	}

	.remove-form {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		flex-wrap: wrap;
	}

	.remove-form input {
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
		padding: 0.3rem 0.6rem;
		border-radius: 4px;
		font-size: 0.82rem;
		min-width: 140px;
	}

	/* Badges — kind */
	.badge {
		font-size: 0.7rem;
		padding: 0.1rem 0.45rem;
		border-radius: 3px;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		font-weight: 600;
		display: inline-block;
	}

	.kind-taint     { background: #431407; color: #fb923c; }
	.kind-quarantine { background: #3b0000; color: #f87171; }
	.kind-watch     { background: #1e3a5f; color: #93c5fd; }

	.effect-block   { background: #3b0000; color: #f87171; }
	.effect-review  { background: #422006; color: #fcd34d; }
	.effect-warn    { background: #3d2a00; color: #fcd34d; }
	.effect-isolate { background: #2e1a4a; color: #c4b5fd; }
	.effect-advisory { background: #1a1a1a; color: #888; }

	/* Severity text */
	.sev { font-size: 0.82rem; font-weight: 500; }
	.sev-low      { color: #888; }
	.sev-medium   { color: #fcd34d; }
	.sev-high     { color: #fb923c; }
	.sev-critical { color: #ef4444; font-weight: 700; }

	/* Misc text */
	.mono { font-family: monospace; }
	.dim  { color: #666; }

	.empty {
		color: #555;
		padding: 1.5rem;
		text-align: center;
		font-size: 0.9rem;
	}

	.msg-error   { color: #ef4444; font-size: 0.85rem; margin: 0.25rem 0; }
	.msg-success { color: #22c55e; font-size: 0.85rem; margin: 0.25rem 0; }
	.msg-dim     { color: #555; font-size: 0.9rem; }

	/* Buttons */
	.btn-primary {
		background: #1e3a5f;
		border: 1px solid #2a4a7a;
		color: #93c5fd;
		padding: 0.35rem 0.85rem;
		border-radius: 4px;
		font-size: 0.85rem;
		cursor: pointer;
	}

	.btn-primary:hover:not(:disabled) {
		background: #243f6a;
		border-color: #3a5a8a;
	}

	.btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

	.btn-secondary {
		background: #1a1a1a;
		border: 1px solid #333;
		color: #888;
		padding: 0.35rem 0.75rem;
		border-radius: 4px;
		font-size: 0.85rem;
		cursor: pointer;
	}

	.btn-secondary:hover { color: #fff; border-color: #555; }

	.btn-refresh {
		background: #1a1a1a;
		border: 1px solid #333;
		color: #888;
		padding: 0.3rem 0.7rem;
		border-radius: 4px;
		font-size: 0.8rem;
		cursor: pointer;
	}

	.btn-refresh:hover:not(:disabled) { color: #fff; border-color: #555; }
	.btn-refresh:disabled { opacity: 0.5; cursor: not-allowed; }

	.btn-remove {
		background: #3b0000;
		border: 1px solid #5a0000;
		color: #f87171;
		padding: 0.2rem 0.55rem;
		border-radius: 3px;
		font-size: 0.75rem;
		cursor: pointer;
	}

	.btn-remove:hover { background: #4a0000; }

	/* Check form */
	.check-form {
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
	}

	.check-form label {
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: #555;
		margin-bottom: 0.1rem;
	}

	.check-form input[type='text'],
	.check-form input[type='range'] {
		width: 100%;
	}

	.check-form input[type='text'] {
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.85rem;
		box-sizing: border-box;
	}

	.check-form input[type='range'] {
		accent-color: #3b82f6;
	}

	.conf-val {
		font-family: monospace;
		color: #93c5fd;
		font-size: 0.85rem;
		margin-left: 0.4rem;
		text-transform: none;
		letter-spacing: 0;
	}

	/* Check result */
	.check-result {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.result-allow   { color: #22c55e; font-size: 1.1rem; font-weight: 600; margin: 0; }
	.result-block   { color: #ef4444; font-size: 1.1rem; font-weight: 600; margin: 0; }
	.result-review  { color: #fcd34d; font-size: 1.05rem; font-weight: 600; margin: 0; }
	.result-isolated { color: #c4b5fd; font-size: 0.85rem; margin: 0; }
	.result-meta    { color: #888; font-size: 0.85rem; margin: 0; }

	.warnings { margin-top: 0.5rem; }

	.warnings-label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: #fcd34d;
		margin: 0 0 0.3rem 0;
	}

	.warnings ul {
		margin: 0;
		padding-left: 1.25rem;
		font-size: 0.85rem;
		color: #ccc;
	}

	.warnings li { margin-bottom: 0.2rem; }

	/* Apply form */
	.apply-form {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}

	.form-row {
		display: flex;
		gap: 1rem;
	}

	.form-field {
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
		flex: 1;
	}

	.form-field label {
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: #555;
	}

	.form-field input,
	.form-field select,
	.form-field textarea {
		background: #0a0a0a;
		border: 1px solid #333;
		color: #e0e0e0;
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.85rem;
		width: 100%;
		box-sizing: border-box;
	}

	.form-field textarea {
		resize: vertical;
		font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
	}

	.form-field select {
		appearance: auto;
	}

	.form-actions {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		flex-wrap: wrap;
	}
</style>
