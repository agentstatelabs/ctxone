<script lang="ts">
	import {
		listTaints,
		checkTaint,
		applyTaint,
		removeTaint
	} from '$lib/teamApi';
	import type { TaintRecord, TaintCheck } from '$lib/teamApi';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

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

	// Load on mount and re-load whenever the active namespace changes.
	$effect(() => {
		void namespaceStore.current;
		refresh();
	});

	const auto = useAutoRefresh(refresh);

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
	let applyAuthorizedAgents = $state('');
	let applySubmitting = $state(false);
	let applyError: string | null = $state(null);
	let applySuccess: string | null = $state(null);

	// Auto-suggest effect defaults when kind changes — quarantine wants
	// isolation, watch is observe-only, taint defaults to a soft warn.
	const KIND_DEFAULT_EFFECT: Record<TaintRecord['kind'], TaintRecord['effect']> = {
		taint: 'warn',
		quarantine: 'isolate',
		watch: 'advisory'
	};

	let lastKind: TaintRecord['kind'] = applyKind;
	$effect(() => {
		if (applyKind !== lastKind) {
			applyEffect = KIND_DEFAULT_EFFECT[applyKind];
			lastKind = applyKind;
		}
	});

	// Validation — surface issues inline before the user submits.
	let validation = $derived(validateApply({
		path: applyPath,
		name: applyName,
		agentId: applyAgentId,
		reason: applyReason,
		kind: applyKind,
		authorizedAgents: applyAuthorizedAgents
	}));

	function validateApply(v: {
		path: string;
		name: string;
		agentId: string;
		reason: string;
		kind: TaintRecord['kind'];
		authorizedAgents: string;
	}): { path?: string; name?: string; agentId?: string; reason?: string; authorizedAgents?: string } {
		const errs: Record<string, string> = {};
		const path = v.path.trim();
		if (!path) errs.path = 'Required';
		else if (!path.startsWith('/')) errs.path = 'Must start with /';
		else if (/\s/.test(path)) errs.path = 'No whitespace allowed';

		const name = v.name.trim();
		if (!name) errs.name = 'Required';
		else if (!/^[a-z0-9][a-z0-9_-]*$/.test(name))
			errs.name = 'lowercase letters, digits, _ or - (must start with letter/digit)';

		const agent = v.agentId.trim();
		if (!agent) errs.agentId = 'Required';
		else if (!/^[a-zA-Z0-9._:/-]+$/.test(agent)) errs.agentId = 'Invalid characters';

		if (!v.reason.trim()) errs.reason = 'Required';
		else if (v.reason.trim().length < 8) errs.reason = 'Be specific (≥8 chars)';

		if (v.kind === 'quarantine') {
			const list = v.authorizedAgents.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
			if (list.length === 0) {
				errs.authorizedAgents =
					'Quarantine without authorized agents blocks everyone. Add at least one, or pick kind=taint.';
			}
		}
		return errs;
	}

	let validationCount = $derived(Object.keys(validation).length);
	let conflictingTaints = $derived(
		applyPath.trim() ? taints.filter((t) => t.path === applyPath.trim()) : []
	);

	function summarizeApply(): string {
		const parts = [
			`Apply ${applyKind.toUpperCase()} (effect=${applyEffect}, severity=${applySeverity})`,
			`at ${applyPath.trim()}`,
			`as agent ${applyAgentId.trim()}`
		];
		return parts.join('\n');
	}

	async function submitApply(e: Event) {
		e.preventDefault();
		if (validationCount > 0) return;

		// Confirm destructive/strong effects explicitly. Soft effects skip the
		// extra dialog so the form stays low-friction for the common case.
		const strong = applyEffect === 'block' || applyEffect === 'isolate' || applySeverity === 'critical';
		if (strong) {
			const ok = confirm(
				`${summarizeApply()}\n\nThis can prevent writes by other agents. Proceed?`
			);
			if (!ok) return;
		} else if (conflictingTaints.length > 0) {
			const ok = confirm(
				`There are already ${conflictingTaints.length} taint(s) at ${applyPath.trim()}. Add another?`
			);
			if (!ok) return;
		}

		applySubmitting = true;
		applyError = null;
		applySuccess = null;
		try {
			const authorized =
				applyKind === 'quarantine'
					? applyAuthorizedAgents
							.split(/[\s,]+/)
							.map((s) => s.trim())
							.filter(Boolean)
					: undefined;
			const res = await applyTaint({
				path: applyPath.trim(),
				name: applyName.trim(),
				kind: applyKind,
				effect: applyEffect,
				severity: applySeverity,
				reason: applyReason.trim(),
				agent_id: applyAgentId.trim(),
				authorized_agents: authorized
			});
			applySuccess = `Taint applied: ${res.taint_id}`;
			applyPath = '';
			applyName = '';
			applyReason = '';
			applyAuthorizedAgents = '';
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
		const target = taints.find((t) => t.id === removeId);
		const label = target ? `${target.kind} on ${target.path}` : removeId;
		if (!confirm(`Remove ${label}?\n\nThis lifts the protection immediately.`)) return;
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

<h2>
	Taint / Quarantine / Watch
	<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
</h2>

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
					{#if validation.path && applyPath}
						<span class="field-error">{validation.path}</span>
					{/if}
				</div>
				<div class="form-field">
					<label for="ap-name">Name</label>
					<input id="ap-name" type="text" bind:value={applyName} placeholder="short identifier" required />
					{#if validation.name && applyName}
						<span class="field-error">{validation.name}</span>
					{/if}
				</div>
			</div>

			{#if conflictingTaints.length > 0}
				<p class="conflict-note">
					⚠ {conflictingTaints.length} existing taint(s) at this exact path:
					{conflictingTaints.map((t) => `${t.kind}/${t.name}`).join(', ')}
				</p>
			{/if}

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
					{#if validation.agentId && applyAgentId}
						<span class="field-error">{validation.agentId}</span>
					{/if}
				</div>
			</div>

			{#if applyKind === 'quarantine'}
				<div class="form-field">
					<label for="ap-authorized">Authorized agents</label>
					<input
						id="ap-authorized"
						type="text"
						bind:value={applyAuthorizedAgents}
						placeholder="comma- or space-separated agent ids allowed past the quarantine"
					/>
					{#if validation.authorizedAgents}
						<span class="field-error">{validation.authorizedAgents}</span>
					{/if}
				</div>
			{/if}

			<div class="form-field">
				<label for="ap-reason">Reason</label>
				<textarea id="ap-reason" bind:value={applyReason} rows="3" placeholder="Describe why this taint is being applied…" required></textarea>
				{#if validation.reason && applyReason}
					<span class="field-error">{validation.reason}</span>
				{/if}
			</div>

			<div class="form-actions">
				<button type="submit" class="btn-primary" disabled={applySubmitting || validationCount > 0}>
					{applySubmitting ? 'Applying…' : 'Apply Taint'}
				</button>
				{#if validationCount > 0}
					<span class="msg-dim">{validationCount} field(s) need attention</span>
				{/if}
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
		color: var(--text-0);
	}

	.panel {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
		margin-bottom: 1.5rem;
	}

	.panel h3 {
		margin: 0 0 1rem 0;
		font-size: 1rem;
		font-weight: 600;
		color: var(--text-0);
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
		color: var(--text-3);
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
		color: var(--text-3);
		font-weight: 600;
		padding: 0.4rem 0.6rem;
		border-bottom: 1px solid var(--border);
	}

	tbody td {
		padding: 0.55rem 0.6rem;
		border-bottom: 1px solid var(--border);
		color: var(--text-1);
		vertical-align: middle;
	}

	tbody tr:last-child td {
		border-bottom: none;
	}

	tbody tr.removing td {
		background: var(--bg-hover);
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
		background: var(--bg-2);
	}

	.remove-form {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		flex-wrap: wrap;
	}

	.remove-form input {
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
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

	.kind-taint      { background: color-mix(in srgb, var(--warning) 18%, transparent); color: var(--warning); }
	.kind-quarantine { background: color-mix(in srgb, var(--danger) 18%, transparent); color: var(--danger); }
	.kind-watch      { background: var(--accent-bg); color: var(--accent); }

	.effect-block    { background: color-mix(in srgb, var(--danger) 18%, transparent); color: var(--danger); }
	.effect-review   { background: color-mix(in srgb, var(--warning) 18%, transparent); color: var(--warning); }
	.effect-warn     { background: color-mix(in srgb, var(--warning) 14%, transparent); color: var(--warning); }
	.effect-isolate  { background: color-mix(in srgb, var(--info) 18%, transparent); color: var(--info); }
	.effect-advisory { background: var(--bg-hover); color: var(--text-2); }

	/* Severity text */
	.sev { font-size: 0.82rem; font-weight: 500; }
	.sev-low      { color: var(--text-2); }
	.sev-medium   { color: var(--warning); }
	.sev-high     { color: var(--warning); font-weight: 600; }
	.sev-critical { color: var(--danger); font-weight: 700; }

	/* Misc text */
	.mono { font-family: monospace; }
	.dim  { color: var(--text-3); }

	.empty {
		color: var(--text-3);
		padding: 1.5rem;
		text-align: center;
		font-size: 0.9rem;
	}

	.msg-error   { color: var(--danger); font-size: 0.85rem; margin: 0.25rem 0; }
	.msg-success { color: var(--success); font-size: 0.85rem; margin: 0.25rem 0; }
	.msg-dim     { color: var(--text-3); font-size: 0.85rem; }

	.field-error {
		color: var(--danger);
		font-size: 0.75rem;
		margin-top: 0.15rem;
	}

	.conflict-note {
		color: var(--warning);
		background: color-mix(in srgb, var(--warning) 10%, transparent);
		border: 1px solid color-mix(in srgb, var(--warning) 30%, transparent);
		border-radius: 4px;
		padding: 0.45rem 0.7rem;
		font-size: 0.8rem;
		margin: 0;
	}

	/* Buttons */
	.btn-primary {
		background: var(--accent-bg);
		border: 1px solid var(--accent-bg-hi);
		color: var(--accent);
		padding: 0.35rem 0.85rem;
		border-radius: 4px;
		font-size: 0.85rem;
		cursor: pointer;
	}

	.btn-primary:hover:not(:disabled) {
		background: var(--accent-bg-hi);
	}

	.btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

	.btn-secondary {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		padding: 0.35rem 0.75rem;
		border-radius: 4px;
		font-size: 0.85rem;
		cursor: pointer;
	}

	.btn-secondary:hover { color: var(--text-0); border-color: var(--text-3); }

	.btn-refresh {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		padding: 0.3rem 0.7rem;
		border-radius: 4px;
		font-size: 0.8rem;
		cursor: pointer;
	}

	.btn-refresh:hover:not(:disabled) { color: var(--text-0); border-color: var(--text-3); }
	.btn-refresh:disabled { opacity: 0.5; cursor: not-allowed; }

	.btn-remove {
		background: color-mix(in srgb, var(--danger) 18%, transparent);
		border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
		color: var(--danger);
		padding: 0.2rem 0.55rem;
		border-radius: 3px;
		font-size: 0.75rem;
		cursor: pointer;
	}

	.btn-remove:hover { background: color-mix(in srgb, var(--danger) 28%, transparent); }

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
		color: var(--text-3);
		margin-bottom: 0.1rem;
	}

	.check-form input[type='text'],
	.check-form input[type='range'] {
		width: 100%;
	}

	.check-form input[type='text'] {
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
		padding: 0.4rem 0.6rem;
		border-radius: 4px;
		font-family: monospace;
		font-size: 0.85rem;
		box-sizing: border-box;
	}

	.check-form input[type='range'] {
		accent-color: var(--accent);
	}

	.conf-val {
		font-family: monospace;
		color: var(--accent);
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

	.result-allow    { color: var(--success); font-size: 1.1rem; font-weight: 600; margin: 0; }
	.result-block    { color: var(--danger); font-size: 1.1rem; font-weight: 600; margin: 0; }
	.result-review   { color: var(--warning); font-size: 1.05rem; font-weight: 600; margin: 0; }
	.result-isolated { color: var(--info); font-size: 0.85rem; margin: 0; }
	.result-meta     { color: var(--text-2); font-size: 0.85rem; margin: 0; }

	.warnings { margin-top: 0.5rem; }

	.warnings-label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: var(--warning);
		margin: 0 0 0.3rem 0;
	}

	.warnings ul {
		margin: 0;
		padding-left: 1.25rem;
		font-size: 0.85rem;
		color: var(--text-1);
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
		color: var(--text-3);
	}

	.form-field input,
	.form-field select,
	.form-field textarea {
		background: var(--bg-0);
		border: 1px solid var(--border);
		color: var(--text-1);
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

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		font-weight: normal;
		margin-left: 0.75rem;
	}
</style>
