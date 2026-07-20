<script lang="ts">
	import { hubFetch, remember } from '$lib/api';
	import { computeBurn } from '$lib/sessionBurn';
	import { formatCompact } from '@agentstate/lens-core';
	import { namespaceStore } from '$lib/namespaceStore.svelte';
	import { useAutoRefresh, formatAgo } from '$lib/refreshStore.svelte';
	import { renderMarkdown } from '$lib/markdown';

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

	// ── Burn metric ─────────────────────────────────────────────────────────
	// Context tokens spent per edit landed, trailing window vs this session's
	// own baseline. Thresholds are calibrated in sessionBurn.ts against the
	// real rolling-window distribution — do not retune them here.
	const burn = $derived(computeBurn(turns));

	/** Sparkline path for the rolling ratio, clamped so outliers don't flatten it. */
	function burnSpark(series: number[], w = 240, h = 32): string {
		if (series.length < 2) return '';
		const cap = 12; // above this the shape stops mattering; it's all "bad"
		const v = series.map((x) => Math.min(x, cap));
		const max = Math.max(...v, 4);
		const dx = w / (v.length - 1);
		return v
			.map((y, i) => `${i === 0 ? 'M' : 'L'}${(i * dx).toFixed(1)},${(h - (y / max) * h).toFixed(1)}`)
			.join(' ');
	}

	// ── Selection → memory ──────────────────────────────────────────────────
	// Highlight any part of the transcript and save it as a real memory. The
	// selection is captured on mouseup/keyup within the transcript only, so a
	// selection elsewhere on the page never arms the button.
	let selText = $state('');
	let selAt: { x: number; y: number } | null = $state(null);
	let memOpen = $state(false);
	let memFact = $state('');
	let memImportance = $state<'high' | 'medium' | 'low'>('medium');
	let memContext = $state('');
	let memSaving = $state(false);
	let memMsg: string | null = $state(null);
	let memFailed = $state(false);

	function captureSelection(e: Event) {
		if (memOpen) return; // don't fight the editor while it's open
		const sel = window.getSelection();
		const text = sel?.toString().trim() ?? '';
		if (!text || !sel || sel.rangeCount === 0) {
			selText = '';
			selAt = null;
			return;
		}
		// Only arm for selections that live inside the transcript.
		const host = (e.currentTarget as HTMLElement) ?? null;
		if (host && !host.contains(sel.anchorNode)) {
			selText = '';
			selAt = null;
			return;
		}
		const r = sel.getRangeAt(0).getBoundingClientRect();
		selText = text;
		selAt = { x: r.left + r.width / 2, y: r.top };
	}

	/**
	 * Drop the floating button. Its position is viewport-fixed and computed
	 * once from the selection rect, so anything that moves the text out from
	 * under it — now including the detail column's own scroll — must clear it
	 * rather than let it strand over unrelated content.
	 */
	function clearSelectionAffordance() {
		if (memOpen || !selText) return;
		selText = '';
		selAt = null;
	}

	function openMemoryEditor() {
		memFact = selText;
		memContext = selected ? listLabel(selected) : '';
		memMsg = null;
		memFailed = false;
		memOpen = true;
	}

	function closeMemoryEditor() {
		memOpen = false;
		selText = '';
		selAt = null;
	}

	async function saveSelectionAsMemory() {
		if (!memFact.trim()) return;
		memSaving = true;
		memMsg = null;
		memFailed = false;
		try {
			const tags = ['kind:excerpt'];
			if (selected) tags.push(`session:${selected.session_id}`);
			const res = await remember({
				fact: memFact.trim(),
				importance: memImportance,
				context: memContext.trim() || undefined,
				tags
			});
			memMsg = `Saved: ${res.path}`;
			if (selected) await loadMemories(selected.session_id);
			// Leave the panel up briefly so the path is readable, then reset.
			setTimeout(() => {
				if (!memFailed) closeMemoryEditor();
			}, 1200);
		} catch (err) {
			memFailed = true;
			memMsg = err instanceof Error ? err.message : 'Save failed';
		} finally {
			memSaving = false;
		}
	}
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
	// the 30s auto-refresh reuses the cache and only derives brand-new ids.
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
			applyDeepLink();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	/**
	 * `?session=<id>` opens that session directly — the dashboard's burn board
	 * links here. Runs after each load but only selects once, so an
	 * auto-refresh cannot yank the user back to the linked session after they
	 * have clicked elsewhere.
	 */
	let deepLinkApplied = false;
	function applyDeepLink() {
		if (deepLinkApplied || typeof window === 'undefined') return;
		const want = new URLSearchParams(window.location.search).get('session');
		if (!want) {
			deepLinkApplied = true;
			return;
		}
		const hit = sessions.find((s) => s.session_id === want);
		if (hit) {
			selected = hit;
			deepLinkApplied = true;
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

	// Git commits made during the session, pulled from the transcript's
	// Bash tool calls, joined to the ctx plan/task named in the commit
	// message. The message carries the task ref (e.g. "(asd-plan-t-lens
	// t-004)") — the transcript stores the command, not the resulting SHA,
	// so the message is the reliable join.
	interface SessionCommit {
		subject: string;
		tasks: { plan: string; task: string }[];
		/** Conventional-commit type, lowercased; 'other' when the subject
		 * doesn't use one (this repo mixes in bare-scope subjects like
		 * "lens: …", which must not be mistaken for a type). */
		type: string;
	}
	const TASK_REF = /\(([a-z][a-z0-9-]*)\s+(t-\d+)\)/gi;

	/** Types we recognise and colour. Anything else falls to 'other'. */
	const COMMIT_TYPES = [
		'feat',
		'fix',
		'docs',
		'refactor',
		'test',
		'chore',
		'build',
		'ci',
		'style',
		'perf',
		'merge',
		'release'
	];
	function commitType(subject: string): string {
		// `type(scope)!: …` or `type!: …` — the scope and the breaking-change
		// bang are both optional.
		const m = /^([a-z]+)(\([^)]*\))?!?:/i.exec(subject.trim());
		const t = m?.[1]?.toLowerCase();
		return t && COMMIT_TYPES.includes(t) ? t : 'other';
	}
	const sessionCommits = $derived.by<SessionCommit[]>(() => {
		const out: SessionCommit[] = [];
		const seen = new Set<string>();
		for (const t of turns) {
			for (const raw of (t.tool_calls_raw ?? []) as { name?: string; input?: { command?: string } }[]) {
				const cmd = raw?.input?.command;
				if (typeof cmd !== 'string' || !cmd.includes('git commit')) continue;
				// Extract each -m "…" message body (may appear more than once).
				for (const m of cmd.matchAll(/-m\s+"((?:[^"\\]|\\.)*)"/g)) {
					const msg = m[1].replace(/\\n/g, '\n').replace(/\\"/g, '"').trim();
					const subject = msg.split('\n')[0].trim();
					if (!subject || seen.has(subject)) continue;
					seen.add(subject);
					const tasks: { plan: string; task: string }[] = [];
					for (const r of msg.matchAll(TASK_REF)) tasks.push({ plan: r[1], task: r[2] });
					out.push({ subject, tasks, type: commitType(subject) });
				}
			}
		}
		return out;
	});

	// ── Commits toolbar: type chips + linked/orphan toggle ──────────────────
	// Filters are per-session view state (not persisted): the useful set of
	// types differs from session to session, so a sticky choice would more
	// often hide commits than help.
	let commitTypeFilter: string[] = $state([]);
	let commitLinkFilter: 'all' | 'linked' | 'orphan' = $state('all');

	/** Types actually present in this session, in COMMIT_TYPES order so the
	 * chip row is stable rather than ordered by first appearance. */
	const commitTypesPresent = $derived.by<string[]>(() => {
		const present = new Set(sessionCommits.map((c) => c.type));
		return [...COMMIT_TYPES, 'other'].filter((t) => present.has(t));
	});

	const visibleCommits = $derived.by<SessionCommit[]>(() =>
		sessionCommits.filter((c) => {
			if (commitTypeFilter.length && !commitTypeFilter.includes(c.type)) return false;
			if (commitLinkFilter === 'linked' && c.tasks.length === 0) return false;
			if (commitLinkFilter === 'orphan' && c.tasks.length > 0) return false;
			return true;
		})
	);

	function toggleCommitType(t: string) {
		commitTypeFilter = commitTypeFilter.includes(t)
			? commitTypeFilter.filter((x) => x !== t)
			: [...commitTypeFilter, t];
	}
	// Reset filters when switching sessions, so a filter chosen on one
	// session doesn't silently empty the next one's list.
	$effect(() => {
		void selected?.session_id;
		commitTypeFilter = [];
		commitLinkFilter = 'all';
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

	// Markdown-rendered turn bodies, memoized per session. Keyed by turn key →
	// {user, assistant} sanitized HTML. `turns` only changes on session load,
	// so this recomputes once per session (not on every keystroke). When a
	// transcript search is active we fall back to the plain-text + <mark>
	// highlighter below, so the map is only consumed when `turnSearch` is empty.
	const renderedTurns = $derived.by(() => {
		const map = new Map<string, { user: string; assistant: string }>();
		for (const t of turns) {
			map.set(t.key, {
				user: renderMarkdown(t.user_text),
				assistant: renderMarkdown(t.assistant_text)
			});
		}
		return map;
	});
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
	{:else if loading && sessions.length === 0}
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
				<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
				<div class="detail" onscroll={clearSelectionAffordance}>
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

					{#if turns.length > 0 && !turnsLoading && !turnsError}
						<h3>Session efficiency</h3>
						<div class="burn burn-{burn.level}">
							<div class="burn-head">
								<span class="burn-badge">{burn.level}</span>
								<span class="burn-headline">{burn.headline}</span>
							</div>
							<p class="burn-detail">{burn.detail}</p>
							{#if burn.series.length > 1}
								<svg
									class="burn-spark"
									viewBox="0 0 240 32"
									preserveAspectRatio="none"
									role="img"
									aria-label="Cost per edit over the session, relative to its baseline"
								>
									<path d={burnSpark(burn.series)} fill="none" stroke="currentColor" stroke-width="1.5" />
								</svg>
								<div class="burn-foot">
									<span>cost per edit vs this session’s baseline →</span>
									{#if burn.knee !== null}
										<span class="burn-knee">crossed over around turn #{burn.knee + 1}</span>
									{/if}
								</div>
							{/if}
						</div>
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
						<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
						<ol class="turns" onmouseup={captureSelection} onkeyup={captureSelection}>
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
											{#if q}
												<div class="msg-body">{#each segments(t.user_text ?? '', q) as seg}{#if seg.hit}<mark>{seg.text}</mark>{:else}{seg.text}{/if}{/each}</div>
											{:else}
												<!-- eslint-disable-next-line svelte/no-at-html-tags — sanitized by renderMarkdown (DOMPurify) -->
												<div class="msg-body markdown">{@html renderedTurns.get(t.key)?.user ?? ''}</div>
											{/if}
										</div>
									{/if}
									{#if t.assistant_text?.trim()}
										<div class="msg assistant">
											<span class="msg-role">Agent</span>
											{#if q}
												<div class="msg-body">{#each segments(t.assistant_text ?? '', q) as seg}{#if seg.hit}<mark>{seg.text}</mark>{:else}{seg.text}{/if}{/each}</div>
											{:else}
												<!-- eslint-disable-next-line svelte/no-at-html-tags — sanitized by renderMarkdown (DOMPurify) -->
												<div class="msg-body markdown">{@html renderedTurns.get(t.key)?.assistant ?? ''}</div>
											{/if}
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

					{#if sessionCommits.length}
						<h3>
							Commits
							<span class="count">
								{visibleCommits.length === sessionCommits.length
									? sessionCommits.length
									: `${visibleCommits.length}/${sessionCommits.length}`}
							</span>
						</h3>
						<div class="commit-filters">
							<div class="chip-row">
								{#each commitTypesPresent as t}
									<button
										type="button"
										class="type-chip t-{t}"
										class:active={commitTypeFilter.includes(t)}
										aria-pressed={commitTypeFilter.includes(t)}
										onclick={() => toggleCommitType(t)}
										title={`Show only ${t} commits`}
									>
										{t}
									</button>
								{/each}
							</div>
							<div class="seg-group">
								{#each [['all', 'All'], ['linked', 'Linked'], ['orphan', 'Orphan']] as [val, label]}
									<button
										type="button"
										class="seg"
										class:active={commitLinkFilter === val}
										onclick={() => (commitLinkFilter = val as typeof commitLinkFilter)}
										title={val === 'linked'
											? 'Commits that reference a ctx plan task'
											: val === 'orphan'
												? 'Commits with no plan task reference'
												: 'All commits'}
									>
										{label}
									</button>
								{/each}
							</div>
						</div>
						{#if visibleCommits.length === 0}
							<p class="muted">No commits match these filters.</p>
						{/if}
						<ul class="commit-list">
							{#each visibleCommits as c}
								<li class="commit-item t-{c.type}" class:orphan={c.tasks.length === 0}>
									<div class="commit-subject">
										<span class="type-badge t-{c.type}">{c.type}</span>
										{c.subject}
									</div>
									{#if c.tasks.length}
										<div class="commit-tasks">
											{#each c.tasks as t}
												<a
													class="commit-task"
													href={`/plans?plan=${encodeURIComponent(t.plan)}&task=${t.task}`}
													title={`Open ${t.plan} ${t.task} in Plans`}
												>
													{t.plan} <strong>{t.task}</strong>
												</a>
											{/each}
										</div>
									{/if}
								</li>
							{/each}
						</ul>
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

<!-- Floating "save this" affordance, anchored to the live selection. -->
{#if selText && !memOpen && selAt}
	<button
		class="sel-save"
		style="left:{selAt.x}px; top:{selAt.y}px"
		onclick={openMemoryEditor}
		title="Save the highlighted text as a memory"
	>
		✦ Save as memory
	</button>
{/if}

{#if memOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="mem-backdrop" onclick={closeMemoryEditor}>
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="mem-card sel-card" onclick={(e) => e.stopPropagation()}>
			<div class="mem-head">
				<h3>Save excerpt as memory</h3>
				<button class="mem-close" onclick={closeMemoryEditor} aria-label="Close">×</button>
			</div>
			<label class="sel-label" for="sel-fact">Fact</label>
			<textarea id="sel-fact" class="sel-text" rows="6" bind:value={memFact} disabled={memSaving}
			></textarea>
			<div class="sel-row">
				<select bind:value={memImportance} disabled={memSaving} aria-label="Importance">
					<option value="high">High</option>
					<option value="medium">Medium</option>
					<option value="low">Low</option>
				</select>
				<input
					type="text"
					bind:value={memContext}
					placeholder="context"
					disabled={memSaving}
					aria-label="Context"
				/>
			</div>
			<div class="sel-actions">
				<button
					class="sel-primary"
					onclick={saveSelectionAsMemory}
					disabled={memSaving || !memFact.trim()}
				>
					{memSaving ? 'Saving…' : 'Remember'}
				</button>
				<button class="sel-secondary" onclick={closeMemoryEditor} disabled={memSaving}>Cancel</button>
			</div>
			{#if memMsg}
				<p class="sel-msg" class:failed={memFailed}>{memMsg}</p>
			{/if}
		</div>
	</div>
{/if}

<svelte:window
	onkeydown={(e) => {
		if (e.key !== 'Escape') return;
		if (memOpen) closeMemoryEditor();
		else openMemory = null;
	}}
	onmousedown={(e) => {
		// A click anywhere else collapses the selection, so the floating
		// button must go with it — otherwise it strands over dead text.
		// The button itself is exempt: its own mousedown precedes its click.
		if (memOpen || !selText) return;
		if ((e.target as HTMLElement)?.closest?.('.sel-save')) return;
		selText = '';
		selAt = null;
	}}
/>

<style>
	/* ── Burn metric ─────────────────────────────────────────────────────── */
	.burn {
		border: 1px solid var(--lens-border);
		border-left-width: 3px;
		border-radius: var(--lens-radius-sm);
		padding: var(--lens-space-3);
		margin-bottom: var(--lens-space-4);
		background: var(--lens-surface-raised);
	}
	.burn-productive { border-left-color: var(--lens-ok, #4ade80); color: var(--lens-ok, #4ade80); }
	.burn-diminishing { border-left-color: var(--lens-warn, #fbbf24); color: var(--lens-warn, #fbbf24); }
	.burn-burning { border-left-color: var(--lens-danger, #f87171); color: var(--lens-danger, #f87171); }
	.burn-unknown { border-left-color: var(--lens-border-strong); color: var(--lens-text-muted); }

	.burn-head {
		display: flex;
		align-items: baseline;
		gap: var(--lens-space-2);
		flex-wrap: wrap;
	}
	.burn-badge {
		font-family: var(--lens-font-mono);
		font-size: var(--lens-font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		border: 1px solid currentColor;
		border-radius: var(--lens-radius-sm);
		padding: 0 var(--lens-space-2);
	}
	.burn-headline {
		font-weight: 600;
		font-size: var(--lens-font-size-sm);
	}
	.burn-detail {
		margin: var(--lens-space-2) 0 0;
		color: var(--lens-text);
		font-size: var(--lens-font-size-sm);
	}
	.burn-spark {
		display: block;
		width: 100%;
		height: 32px;
		margin-top: var(--lens-space-3);
		overflow: visible;
	}
	.burn-foot {
		display: flex;
		justify-content: space-between;
		gap: var(--lens-space-2);
		flex-wrap: wrap;
		margin-top: var(--lens-space-1, 4px);
		font-size: var(--lens-font-size-xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-text-muted);
	}
	.burn-knee { color: currentColor; }

	/* ── Selection → memory ──────────────────────────────────────────────── */
	.sel-save {
		position: fixed;
		transform: translate(-50%, calc(-100% - 8px));
		z-index: 60;
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent, var(--lens-border-strong));
		border-radius: var(--lens-radius-sm);
		color: var(--lens-accent-hover, var(--lens-accent));
		padding: var(--lens-space-1, 4px) var(--lens-space-3);
		font-size: var(--lens-font-size-xs);
		font-weight: 600;
		cursor: pointer;
		box-shadow: 0 2px 10px rgb(0 0 0 / 0.35);
		white-space: nowrap;
	}
	.sel-save:hover { border-color: var(--lens-accent-hover, var(--lens-accent)); }

	.sel-card { max-width: 560px; }
	.sel-label {
		display: block;
		font-size: var(--lens-font-size-xs);
		color: var(--lens-text-muted);
		margin-bottom: var(--lens-space-1, 4px);
	}
	.sel-text,
	.sel-row input,
	.sel-row select {
		background: var(--lens-surface);
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm);
		color: var(--lens-text);
		padding: var(--lens-space-2);
		font-size: var(--lens-font-size-sm);
		font-family: var(--lens-font-sans);
		box-sizing: border-box;
	}
	.sel-text {
		width: 100%;
		resize: vertical;
		font-family: var(--lens-font-mono);
		line-height: 1.5;
	}
	.sel-row {
		display: flex;
		gap: var(--lens-space-2);
		margin-top: var(--lens-space-2);
	}
	.sel-row input { flex: 1; min-width: 0; }
	.sel-actions {
		display: flex;
		gap: var(--lens-space-2);
		margin-top: var(--lens-space-3);
	}
	.sel-primary,
	.sel-secondary {
		border-radius: var(--lens-radius-sm);
		padding: var(--lens-space-2) var(--lens-space-4);
		font-size: var(--lens-font-size-sm);
		font-weight: 600;
		cursor: pointer;
	}
	.sel-primary {
		background: var(--lens-accent-surface);
		border: 1px solid var(--lens-accent, var(--lens-border));
		color: var(--lens-accent-hover, var(--lens-accent));
	}
	.sel-secondary {
		background: transparent;
		border: 1px solid var(--lens-border);
		color: var(--lens-text-muted);
	}
	.sel-primary:disabled,
	.sel-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
	.sel-msg {
		margin: var(--lens-space-2) 0 0;
		font-size: var(--lens-font-size-xs);
		font-family: var(--lens-font-mono);
		color: var(--lens-ok, #4ade80);
		overflow-wrap: anywhere;
	}
	.sel-msg.failed { color: var(--lens-danger); }

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

	/*
		Each column owns its own scroll: a long transcript no longer drags the
		session list off-screen, and paging the list no longer moves the
		transcript you are reading.

		Done by pinning the page to the viewport and letting the columns take
		the leftover height, rather than sticky + a `calc(100vh - …)` guess on
		each column. The guess version measured 76px of the detail pane
		hanging below the fold: the columns start at y=90 (header + main's
		padding), and `content-box` leaves their own 24px/24px padding outside
		max-height. Deriving the height from the flex chain has no such
		off-by-a-header — whatever the header costs, .layout gets the rest.

		4rem = main's 2rem vertical padding, top and bottom. That is the one
		number still assumed from the shell; everything else falls out.
	*/
	.page {
		display: flex;
		flex-direction: column;
		height: calc(100vh - 4rem);
		min-height: 0;
	}

	.header { flex: none; }

	.layout {
		flex: 1 1 auto;
		/* Without this, the grid's min-content height wins and nothing scrolls. */
		min-height: 0;
	}

	.list-col,
	.detail {
		height: 100%;
		overflow-y: auto;
		min-height: 0;
		/* Padding must count against the height, or it pushes past the fold. */
		box-sizing: border-box;
		/* Reserve the scrollbar so content doesn't reflow when it appears. */
		scrollbar-gutter: stable;
		padding-right: var(--lens-space-2);
	}

	/* Matches the dashboard's breakpoint. Short viewports get two cramped
	   scroll panes out of one usable one, so below this hand scrolling back
	   to the page and let the columns run their natural height. */
	@media (max-width: 1100px), (max-height: 600px) {
		/* Unwind the whole chain, not just the columns — leaving .page pinned
		   to the viewport while the columns run free would clip them. */
		.page {
			display: block;
			height: auto;
		}
		.list-col,
		.detail {
			height: auto;
			overflow-y: visible;
			padding-right: 0;
		}
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

	/* ── Rendered markdown (turn bodies) ─────────────────────────────────────
	   Injected via {@html} after DOMPurify. Everything below targets that
	   sanitized subtree, so the selectors must be :global — Svelte's scope hash
	   never lands on injected nodes. Scoped under `.markdown` so nothing here
	   leaks to the rest of the panel. */
	.msg-body.markdown {
		white-space: normal;
	}
	.markdown :global(> *:first-child) {
		margin-top: 0;
	}
	.markdown :global(> *:last-child) {
		margin-bottom: 0;
	}
	.markdown :global(p) {
		margin: 0 0 0.55em;
	}
	.markdown :global(h1),
	.markdown :global(h2),
	.markdown :global(h3),
	.markdown :global(h4),
	.markdown :global(h5),
	.markdown :global(h6) {
		margin: 0.9em 0 0.4em;
		line-height: 1.3;
		font-weight: 700;
		color: var(--lens-text-strong, var(--lens-text));
		text-transform: none;
		letter-spacing: 0;
	}
	/* Headings live inside a panel — scale them down so they don't dominate. */
	.markdown :global(h1) { font-size: 1.15em; }
	.markdown :global(h2) { font-size: 1.08em; }
	.markdown :global(h3) { font-size: 1em; }
	.markdown :global(h4),
	.markdown :global(h5),
	.markdown :global(h6) { font-size: 0.95em; }

	.markdown :global(ul),
	.markdown :global(ol) {
		margin: 0 0 0.55em;
		padding-left: 1.4em;
	}
	.markdown :global(li) {
		margin: 0.15em 0;
	}
	.markdown :global(li > ul),
	.markdown :global(li > ol) {
		margin: 0.15em 0;
	}
	.markdown :global(li input[type='checkbox']) {
		margin-right: 0.4em;
	}
	.markdown :global(a) {
		color: var(--lens-accent, #6ea8ff);
		text-decoration: none;
	}
	.markdown :global(a:hover) {
		text-decoration: underline;
	}
	.markdown :global(strong) { font-weight: 700; color: var(--lens-text-strong, var(--lens-text)); }
	.markdown :global(hr) {
		border: none;
		border-top: 1px solid var(--lens-border);
		margin: 0.8em 0;
	}
	.markdown :global(blockquote) {
		margin: 0 0 0.55em;
		padding: 0.1em 0.8em;
		border-left: 3px solid var(--lens-accent-border, var(--lens-border-strong, var(--lens-border)));
		color: var(--lens-text-secondary, var(--lens-muted));
	}

	/* Inline code — a small mono chip. `:not(pre code)` keeps this off fenced
	   blocks, which are styled separately below. */
	.markdown :global(code) {
		font-family: var(--lens-font-mono, monospace);
		font-size: 0.9em;
	}
	.markdown :global(:not(pre) > code) {
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 10%, var(--lens-surface-raised));
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm, 4px);
		padding: 0.05em 0.35em;
		white-space: normal;
		word-break: break-word;
	}

	/* Fenced code blocks — mono, subtle surface, optional language label,
	   horizontal scroll (JSON/XML/wide code read cleanly instead of wrapping). */
	.markdown :global(.md-code) {
		margin: 0 0 0.6em;
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm, 4px);
		background: var(--lens-surface-raised, var(--lens-surface));
		overflow: hidden;
	}
	.markdown :global(.md-code-lang) {
		font-family: var(--lens-font-mono, monospace);
		font-size: var(--lens-font-size-2xs, 0.68rem);
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--lens-muted);
		padding: 0.25em 0.6em;
		border-bottom: 1px solid var(--lens-border);
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 6%, transparent);
	}
	.markdown :global(.md-code pre) {
		margin: 0;
		padding: 0.55em 0.65em;
		overflow-x: auto;
	}
	.markdown :global(.md-code pre code) {
		font-family: var(--lens-font-mono, monospace);
		font-size: var(--lens-font-size-2xs, 0.72rem);
		line-height: 1.5;
		color: var(--lens-text);
		white-space: pre;
		background: none;
		border: none;
		padding: 0;
	}

	/* Tables — bordered, shaded header, wrapped in a scroll surface so wide
	   tables never break the panel layout. */
	.markdown :global(.md-table-wrap) {
		overflow-x: auto;
		margin: 0 0 0.6em;
		border: 1px solid var(--lens-border);
		border-radius: var(--lens-radius-sm, 4px);
	}
	.markdown :global(table) {
		border-collapse: collapse;
		width: max-content;
		min-width: 100%;
		font-size: var(--lens-font-size-2xs, 0.72rem);
	}
	.markdown :global(th),
	.markdown :global(td) {
		border: 1px solid var(--lens-border);
		padding: 0.3em 0.6em;
		text-align: left;
		vertical-align: top;
	}
	.markdown :global(th) {
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 8%, var(--lens-surface-raised));
		color: var(--lens-text-strong, var(--lens-text));
		font-weight: 700;
		white-space: nowrap;
	}
	.markdown :global(tbody tr:nth-child(even) td) {
		background: color-mix(in srgb, var(--lens-text, #fff) 3%, transparent);
	}
	.markdown :global(img) {
		max-width: 100%;
		height: auto;
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

	.commit-list {
		list-style: none;
		margin: 0 0 1rem;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
	}
	/* One hue per commit type, exposed as --ct so the badge and the row's
	   left border stay in sync from a single declaration. Values come from
	   the app palette (app.css) rather than fresh hex so the section
	   re-themes with everything else. */
	.t-feat {
		--ct: var(--success);
	}
	.t-fix {
		--ct: var(--danger);
	}
	.t-docs {
		--ct: var(--info);
	}
	.t-refactor {
		--ct: var(--accent);
	}
	.t-test {
		--ct: var(--warning);
	}
	.t-perf {
		--ct: var(--warning);
	}
	.t-build,
	.t-ci,
	.t-chore,
	.t-style {
		--ct: var(--text-3);
	}
	.t-merge,
	.t-release {
		--ct: var(--accent);
	}
	.t-other {
		--ct: var(--text-3);
	}

	.commit-item {
		background: var(--bg-0);
		border: 1px solid var(--border);
		/* Type drives the hue; linkage drives whether it reads as solid.
		   Orphan commits (no plan task) get a dashed edge so they're
		   distinguishable without relying on colour alone. */
		border-left: 3px solid var(--ct, var(--text-3));
		border-radius: 6px;
		padding: 0.5rem 0.7rem;
	}
	.commit-item.orphan {
		border-left-style: dashed;
	}
	.type-badge {
		display: inline-block;
		font-family: var(--lens-font-mono, monospace);
		font-size: var(--lens-font-size-2xs, 0.68rem);
		line-height: 1.4;
		padding: 0.05rem 0.4rem;
		margin-right: 0.4rem;
		border-radius: 4px;
		background: color-mix(in srgb, var(--ct, var(--text-3)) 16%, transparent);
		border: 1px solid color-mix(in srgb, var(--ct, var(--text-3)) 45%, var(--border));
		color: var(--ct, var(--text-2));
		vertical-align: baseline;
	}

	/* Same segmented-control idiom the Pinned page uses, kept visually
	   identical so the two pages don't drift. */
	.seg-group {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
	}
	.seg {
		background: var(--bg-1);
		border: 0;
		color: var(--text-2);
		padding: 0.2rem 0.6rem;
		font-size: var(--lens-font-size-2xs, 0.7rem);
		font-family: var(--lens-font-mono, monospace);
		cursor: pointer;
	}
	.seg:not(:last-child) {
		border-right: 1px solid var(--border);
	}
	.seg.active {
		background: var(--accent-bg);
		color: var(--accent);
	}

	.commit-filters {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 0.6rem;
	}
	.chip-row {
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem;
	}
	.type-chip {
		font-family: var(--lens-font-mono, monospace);
		font-size: var(--lens-font-size-2xs, 0.68rem);
		padding: 0.15rem 0.5rem;
		border-radius: 999px;
		cursor: pointer;
		background: transparent;
		border: 1px solid color-mix(in srgb, var(--ct, var(--text-3)) 35%, var(--border));
		color: var(--text-2);
	}
	.type-chip:hover {
		background: color-mix(in srgb, var(--ct, var(--text-3)) 10%, transparent);
	}
	.type-chip.active {
		background: color-mix(in srgb, var(--ct, var(--text-3)) 22%, transparent);
		border-color: var(--ct, var(--text-3));
		color: var(--ct, var(--text-1));
	}
	.commit-subject {
		font-family: var(--lens-font-mono, monospace);
		font-size: var(--lens-font-size-xs, 0.8rem);
		color: var(--lens-text, #f4f6fa);
		word-break: break-word;
	}
	.commit-tasks {
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
		margin-top: 0.35rem;
	}
	.commit-task {
		font-size: var(--lens-font-size-2xs, 0.68rem);
		padding: 0.05rem 0.4rem;
		border-radius: 999px;
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 14%, transparent);
		border: 1px solid color-mix(in srgb, var(--lens-accent, #6ea8ff) 40%, var(--border));
		color: var(--lens-accent, #93c5fd);
		text-decoration: none;
		white-space: nowrap;
	}
	.commit-task:hover {
		background: color-mix(in srgb, var(--lens-accent, #6ea8ff) 24%, transparent);
	}
</style>
