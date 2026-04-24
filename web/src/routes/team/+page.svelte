<script lang="ts">
	import { onMount } from 'svelte';
	import {
		getTeamMembers,
		getTeamActivity,
		getTeamSavings
	} from '$lib/teamApi';
	import type { TeamMember, TeamActivityEntry, TeamSavings } from '$lib/teamApi';

	let members: TeamMember[] = $state([]);
	let activity: TeamActivityEntry[] = $state([]);
	let savings: TeamSavings | null = $state(null);
	let loading = $state(true);
	let error: string | null = $state(null);

	async function refresh() {
		error = null;
		try {
			const [m, a, s] = await Promise.all([
				getTeamMembers(),
				getTeamActivity(20),
				getTeamSavings()
			]);
			members = m;
			activity = a;
			savings = s;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load team data';
		} finally {
			loading = false;
		}
	}

	onMount(refresh);

	function timeSince(iso: string): string {
		const diff = Date.now() - new Date(iso).getTime();
		const minutes = Math.floor(diff / 60_000);
		if (minutes < 1) return 'just now';
		if (minutes < 60) return `${minutes}m ago`;
		const hours = Math.floor(minutes / 60);
		if (hours < 24) return `${hours}h ago`;
		return `${Math.floor(hours / 24)}d ago`;
	}

	function fmtTokens(n: number): string {
		if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
		if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
		return String(n);
	}
</script>

<h2>Team</h2>

{#if loading}
	<p class="loading">Loading team data…</p>
{:else if error}
	<p class="error">{error}</p>
{/if}

{#if !loading}
	<div class="team-grid">
		<!-- Members panel -->
		<section class="panel members-panel">
			<h3>Members</h3>
			{#if members.length === 0}
				<p class="empty">No members found.</p>
			{:else}
				<div class="member-list">
					{#each members as member}
						<div class="member-row">
							<div class="member-info">
								<span class="member-id">{member.id}</span>
								<span class="member-kind badge" class:agent={member.kind === 'agent'} class:human={member.kind === 'human'}>
									{member.kind}
								</span>
							</div>
							<div class="member-meta">
								<span class="member-stat">{member.commit_count} commits</span>
								<span class="member-time">{timeSince(member.last_seen)}</span>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<!-- Savings summary -->
		<section class="panel savings-panel">
			<h3>Token Savings</h3>
			{#if savings}
				<div class="savings-card">
					<div class="savings-row big">
						<span class="savings-label">Total tokens saved</span>
						<span class="savings-value saved">{fmtTokens(savings.total_tokens_saved)}</span>
					</div>
					<div class="savings-row big">
						<span class="savings-label">Savings ratio</span>
						<span class="savings-value ratio">{savings.savings_ratio.toFixed(1)}x</span>
					</div>
				</div>
				{#if savings.top_contributors.length > 0}
					<h4>Top Contributors</h4>
					<div class="contributor-list">
						{#each savings.top_contributors as c, i}
							<div class="contributor-row">
								<span class="contributor-rank">#{i + 1}</span>
								<span class="contributor-id">{c.agent_id}</span>
								<span class="contributor-saved">{fmtTokens(c.tokens_saved)} saved</span>
							</div>
						{/each}
					</div>
				{/if}
			{:else if !loading}
				<p class="empty">No savings data available.</p>
			{/if}
		</section>
	</div>

	<!-- Activity feed -->
	<section class="panel activity-panel">
		<h3>Recent Activity</h3>
		{#if activity.length === 0}
			<p class="empty">No activity yet.</p>
		{:else}
			<div class="activity-list">
				{#each activity as entry}
					<div class="activity-row">
						<div class="activity-header">
							<span class="activity-time">{entry.timestamp.slice(0, 19)}</span>
							<span class="activity-agent">{entry.agent_id}</span>
							<span class="activity-category">{entry.category}</span>
						</div>
						<div class="activity-body">
							<span class="activity-path">{entry.path}</span>
							<span class="activity-message">{entry.message}</span>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</section>
{/if}

<style>
	.loading {
		color: #555;
		font-size: 0.9rem;
	}

	.error { color: #ef4444; }

	.empty {
		color: #555;
		padding: 1.5rem;
		text-align: center;
		font-size: 0.9rem;
	}

	.team-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1.5rem;
		margin-bottom: 1.5rem;
	}

	.panel {
		background: #111;
		border: 1px solid #222;
		border-radius: 8px;
		padding: 1.25rem 1.5rem;
	}

	.panel h3 {
		margin: 0 0 1rem 0;
		font-size: 1rem;
		font-weight: 600;
		color: #fff;
	}

	.panel h4 {
		margin: 1rem 0 0.5rem 0;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.07em;
		color: #555;
		font-weight: 600;
	}

	/* Members */
	.member-list {
		display: flex;
		flex-direction: column;
		gap: 0;
	}

	.member-row {
		display: flex;
		justify-content: space-between;
		align-items: center;
		padding: 0.6rem 0;
		border-bottom: 1px solid #1a1a1a;
	}

	.member-row:last-child { border-bottom: none; }

	.member-info {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.member-id {
		font-family: monospace;
		font-size: 0.9rem;
		color: #ccc;
	}

	.badge {
		font-size: 0.7rem;
		padding: 0.1rem 0.45rem;
		border-radius: 3px;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	.badge.agent {
		background: #1e3a5f;
		color: #93c5fd;
	}

	.badge.human {
		background: #1e3e2a;
		color: #86efac;
	}

	.member-meta {
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}

	.member-stat {
		font-size: 0.8rem;
		color: #666;
	}

	.member-time {
		font-size: 0.75rem;
		color: #444;
		font-family: monospace;
	}

	/* Savings */
	.savings-card {
		display: flex;
		flex-direction: column;
	}

	.savings-row {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		padding: 0.5rem 0;
		border-bottom: 1px solid #1a1a1a;
	}

	.savings-row:last-child { border-bottom: none; }
	.savings-row.big { padding: 0.6rem 0; }

	.savings-label { color: #888; font-size: 0.9rem; }
	.savings-value { color: #fff; font-family: monospace; }
	.savings-value.saved { color: #22c55e; }
	.savings-value.ratio { color: #3b82f6; font-size: 1.4rem; font-weight: 700; }

	.contributor-list {
		display: flex;
		flex-direction: column;
		gap: 0;
	}

	.contributor-row {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		padding: 0.4rem 0;
		border-bottom: 1px solid #1a1a1a;
		font-size: 0.85rem;
	}

	.contributor-row:last-child { border-bottom: none; }

	.contributor-rank {
		color: #555;
		font-family: monospace;
		width: 1.5rem;
	}

	.contributor-id {
		font-family: monospace;
		color: #ccc;
		flex: 1;
	}

	.contributor-saved {
		color: #22c55e;
		font-family: monospace;
		font-size: 0.8rem;
	}

	/* Activity */
	.activity-panel {
		margin-bottom: 0;
	}

	.activity-list {
		display: flex;
		flex-direction: column;
	}

	.activity-row {
		padding: 0.6rem 0;
		border-bottom: 1px solid #1a1a1a;
	}

	.activity-row:last-child { border-bottom: none; }

	.activity-header {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		margin-bottom: 0.25rem;
	}

	.activity-time {
		font-family: monospace;
		font-size: 0.75rem;
		color: #555;
	}

	.activity-agent {
		font-family: monospace;
		font-size: 0.8rem;
		color: #93c5fd;
	}

	.activity-category {
		background: #1e3a5f;
		color: #93c5fd;
		padding: 0.1rem 0.45rem;
		border-radius: 3px;
		font-size: 0.7rem;
	}

	.activity-body {
		display: flex;
		align-items: baseline;
		gap: 0.6rem;
		font-size: 0.9rem;
	}

	.activity-path {
		font-family: monospace;
		font-size: 0.8rem;
		color: #666;
		flex-shrink: 0;
	}

	.activity-message { color: #ccc; }
</style>
