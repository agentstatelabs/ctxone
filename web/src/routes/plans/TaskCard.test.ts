import { describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import type { Task } from '$lib/plansApi';
import TaskCard from './TaskCard.svelte';
import ListView from './ListView.svelte';
import { buildGraph } from './model';

function task(overrides: Partial<Task> = {}): Task {
	return {
		id: 't-001',
		title: 'Ship it',
		status: 'pending',
		priority: 'medium',
		parent_id: null,
		blocked_by: [],
		assigned_to: null,
		created_at: null,
		created_by: null,
		started_at: null,
		started_by: null,
		completed_at: null,
		completed_by: null,
		abandoned_at: null,
		abandoned_reason: null,
		proof: null,
		...overrides
	};
}

const handlers = {
	onOpen: vi.fn(),
	onStart: vi.fn(),
	onRequestComplete: vi.fn(),
	onRequestAbandon: vi.fn()
};

describe('notes marker', () => {
	it('marks a card whose task has notes', () => {
		const t = task({ has_notes: true });
		const r = render(TaskCard, { task: t, graph: buildGraph([t]), ...handlers });
		expect(r.getByRole('img', { name: 'Has notes' })).toBeTruthy();
	});

	it('leaves a card without notes unmarked', () => {
		const t = task();
		const r = render(TaskCard, { task: t, graph: buildGraph([t]), ...handlers });
		expect(r.queryByRole('img', { name: 'Has notes' })).toBeNull();
	});

	it('marks only the list rows whose tasks have notes', () => {
		const tasks = [task({ id: 't-001', has_notes: true }), task({ id: 't-002' })];
		const r = render(ListView, { tasks, graph: buildGraph(tasks), ...handlers });
		expect(r.getAllByRole('img', { name: 'Has notes' })).toHaveLength(1);
	});
});
