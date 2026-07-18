<script lang="ts">
	import { hubFetch } from '$lib/api';
	import { formatCompact } from '@agentstate/lens-core';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';

	interface Session {
		session_id: string;
		/** Human-readable title (server-derived from the first user turn).
		 * Optional — absent on older Hubs; we fall back to the id. */
		name?: string | null;
		session_tokens_used: number;
		session_tokens_saved: number;
		total_graph_size_tokens: number;
		cumulative_ratio: number;
		llm_input_tokens: number;
		llm_output_tokens: number;
		llm_cache_read_tokens: number;
		llm_call_count: number;
		last_model: string | null;
		/** Every model the session used (server; t-022). Optional — older
		 * hubs omit it, so we fall back to last_model + derived first-turn. */
		models_used?: string[];
		last_provider: string | null;
		/** Optional on newer hubs — agent/tool origin. Absent on older ones. */
		source?: string | null;
		/** Optional ISO timestamps on newer hubs. Absent → derived client-side. */
		started_at?: string | null;
		updated_at?: string | null;
	}

	interface MemoryCommit {
		id: string;
		timestamp: string;
		agent_id: string;
		intent: { description: string; tags: string[] };
	}

	// Commits tagged with the session but that are plumbing, not memories:
	// transcript turn captures + the session title/meta nodes. These would
	// otherwise flood the Memories list (they ARE the transcript, shown in
	// the Conversation panel).
	const CAPTURE_KINDS = new Set(['full-turn', 'session-title', 'session-meta']);
	function isCapture(c: MemoryCommit): boolean {
		return (c.intent.tags ?? []).some(
			(t) => t.startsWith('kind:') && CAPTURE_KINDS.has(t.slice(5))
		);
	}
	function memPath(c: MemoryCommit): string | null {
		// If the memory tags carry its path, we can deep-link into Browse.
		return (c.intent.tags ?? []).find((t) => t.startsWith('/'))?.trim() ?? null;
	}
	let openMemory: MemoryCommit | null = $state(null);

	let sessions: Session[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);
	let selected: Session | null = $state(null);
	let memories: MemoryCommit[] = $state([]);
	let memoriesLoading = $state(false);

	interface Turn {
		key: string;
		turn_index?: number;
		timestamp?: string;
		model?: string;
		user_text?: string;
		assistant_text?: string;
		tool_calls?: string[];
		tool_calls_raw?: unknown[];
		tokens?: { input?: number; output?: number; cache_read?: number; cache_creation?: number };
	}
	let turns: Turn[] = $state([]);
	let turnsLoading = $state(false);
	let turnsError: string | null = $state(null);
	let expandedTools: Record<string, boolean> = $state({});

	$effect(() => {
		if (selected) {
			loadMemories(selected.session_id);
			loadTurns(selected.session_id);
		} else {
			memories = [];
			turns = [];
		}
	});

	async function loadTurns(sessionId: string) {
		turnsLoading = true;
		turnsError = null;
		turns = [];
		expandedTools = {};
		turnSearch = ''; // reset within-session search on session change
		try {
			// One subtree fetch returns every turn for the session.
			const r = await hubFetch(
				`/api/state/main?path=/sessions/${encodeURIComponent(sessionId)}/turns`
			);
			if (r.status === 404) {
				turns = []; // session predates turn capture
				return;
			}
			if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
			const tree = await r.json();
			if (tree && typeof tree === 'object') {
				turns = Object.entries(tree as Record<string, Turn>)
					.map(([key, v]) => ({ ...v, key }))
					.sort((a, b) => (a.turn_index ?? 0) - (b.turn_index ?? 0));
			}
		} catch (e) {
			turnsError = e instanceof Error ? e.message : String(e);
		} finally {
			turnsLoading = false;
		}
	}

	async function loadMemories(sessionId: string) {
		memoriesLoading = true;
		try {
			const r = await hubFetch('/api/log/main?limit=500');
			if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
			const all: MemoryCommit[] = await r.json();
			const tag = `session:${sessionId}`;
			// Real memories only — drop transcript-capture / title / meta commits.
			memories = all.filter((c) => c.intent.tags?.includes(tag) && !isCapture(c));
		} catch {
			memories = [];
		} finally {
			memoriesLoading = false;
		}
	}

	// Client-derived per-session metadata: for GUID sessions the server hasn't
	// named/dated yet, fetch the first turn once and cache {title, date, model}.
	// The whole t0000 node carries user_text (→ title), timestamp (→ date), and
	// model. Cached by session id and only fetched for sessions not yet seen —
	// the 15s auto-refresh reuses the cache and only derives brand-new ids.
	// Superseded by server `name`/`started_at` fields once they land on the hub.
	interface DerivedMeta {
		title?: string;
		/** epoch ms of the first turn, or undefined when no t0000 timestamp. */
		date?: number;
		model?: string;
	}
	let derivedMeta: Record<string, DerivedMeta> = $state({});

	// Sync = re-scan this machine's Claude Code transcripts into the hub
	// (turns, titles, token metrics). Runs the local CLI via a hub endpoint;
	// only works when the hub is co-located with ~/.claude/projects.
	let syncing = $state(false);
	let syncMsg: string | null = $state(null);
	let syncErr = $state(false);

	async function syncSessions() {
		syncing = true;
		syncMsg = null;
		syncErr = false;
		try {
			const r = await hubFetch('/api/sessions/sync', { method: 'POST' });
			if (r.status === 404) {
				syncErr = true;
				syncMsg = 'Sync not available on this Hub version.';
				return;
			}
			if (!r.ok) {
				syncErr = true;
				syncMsg = `Sync failed: ${(await r.text()) || r.statusText}`;
				return;
			}
			const res = await r.json();
			syncMsg = `Synced ${res.sessions ?? '?'} sessions · ${fmt(res.tokens ?? 0)} tokens`;
			derivedMeta = {}; // titles/dates may now come from the server
			await load();
		} catch (e) {
			syncErr = true;
			syncMsg = `Sync failed: ${e instanceof Error ? e.message : String(e)}`;
		} finally {
			syncing = false;
		}
	}

	async function load() {
		loading = true;
		error = null;
		try {
			const r = await hubFetch('/api/stats/sessions');
			if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
			sessions = await r.json();
			sessions.sort((a, b) => b.session_tokens_used - a.session_tokens_used);
			void deriveListNames();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	async function deriveListNames() {
		// Only sessions we haven't derived yet (cache by id). We still derive
		// dates for server-named sessions, so the gate is purely "not seen".
		// The auto-refresh reuses this cache and only fetches genuinely-new ids.
		const todo = sessions.filter((s) => !(s.session_id in derivedMeta));
		const CONCURRENCY = 8;
		for (let i = 0; i < todo.length; i += CONCURRENCY) {
			await Promise.all(
				todo.slice(i, i + CONCURRENCY).map(async (s) => {
					// Mark as attempted up-front so a 404 (no t0000) still counts as
					// "seen" and we never refetch it on every tick.
					const meta: DerivedMeta = {};
					try {
						// Fast path: the first turn is usually t0000. If it isn't
						// (partial snapshots can start mid-session, e.g. t0043),
						// fall back to the turns subtree and take the real first
						// key — so title/date/model are never silently missing.
						const base = `/api/state/main?path=/sessions/${encodeURIComponent(s.session_id)}/turns`;
						let node: Turn | null = null;
						const r0 = await hubFetch(`${base}/t0000`);
						if (r0.ok) {
							const j = await r0.json();
							if (j && typeof j === 'object') node = j as Turn;
						}
						if (!node) {
							const rAll = await hubFetch(base);
							if (rAll.ok) {
								const tree = await rAll.json();
								if (tree && typeof tree === 'object') {
									const keys = Object.keys(tree).sort();
									if (keys.length) node = tree[keys[0]] as Turn;
								}
							}
						}
						if (node && typeof node === 'object') {
							const ut = node.user_text;
							if (typeof ut === 'string' && ut.trim()) meta.title = truncate(ut.trim(), 64);
							const ts = node.timestamp;
							if (typeof ts === 'string' && ts.trim()) {
								const ms = Date.parse(ts);
								if (!Number.isNaN(ms)) meta.date = ms;
							}
							const md = node.model;
							if (typeof md === 'string' && md.trim()) meta.model = md.trim();
						}
					} catch {
						/* leave meta empty — session keeps its id label, no date */
					}
					derivedMeta[s.session_id] = meta;
				})
			);
		}
	}

	$effect(() => {
		// Re-load whenever the active namespace changes
		void namespaceStore.current;
		selected = null;
		load();
	});

	const auto = useAutoRefresh(async () => {
		await load();
		if (selected) await loadMemories(selected.session_id);
	});

	function ratioColor(r: number): string {
		if (r >= 5) return 'var(--success)';
		if (r >= 2) return 'var(--success)';
		if (r >= 1.2) return 'var(--accent)';
		return 'var(--text-2)';
	}

	// Compact display (12.4K / 17.5M / 1.2B) — exact value goes in the
	// title attribute so precision is a hover away.
	function fmt(n: number): string {
		return formatCompact(n ?? 0);
	}
	function exact(n: number): string {
		return (n ?? 0).toLocaleString();
	}

	// A session that carries a name (server, or a client-derived first-turn
	// title) gets a human label; otherwise the id stands in.
	function truncate(s: string, n: number): string {
		return s.length > n ? s.slice(0, n - 1) + '…' : s;
	}
	function listLabel(s: Session): string {
		return s.name?.trim() || derivedMeta[s.session_id]?.title || s.session_id;
	}
	function hasDistinctName(s: Session): boolean {
		return !!(s.name?.trim() || derivedMeta[s.session_id]?.title);
	}
	// Detail title: server name > first user message (turns already loaded) > id.
	const detailTitle: string = $derived.by(() => {
		const sel = selected;
		if (!sel) return '';
		if (sel.name?.trim()) return sel.name.trim();
		const firstUser = turns.find((t) => t.user_text?.trim())?.user_text?.trim();
		if (firstUser) return truncate(firstUser, 80);
		return sel.session_id;
	});

	// ── Agent-type derivation ──────────────────────────────────────────────
	// Prefer a server `source` when present; otherwise heuristic from id/name.
	function mapSource(src: string): string {
		const s = src.toLowerCase();
		if (s.includes('codex')) return 'Codex';
		if (s.includes('cursor')) return 'Cursor';
		if (s.includes('copilot')) return 'Copilot';
		if (s.includes('claude')) return 'Claude Code';
		// Unknown but present source — surface it verbatim (title-cased-ish).
		return src;
	}
	function agentType(s: Session): string {
		if (s.source?.trim()) return mapSource(s.source.trim());
		const hay = `${s.session_id} ${s.name ?? ''}`.toLowerCase();
		if (hay.includes('codex')) return 'Codex';
		if (hay.includes('cursor')) return 'Cursor';
		if (hay.includes('copilot')) return 'Copilot';
		return 'Claude Code'; // GUID / claude default
	}

	// ── Per-session date ───────────────────────────────────────────────────
	// Server timestamps win when present; else the derived first-turn date.
	// Undefined → undated (sinks to the bottom of any sort).
	function sessionDate(s: Session): number | undefined {
		const srv = s.updated_at ?? s.started_at;
		if (srv) {
			const ms = Date.parse(srv);
			if (!Number.isNaN(ms)) return ms;
		}
		return derivedMeta[s.session_id]?.date;
	}
	function sessionModel(s: Session): string | null {
		return s.last_model ?? derivedMeta[s.session_id]?.model ?? null;
	}
	// Every model a session touched: server models_used when present, else
	// the union of last_model + the derived first-turn model. Lets the model
	// filter match a session by ANY model it used, not just its last one.
	function sessionModels(s: Session): string[] {
		if (s.models_used?.length) return s.models_used;
		const set = new Set<string>();
		if (s.last_model) set.add(s.last_model);
		const dm = derivedMeta[s.session_id]?.model;
		if (dm) set.add(dm);
		return [...set];
	}
	function shortDate(ms: number, now = Date.now()): string {
		const diff = now - ms;
		const day = 86_400_000;
		if (diff < 0) return 'now';
		if (diff < 60_000) return 'just now';
		if (diff < 3_600_000) return `${Math.round(diff / 60_000)}m ago`;
		if (diff < day) return `${Math.round(diff / 3_600_000)}h ago`;
		if (diff < 7 * day) return `${Math.round(diff / day)}d ago`;
		const d = new Date(ms);
		const sameYear = new Date(now).getFullYear() === d.getFullYear();
		return d.toLocaleDateString(undefined, {
			month: 'short',
			day: 'numeric',
			...(sameYear ? {} : { year: 'numeric' })
		});
	}

	// ── Toolbar: search / sort / filter (persisted to localStorage) ─────────
	type SortKey = 'date' | 'used' | 'saved' | 'ratio' | 'name';
	type SortDir = 'asc' | 'desc';
	const SORT_LABELS: Record<SortKey, string> = {
		date: 'Date',
		used: 'Tokens used',
		saved: 'Tokens saved',
		ratio: 'Ratio',
		name: 'Name'
	};
	const LS_KEY = 'ctxone:sessions:toolbar';

	let searchInput = $state(''); // raw box value
	let searchQuery = $state(''); // debounced, drives filtering
	let sortKey: SortKey = $state('used'); // default: tokens used desc (unchanged)
	let sortDir: SortDir = $state('desc');
	let agentFilter: string[] = $state([]); // empty = all
	let modelFilter: string[] = $state([]); // empty = all
	const PAGE_SIZE = 30;
	let visibleCount = $state(PAGE_SIZE);

	// Restore persisted sort + filter choices once, on mount.
	$effect(() => {
		if (typeof localStorage === 'undefined') return;
		try {
			const raw = localStorage.getItem(LS_KEY);
			if (!raw) return;
			const p = JSON.parse(raw);
			if (p.sortKey in SORT_LABELS) sortKey = p.sortKey;
			if (p.sortDir === 'asc' || p.sortDir === 'desc') sortDir = p.sortDir;
			if (Array.isArray(p.agentFilter)) agentFilter = p.agentFilter;
			if (Array.isArray(p.modelFilter)) modelFilter = p.modelFilter;
		} catch {
			/* ignore malformed persisted state */
		}
	});
	// Persist on change.
	$effect(() => {
		const snapshot = JSON.stringify({ sortKey, sortDir, agentFilter, modelFilter });
		if (typeof localStorage !== 'undefined') localStorage.setItem(LS_KEY, snapshot);
	});
	// Debounce the search box → searchQuery (250ms).
	$effect(() => {
		const v = searchInput;
		const id = setTimeout(() => (searchQuery = v), 250);
		return () => clearTimeout(id);
	});
	// Any filter/sort/search change resets paging to the first page.
	$effect(() => {
		void [searchQuery, sortKey, sortDir, agentFilter, modelFilter];
		visibleCount = PAGE_SIZE;
	});

	// Distinct filter options derived from the loaded set.
	const agentOptions: string[] = $derived([...new Set(sessions.map(agentType))].sort());
	const modelOptions: string[] = $derived(
		[...new Set(sessions.flatMap(sessionModels).filter((m): m is string => !!m))].sort()
	);

	function toggle(list: string[], v: string): string[] {
		return list.includes(v) ? list.filter((x) => x !== v) : [...list, v];
	}

	// Filtered + sorted list (the pipeline the count and paging observe).
	const filtered: Session[] = $derived.by(() => {
		const q = searchQuery.trim().toLowerCase();
		let list = sessions.filter((s) => {
			if (q) {
				const hay = `${s.name ?? ''} ${derivedMeta[s.session_id]?.title ?? ''} ${s.session_id}`.toLowerCase();
				if (!hay.includes(q)) return false;
			}
			if (agentFilter.length && !agentFilter.includes(agentType(s))) return false;
			if (modelFilter.length) {
				// Match if the session used ANY of the filtered models.
				const used = sessionModels(s);
				if (!used.some((m) => modelFilter.includes(m))) return false;
			}
			return true;
		});
		const dir = sortDir === 'asc' ? 1 : -1;
		list = [...list].sort((a, b) => {
			switch (sortKey) {
				case 'date': {
					// Undated always sink to the bottom regardless of direction.
					const da = sessionDate(a);
					const db = sessionDate(b);
					if (da === undefined && db === undefined) return 0;
					if (da === undefined) return 1;
					if (db === undefined) return -1;
					return (da - db) * dir;
				}
				case 'used':
					return (a.session_tokens_used - b.session_tokens_used) * dir;
				case 'saved':
					return (a.session_tokens_saved - b.session_tokens_saved) * dir;
				case 'ratio':
					return (a.cumulative_ratio - b.cumulative_ratio) * dir;
				case 'name':
					return listLabel(a).localeCompare(listLabel(b)) * dir;
			}
		});
		return list;
	});
	const paged: Session[] = $derived(filtered.slice(0, visibleCount));

	// ── Within-session transcript search ───────────────────────────────────
	let turnSearch = $state(''); // cleared on session change (see loadTurns)
	const filteredTurns: Turn[] = $derived.by(() => {
		const q = turnSearch.trim().toLowerCase();
		if (!q) return turns;
		return turns.filter((t) => {
			if (t.user_text?.toLowerCase().includes(q)) return true;
			if (t.assistant_text?.toLowerCase().includes(q)) return true;
			if (t.tool_calls?.some((tc) => tc.toLowerCase().includes(q))) return true;
			return false;
		});
	});
	// Split text into {text, hit} segments so matches can be <mark>-highlighted
	// without {@html} (keeps it XSS-safe).
	function segments(text: string, q: string): { text: string; hit: boolean }[] {
		const needle = q.trim().toLowerCase();
		if (!needle) return [{ text, hit: false }];
		const out: { text: string; hit: boolean }[] = [];
		const hay = text.toLowerCase();
		let i = 0;
		for (;;) {
			const idx = hay.indexOf(needle, i);
			if (idx === -1) {
				out.push({ text: text.slice(i), hit: false });
				break;
			}
			if (idx > i) out.push({ text: text.slice(i, idx), hit: false });
			out.push({ text: text.slice(idx, idx + needle.length), hit: true });
			i = idx + needle.length;
		}
		return out;
	}
</script>

<div class="page">
	<div class="header">
		<h1>Sessions</h1>
		<button class="refresh-btn" onclick={load} disabled={loading}>
			{loading ? 'Loading…' : 'Refresh'}
		</button>
		<button
			class="sync-btn"
			onclick={syncSessions}
			disabled={syncing}
			title="Re-scan Claude Code transcripts on this machine and pull them into the hub (local hub only)"
		>
			{syncing ? 'Syncing…' : '⟳ Sync transcripts'}
		</button>
		{#if syncMsg}
			<span class="sync-msg" class:err={syncErr}>{syncMsg}</span>
		{/if}
		<span class="ago">refreshed {formatAgo(auto.lastRefreshed)}</span>
	</div>

	{#if error}
		<p class="error">{error}</p>
	{:else if loading}
		<p class="muted">Loading sessions…</p>
	{:else if sessions.length === 0}
		<p class="muted">No sessions yet. Run <code>ctx recall</code> or <code>ctx remember</code> to start one.</p>
	{:else}
		<div class="layout">
			<div class="list-col">
				<div class="toolbar">
					<input
						class="search"
						type="search"
						placeholder="Search name / title / id…"
						bind:value={searchInput}
						aria-label="Search sessions"
					/>
					<div class="toolbar-row">
						<div class="sort">
							<select class="sort-select" bind:value={sortKey} aria-label="Sort by">
								{#each Object.entries(SORT_LABELS) as [k, label]}
									<option value={k}>{label}</option>
								{/each}
							</select>
							<button
								class="dir-btn"
								onclick={() => (sortDir = sortDir === 'asc' ? 'desc' : 'asc')}
								title={sortDir === 'asc' ? 'Ascending' : 'Descending'}
								aria-label="Toggle sort direction"
							>
								{sortDir === 'asc' ? '↑' : '↓'}
							</button>
						</div>
						<span class="count">{filtered.length} of {sessions.length}</span>
					</div>
					{#if agentOptions.length > 1 || modelOptions.length > 1 || agentFilter.length || modelFilter.length}
						<div class="chips">
							{#if agentOptions.length > 1 || agentFilter.length}
								{#each agentOptions as a}
									<button
										class="chip"
										class:on={agentFilter.includes(a)}
										onclick={() => (agentFilter = toggle(agentFilter, a))}
									>{a}</button>
								{/each}
							{/if}
							{#if (agentOptions.length > 1 || agentFilter.length) && (modelOptions.length > 1 || modelFilter.length)}
								<span class="chip-sep" aria-hidden="true"></span>
							{/if}
							{#if modelOptions.length > 1 || modelFilter.length}
								{#each modelOptions as m}
									<button
										class="chip model"
										class:on={modelFilter.includes(m)}
										onclick={() => (modelFilter = toggle(modelFilter, m))}
									>{m}</button>
								{/each}
							{/if}
							{#if agentFilter.length || modelFilter.length}
								<button
									class="chip clear"
									onclick={() => { agentFilter = []; modelFilter = []; }}
								>Clear</button>
							{/if}
						</div>
					{/if}
				</div>

				<div class="list">
					{#if filtered.length === 0}
						<p class="muted no-match">No sessions match.</p>
					{/if}
					{#each paged as s (s.session_id)}
						{@const date = sessionDate(s)}
						<button
							class="session-row"
							class:active={selected?.session_id === s.session_id}
							onclick={() => selected = s}
						>
							<div class="session-name">{listLabel(s)}</div>
							{#if hasDistinctName(s)}
								<div class="session-id" title={s.session_id}>{s.session_id}</div>
							{/if}
							<div class="session-tags">
								<span class="agent-chip">{agentType(s)}</span>
								{#if date !== undefined}
									<span class="row-date" title={new Date(date).toLocaleString()}>{shortDate(date)}</span>
								{/if}
							</div>
							<div class="session-meta">
								<span>{fmt(s.session_tokens_used)} tokens used</span>
								<span class="ratio" style="color: {ratioColor(s.cumulative_ratio)}">
									{s.cumulative_ratio.toFixed(1)}x
								</span>
							</div>
						</button>
					{/each}
					{#if filtered.length > visibleCount}
						<button class="load-more" onclick={() => (visibleCount += PAGE_SIZE)}>
							Load more ({filtered.length - visibleCount} remaining)
						</button>
					{/if}
				</div>
			</div>

			{#if selected}
				<div class="detail">
					<h2>{detailTitle}</h2>
					{#if detailTitle !== selected.session_id}
						<div class="detail-id" title="session id">{selected.session_id}</div>
					{/if}

					<div class="stat-grid">
						<div class="stat">
							<div class="stat-value" title={exact(selected.session_tokens_used)}>{fmt(selected.session_tokens_used)}</div>
							<div class="stat-label">Tokens used</div>
						</div>
						<div class="stat">
							<div class="stat-value" style="color: var(--success)" title={exact(selected.session_tokens_saved)}>{fmt(selected.session_tokens_saved)}</div>
							<div class="stat-label">Tokens saved</div>
						</div>
						<div class="stat">
							<div class="stat-value" title={exact(selected.total_graph_size_tokens)}>{fmt(selected.total_graph_size_tokens)}</div>
							<div class="stat-label">Graph size (tokens)</div>
						</div>
						<div class="stat">
							<div class="stat-value" style="color: {ratioColor(selected.cumulative_ratio)}; font-size: 2rem">
								{selected.cumulative_ratio.toFixed(1)}x
							</div>
							<div class="stat-label">Savings ratio</div>
						</div>
					</div>

					{#if selected.session_tokens_used === 0 && selected.llm_call_count > 0}
						<p class="zero-hint">
							This session reported LLM usage but no memory operations carry its
							session id — its recalls likely ran under the
							<code>default</code> session (the agent isn't sending
							<code>X-CTXone-Session</code> on memory calls). Used/saved
							accrue there instead.
						</p>
					{/if}

					{#if selected.llm_call_count > 0}
						<h3>LLM Consumption</h3>
						<div class="stat-grid">
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_call_count)}>{fmt(selected.llm_call_count)}</div>
								<div class="stat-label">API calls</div>
							</div>
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_input_tokens)}>{fmt(selected.llm_input_tokens)}</div>
								<div class="stat-label">Input tokens</div>
							</div>
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_output_tokens)}>{fmt(selected.llm_output_tokens)}</div>
								<div class="stat-label">Output tokens</div>
							</div>
							<div class="stat">
								<div class="stat-value" title={exact(selected.llm_cache_read_tokens)}>{fmt(selected.llm_cache_read_tokens)}</div>
								<div class="stat-label">Cache read tokens</div>
							</div>
						</div>
						{#if selected.models_used && selected.models_used.length > 1}
							<p class="model-info">
								Models: {#each selected.models_used as m, i}<code>{m}</code>{#if i < selected.models_used.length - 1}, {/if}{/each}
								{#if selected.last_provider}via {selected.last_provider}{/if}
							</p>
						{:else if selected.last_model}
							<p class="model-info">
								Last model: <code>{selected.last_model}</code>
								{#if selected.last_provider}via {selected.last_provider}{/if}
							</p>
						{/if}
					{:else}
						<p class="muted hint">
							No LLM usage reported. Agents can call <code>record_llm_usage</code> (MCP)
							or <code>POST /api/stats/llm_usage</code> to surface real token counts.
						</p>
					{/if}

					<h3>
							Conversation
							{#if turns.length}
								<span class="count"
									>{turnSearch.trim() ? `${filteredTurns.length} of ${turns.length}` : turns.length} turns</span
								>
							{/if}
						</h3>
						{#if turns.length > 0 && !turnsLoading && !turnsError}
							<input
								class="turn-search"
								type="search"
								placeholder="Search this transcript…"
								bind:value={turnSearch}
								aria-label="Search transcript"
							/>
						{/if}
					{#if turnsLoading}
						<p class="muted">Loading transcript…</p>
					{:else if turnsError}
						<p class="muted hint">Transcript unavailable: {turnsError}</p>
					{:else if turns.length === 0}
						<p class="muted hint">
							No captured turns for this session. Turn content is recorded when an
							agent posts to <code>/api/sessions/{'{sid}'}/turns</code> (e.g. via the
							session-ingest tooling).
						</p>
					{:else if filteredTurns.length === 0}
						<p class="muted hint">No turns match this search.</p>
					{:else}
						<ol class="turns">
							{#each filteredTurns as t (t.key)}
								{@const q = turnSearch.trim()}
								<li class="turn">
									<div class="turn-head">
										<span class="turn-idx">#{(t.turn_index ?? 0) + 1}</span>
										{#if t.model}<span class="turn-model">{t.model}</span>{/if}
										{#if t.timestamp}<span class="turn-time"
												>{new Date(t.timestamp).toLocaleString()}</span
											>{/if}
										{#if t.tokens}
											<span class="turn-tok" title="input / output tokens"
												>{fmt(t.tokens.input ?? 0)}↑ {fmt(t.tokens.output ?? 0)}↓</span
											>
										{/if}
									</div>
									{#if t.user_text?.trim()}
										<div class="msg user">
											<span class="msg-role">User</span>
											<div class="msg-body">{#each segments(t.user_text ?? '', q) as seg}{#if seg.hit}<mark>{seg.text}</mark>{:else}{seg.text}{/if}{/each}</div>
										</div>
									{/if}
									{#if t.assistant_text?.trim()}
										<div class="msg assistant">
											<span class="msg-role">Assistant</span>
											<div class="msg-body">{#each segments(t.assistant_text ?? '', q) as seg}{#if seg.hit}<mark>{seg.text}</mark>{:else}{seg.text}{/if}{/each}</div>
										</div>
									{/if}
									{#if t.tool_calls?.length}
										<div class="msg tool">
											<button
												class="msg-role tool-toggle"
												onclick={() => (expandedTools[t.key] = !expandedTools[t.key])}
											>
												{expandedTools[t.key] ? '▾' : '▸'} {t.tool_calls.length} tool call{t
													.tool_calls.length > 1
													? 's'
													: ''}
											</button>
											<div class="msg-body">
												{#each t.tool_calls as tc}
													<div class="tool-summary">{#each segments(tc, q) as seg}{#if seg.hit}<mark>{seg.text}</mark>{:else}{seg.text}{/if}{/each}</div>
												{/each}
												{#if expandedTools[t.key] && t.tool_calls_raw?.length}
													<pre class="tool-raw">{JSON.stringify(t.tool_calls_raw, null, 2)}</pre>
												{/if}
											</div>
										</div>
									{/if}
								</li>
							{/each}
						</ol>
					{/if}

					<h3>Memories {#if memories.length}<span class="count">{memories.length}</span>{/if}</h3>
					{#if memoriesLoading}
						<p class="muted">Loading memories…</p>
					{:else if memories.length === 0}
						<p class="muted hint">
							No memories from this session. Transcript turns show in the
							Conversation panel above; this lists facts captured via
							<code>remember</code>.
						</p>
					{:else}
						<ul class="memory-list">
							{#each memories as m}
								<li>
									<button
										class="memory-item"
										onclick={() => (openMemory = m)}
										title="View memory"
									>
										<div class="memory-head">
											<code class="memory-id">{m.id.slice(0, 12)}</code>
											<span class="memory-agent">{m.agent_id}</span>
											<span class="memory-time">{new Date(m.timestamp).toLocaleString()}</span>
										</div>
										<div class="memory-desc">{m.intent.description}</div>
									</button>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			{:else}
				<div class="detail placeholder">
					<p class="muted">Select a session to see details.</p>
				</div>
			{/if}
		</div>
	{/if}
</div>

{#if openMemory}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="mem-backdrop" onclick={() => (openMemory = null)}>
		<div class="mem-modal" onclick={(e) => e.stopPropagation()}>
			<div class="mem-modal-head">
				<h4>Memory</h4>
				<button class="mem-close" onclick={() => (openMemory = null)} aria-label="Close">×</button>
			</div>
			<div class="mem-body">{openMemory.intent.description}</div>
			<dl class="mem-meta">
				<dt>Commit</dt>
				<dd><code>{openMemory.id}</code></dd>
				<dt>Agent</dt>
				<dd>{openMemory.agent_id}</dd>
				<dt>When</dt>
				<dd>{new Date(openMemory.timestamp).toLocaleString()}</dd>
			</dl>
			{#if openMemory.intent.tags?.length}
				<div class="memory-tags">
					{#each openMemory.intent.tags as t}<span class="tag">{t}</span>{/each}
				</div>
			{/if}
			{#if memPath(openMemory)}
				<a class="mem-link" href={`/browse?path=${encodeURIComponent(memPath(openMemory) ?? '')}`}>
					Open in Browse →
				</a>
			{/if}
		</div>
	</div>
{/if}

<svelte:window onkeydown={(e) => e.key === 'Escape' && (openMemory = null)} />

<style>
	.page { max-width: 1100px; }

	.header {
		display: flex;
		align-items: center;
		gap: 1rem;
		margin-bottom: 1.5rem;
	}

	h1 { margin: 0; font-size: 1.8rem; }

	.refresh-btn {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		padding: 0.35rem 0.75rem;
		border-radius: 6px;
		cursor: pointer;
		font-size: 0.85rem;
	}

	.refresh-btn:hover:not(:disabled) { color: var(--text-0); border-color: var(--text-3); }
	.refresh-btn:disabled { opacity: 0.5; cursor: default; }

	.ago {
		font-size: 0.75rem;
		font-family: monospace;
		color: var(--text-3);
		margin-left: auto;
	}

	.error { color: var(--danger); }
	.muted { color: var(--text-3); font-size: 0.9rem; }

	.layout {
		display: grid;
		grid-template-columns: 300px 1fr;
		gap: 1.5rem;
		align-items: start;
	}

	.list {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.session-row {
		width: 100%;
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.75rem 1rem;
		text-align: left;
		cursor: pointer;
		color: var(--text-1);
		transition: all 0.15s;
	}

	.session-row:hover { border-color: var(--text-3); background: var(--bg-1); }
	.session-row.active { border-color: var(--border-hi); background: var(--accent-bg); }

	.session-name {
		font-family: var(--lens-font, inherit);
		font-size: 0.9rem;
		font-weight: 600;
		line-height: 1.35;
		margin-bottom: 0.2rem;
		overflow-wrap: anywhere;
	}

	.session-meta {
		display: flex;
		justify-content: space-between;
		font-size: 0.78rem;
		color: var(--text-3);
	}

	.ratio { font-weight: 600; }

	.detail {
		background: var(--bg-1);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1.5rem;
	}

	.detail.placeholder {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 200px;
	}

	.detail h2 {
		margin: 0 0 1.25rem 0;
		font-family: monospace;
		font-size: 1.1rem;
		word-break: break-all;
	}

	.detail h3 {
		margin: 1.5rem 0 0.75rem;
		font-size: 0.9rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-3);
	}

	.stat-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
		gap: 1rem;
	}

	.stat {
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 1rem;
	}

	.stat-value {
		font-size: 1.5rem;
		font-weight: 700;
		color: var(--text-0);
	}

	.stat-label {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-3);
		margin-top: 0.25rem;
	}

	.model-info {
		margin-top: 1rem;
		font-size: 0.85rem;
		color: var(--text-3);
	}

	.hint { margin-top: 1rem; }

	.memory-list {
		list-style: none;
		padding: 0;
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.memory-item {
		display: block;
		width: 100%;
		text-align: left;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
		cursor: pointer;
		transition: border-color var(--lens-dur-fast, 120ms) ease;
		font: inherit;
		color: inherit;
	}
	.memory-item:hover {
		border-color: var(--lens-accent, #6ea8ff);
	}
	h3 .count {
		font-size: var(--lens-font-size-xs, 0.75rem);
		color: var(--lens-muted, #96a2bd);
		font-weight: 400;
		margin-left: 0.35rem;
	}
	.mem-backdrop {
		position: fixed;
		inset: 0;
		background: rgb(0 0 0 / 0.55);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 50;
		padding: 1rem;
	}
	.mem-modal {
		background: var(--lens-surface-raised, var(--bg-1));
		border: 1px solid var(--lens-border-strong, var(--border));
		border-radius: var(--lens-radius-md, 10px);
		box-shadow: 0 12px 40px rgb(0 0 0 / 0.5);
		max-width: 560px;
		width: 100%;
		max-height: 80vh;
		overflow-y: auto;
		padding: 1rem 1.1rem;
	}
	.mem-modal-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 0.6rem;
	}
	.mem-modal-head h4 {
		margin: 0;
		font-size: var(--lens-font-size-sm, 0.9rem);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--lens-muted, #96a2bd);
	}
	.mem-close {
		background: none;
		border: none;
		color: var(--lens-muted, #96a2bd);
		font-size: 1.4rem;
		line-height: 1;
		cursor: pointer;
	}
	.mem-close:hover {
		color: var(--lens-text, #f4f6fa);
	}
	.mem-body {
		white-space: pre-wrap;
		word-break: break-word;
		line-height: 1.55;
		color: var(--lens-text, #f4f6fa);
		margin-bottom: 0.8rem;
	}
	.mem-meta {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.2rem 0.8rem;
		font-size: var(--lens-font-size-xs, 0.78rem);
		margin: 0 0 0.6rem;
	}
	.mem-meta dt {
		color: var(--lens-muted, #96a2bd);
		text-transform: uppercase;
		letter-spacing: 0.03em;
		font-size: var(--lens-font-size-2xs, 0.68rem);
	}
	.mem-meta dd {
		margin: 0;
		color: var(--lens-text, #dfe4ec);
	}
	.mem-link {
		display: inline-block;
		margin-top: 0.4rem;
		color: var(--lens-accent, #6ea8ff);
		font-size: var(--lens-font-size-xs, 0.8rem);
		text-decoration: none;
	}
	.mem-link:hover {
		text-decoration: underline;
	}

	.memory-head {
		display: flex;
		gap: 0.75rem;
		align-items: center;
		font-size: 0.75rem;
		color: var(--text-3);
		margin-bottom: 0.3rem;
	}

	.memory-id { color: var(--text-2); }
	.memory-agent { color: var(--accent); }
	.memory-time { margin-left: auto; }

	.memory-desc {
		font-size: 0.88rem;
		color: var(--text-1);
		line-height: 1.4;
	}

	.memory-tags {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
		margin-top: 0.4rem;
	}

	.tag {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		font-size: 0.7rem;
		padding: 0.1rem 0.4rem;
		border-radius: 3px;
	}

	code {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		padding: 0.1em 0.35em;
		border-radius: 3px;
		font-size: 0.85em;
	}

	.zero-hint {
		margin: 0.6rem 0 0;
		padding: 0.5rem 0.7rem;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-muted);
		background: color-mix(in srgb, var(--lens-info, #67c7e6) 8%, var(--lens-surface));
		border: 1px solid color-mix(in srgb, var(--lens-info, #67c7e6) 25%, var(--lens-border));
		border-radius: var(--lens-radius-sm);
	}
	.zero-hint code {
		font-family: var(--lens-font-mono);
		color: var(--lens-text);
	}

	h3 .count {
		font-size: var(--lens-font-size-xs);
		font-weight: 400;
		color: var(--lens-muted);
		margin-left: 0.4rem;
	}
	.turns {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
		max-height: 460px;
		overflow-y: auto;
	}
	.turn {
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		background: var(--lens-surface);
		padding: 0.5rem 0.6rem;
	}
	.turn-head {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		margin-bottom: 0.4rem;
	}
	.turn-idx {
		font-weight: 700;
		color: var(--lens-text);
	}
	.turn-tok {
		margin-left: auto;
	}
	.msg {
		display: grid;
		grid-template-columns: 68px 1fr;
		gap: 0.5rem;
		padding: 0.25rem 0;
	}
	.msg-role {
		font-size: var(--lens-font-size-2xs);
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		padding-top: 0.15rem;
	}
	.msg.user .msg-role {
		color: var(--lens-accent);
	}
	.msg.assistant .msg-role {
		color: var(--lens-ok);
	}
	.msg.tool .msg-role {
		color: var(--lens-info, #67c7e6);
	}
	.msg-body {
		font-size: var(--lens-font-size-xs);
		line-height: 1.5;
		color: var(--lens-text);
		white-space: pre-wrap;
		word-break: break-word;
		min-width: 0;
	}
	.tool-toggle {
		background: none;
		border: none;
		cursor: pointer;
		text-align: left;
		padding: 0.15rem 0;
		font-family: inherit;
	}
	.tool-summary {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
	}
	.tool-raw {
		margin: 0.35rem 0 0;
		padding: 0.4rem 0.5rem;
		background: color-mix(in srgb, var(--lens-info, #67c7e6) 6%, var(--lens-surface-raised));
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-2xs);
		overflow-x: auto;
		max-height: 260px;
	}

	.session-id {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		margin-top: 0.1rem;
	}
	.detail-id {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-2xs);
		color: var(--lens-muted);
		margin: -0.2rem 0 0.6rem;
	}

	.sync-btn {
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 14%, var(--bg-hover));
		border: 1px solid color-mix(in srgb, var(--lens-accent, #6ea8ff) 40%, var(--border));
		color: var(--lens-accent, #93c5fd);
		padding: 0.35rem 0.75rem;
		border-radius: 6px;
		cursor: pointer;
		font-size: 0.85rem;
	}
	.sync-btn:hover:not(:disabled) {
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 22%, var(--bg-hover));
	}
	.sync-btn:disabled { opacity: 0.6; cursor: default; }
	.sync-msg {
		font-size: 0.8rem;
		color: var(--lens-ok, #4ade80);
	}
	.sync-msg.err { color: var(--lens-danger, #ff6b6b); }

	/* ── Toolbar ─────────────────────────────────────────────────────────── */
	.list-col {
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
		min-width: 0;
	}
	.toolbar {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}
	.search,
	.turn-search {
		width: 100%;
		background: var(--bg-0);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--text-0);
		padding: 0.4rem 0.6rem;
		font-size: 0.85rem;
		font-family: inherit;
	}
	.search:focus,
	.turn-search:focus {
		outline: none;
		border-color: var(--lens-accent, #6ea8ff);
	}
	.turn-search {
		margin-bottom: 0.6rem;
	}
	.toolbar-row {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	.sort {
		display: flex;
		gap: 0.3rem;
	}
	.sort-select {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-1);
		border-radius: 6px;
		padding: 0.3rem 0.4rem;
		font-size: 0.8rem;
		cursor: pointer;
	}
	.dir-btn {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-1);
		border-radius: 6px;
		padding: 0.3rem 0.5rem;
		font-size: 0.85rem;
		cursor: pointer;
		line-height: 1;
	}
	.dir-btn:hover {
		border-color: var(--text-3);
		color: var(--text-0);
	}
	.count {
		font-size: var(--lens-font-size-xs, 0.78rem);
		color: var(--lens-muted, var(--text-3));
		margin-left: auto;
		white-space: nowrap;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
		align-items: center;
	}
	.chip {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		font-size: 0.72rem;
		padding: 0.15rem 0.5rem;
		border-radius: 999px;
		cursor: pointer;
		font-family: inherit;
	}
	.chip:hover {
		border-color: var(--text-3);
		color: var(--text-0);
	}
	.chip.on {
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 22%, var(--bg-hover));
		border-color: color-mix(in srgb, var(--lens-accent, #6ea8ff) 55%, var(--border));
		color: var(--lens-accent, #93c5fd);
	}
	.chip.model {
		font-family: var(--lens-font-mono, monospace);
	}
	.chip.clear {
		color: var(--lens-muted, var(--text-3));
	}
	.chip-sep {
		width: 1px;
		align-self: stretch;
		background: var(--border);
		margin: 0.1rem 0.2rem;
	}
	.no-match {
		padding: 0.5rem 0.2rem;
	}

	.session-tags {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		margin: 0.25rem 0;
	}
	.agent-chip {
		font-size: 0.68rem;
		font-weight: 600;
		padding: 0.1rem 0.4rem;
		border-radius: 4px;
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 14%, var(--bg-0));
		border: 1px solid color-mix(in srgb, var(--lens-accent, #6ea8ff) 30%, var(--border));
		color: var(--lens-accent, #93c5fd);
	}
	.row-date {
		font-size: 0.72rem;
		color: var(--text-3);
	}
	.load-more {
		background: var(--bg-hover);
		border: 1px solid var(--border);
		color: var(--text-2);
		border-radius: 8px;
		padding: 0.5rem;
		font-size: 0.8rem;
		cursor: pointer;
		font-family: inherit;
	}
	.load-more:hover {
		border-color: var(--text-3);
		color: var(--text-0);
	}
	.msg-body :global(mark) {
		background: color-mix(in srgb, var(--lens-warn, #f5c451) 45%, transparent);
		color: inherit;
		border-radius: 2px;
		padding: 0 1px;
	}
	.tool-summary :global(mark) {
		background: color-mix(in srgb, var(--lens-warn, #f5c451) 45%, transparent);
		color: inherit;
		border-radius: 2px;
	}
</style>
