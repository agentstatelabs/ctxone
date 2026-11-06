import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, waitFor } from '@testing-library/svelte';

vi.mock('$lib/plansApi', () => ({ getTask: vi.fn(), editTask: vi.fn() }));

import { editTask, getTask, type Task } from '$lib/plansApi';
import TaskDetailPanel from './TaskDetailPanel.svelte';
import { buildGraph } from './model';

const getMock = vi.mocked(getTask);
const editMock = vi.mocked(editTask);

function task(overrides: Partial<Task> = {}): Task {
	return {
		id: 't-001',
		title: 'Ship it',
		status: 'in_progress',
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

/** The listing copy of `full`: `has_notes` in place of the notes. */
function listing(full: Task): Task {
	const { notes, ...rest } = full;
	return notes ? { ...rest, has_notes: true } : rest;
}

function props(listed: Task, extra: Record<string, unknown> = {}) {
	return {
		task: listed,
		graph: buildGraph([listed]),
		planName: 'lens',
		branch: 'main',
		onClose: vi.fn(),
		onNavigate: vi.fn(),
		onStart: vi.fn(),
		onComplete: vi.fn(async () => true),
		onAbandon: vi.fn(async () => true),
		onNotesSaved: vi.fn(),
		...extra
	};
}

function textarea(r: { getByLabelText: (label: string) => HTMLElement }): HTMLTextAreaElement {
	return r.getByLabelText('Task notes') as HTMLTextAreaElement;
}

beforeEach(() => {
	getMock.mockReset();
	editMock.mockReset();
});

describe('TaskDetailPanel notes', () => {
	it('reads the task on its own and shows its notes as plain text', async () => {
		const full = task({ notes: 'line one\n<b>not bold</b>\n\nline four' });
		getMock.mockResolvedValue(full);
		const r = render(TaskDetailPanel, props(listing(full)));

		const shown = await r.findByTestId('task-notes');
		expect(getMock).toHaveBeenCalledWith('lens', 't-001', 'main');
		expect(shown.textContent).toBe('line one\n<b>not bold</b>\n\nline four');
		expect(shown.querySelector('b')).toBeNull();
	});

	it('saves edited notes with a PATCH and tells the page', async () => {
		const full = task({ notes: 'old brief' });
		getMock.mockResolvedValue(full);
		editMock.mockResolvedValue({ ...listing(full), changed: ['notes'] });
		const p = props(listing(full));
		const r = render(TaskDetailPanel, p);
		await r.findByTestId('task-notes');

		await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
		const save = r.getByRole('button', { name: 'Save notes' }) as HTMLButtonElement;
		expect(textarea(r).value).toBe('old brief');
		expect(save.disabled).toBe(true);

		await fireEvent.input(textarea(r), { target: { value: 'new brief\nsecond line\n' } });
		expect(save.disabled).toBe(false);
		await fireEvent.click(save);

		await waitFor(() => expect(p.onNotesSaved).toHaveBeenCalledWith('t-001', false));
		expect(editMock).toHaveBeenCalledWith(
			'lens',
			't-001',
			{ notes: 'new brief\nsecond line' },
			'main'
		);
		expect(r.queryByLabelText('Task notes')).toBeNull();
	});

	it("keeps the editor open and shows the Hub's validation error", async () => {
		const full = task({ notes: 'old brief' });
		getMock.mockResolvedValue(full);
		const msg = 'notes contain control or unsafe formatting characters';
		let refuse!: (e: Error) => void;
		editMock.mockReturnValue(new Promise((_, reject) => (refuse = reject)));
		const p = props(listing(full));
		const r = render(TaskDetailPanel, p);
		await r.findByTestId('task-notes');

		await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
		await fireEvent.input(textarea(r), { target: { value: 'bad\u202Etext' } });
		await fireEvent.click(r.getByRole('button', { name: 'Save notes' }));

		// Read-only, not disabled, while saving: disabling it would take the
		// focus away, and the text would have to be clicked again to fix it.
		expect(textarea(r).readOnly).toBe(true);
		expect(textarea(r).disabled).toBe(false);
		refuse(new Error(msg));

		const alert = await r.findByRole('alert');
		expect(alert.textContent).toContain(msg);
		expect(textarea(r).value).toBe('bad\u202Etext');
		expect(textarea(r).readOnly).toBe(false);
		expect(p.onNotesSaved).not.toHaveBeenCalled();
	});

	it('clears the notes after a confirm', async () => {
		const full = task({ notes: 'old brief' });
		getMock.mockResolvedValue(full);
		editMock.mockResolvedValue({ ...listing(task()), changed: ['notes'] });
		const p = props(listing(full));
		const r = render(TaskDetailPanel, p);
		await r.findByTestId('task-notes');

		await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
		await fireEvent.click(r.getByRole('button', { name: 'Clear notes' }));
		expect(editMock).not.toHaveBeenCalled();
		// The re-read after the save hangs: the panel must not wait for it.
		getMock.mockReturnValue(new Promise(() => {}));
		await fireEvent.click(r.getByRole('button', { name: 'Confirm clear' }));

		await waitFor(() => expect(p.onNotesSaved).toHaveBeenCalledWith('t-001', true));
		expect(editMock).toHaveBeenCalledWith('lens', 't-001', { notes: '' }, 'main');
		expect(r.queryByTestId('task-notes')).toBeNull();
		expect(r.getByText('No notes.')).toBeTruthy();
	});

	it('shows a packed title as stored and starts the editor from its brief', async () => {
		const full = task({ title: 'Fix login — it 500s on Safari', notes: null });
		getMock.mockResolvedValue(full);
		const r = render(TaskDetailPanel, props(listing(full)));

		expect(r.getByRole('heading').textContent).toBe('Fix login — it 500s on Safari');
		await r.findByText("No notes yet: this task's brief is still in its title.");
		await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
		expect(textarea(r).value).toBe('it 500s on Safari');
		expect(r.getByText(/Saving takes it out of the title/)).toBeTruthy();
	});

	it('shows the notes of a finished task without an editor', async () => {
		const full = task({ status: 'done', notes: 'what was done' });
		getMock.mockResolvedValue(full);
		const r = render(TaskDetailPanel, props(listing(full)));

		await r.findByTestId('task-notes');
		expect(r.queryByRole('button', { name: 'Edit' })).toBeNull();
	});

	it('keeps the editor and its draft through a plan refresh', async () => {
		const full = task({ notes: 'old brief' });
		getMock.mockResolvedValue(full);
		const p = props(listing(full));
		const r = render(TaskDetailPanel, p);
		await r.findByTestId('task-notes');

		await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
		await fireEvent.input(textarea(r), { target: { value: 'half-typed' } });
		const calls = getMock.mock.calls.length;

		// A refresh hands the panel a fresh copy of the same task.
		const fresh = listing(full);
		await r.rerender({ ...p, task: fresh, graph: buildGraph([fresh]) });

		expect(getMock.mock.calls.length).toBe(calls + 1);
		expect(textarea(r).value).toBe('half-typed');
	});

	it('keeps an open proof form through a plan refresh', async () => {
		getMock.mockResolvedValue(task());
		const p = props(task());
		const r = render(TaskDetailPanel, p);
		await fireEvent.click(r.getByRole('button', { name: '✓ Complete…' }));

		const fresh = task();
		await r.rerender({ ...p, task: fresh, graph: buildGraph([fresh]) });
		expect(r.queryByRole('button', { name: 'Mark done' })).not.toBeNull();
	});

	it('offers a retry when the notes cannot be read', async () => {
		const full = task({ notes: 'brief' });
		getMock.mockRejectedValueOnce(new Error('503 Service Unavailable'));
		getMock.mockResolvedValue(full);
		const r = render(TaskDetailPanel, props(listing(full)));

		const alert = await r.findByRole('alert');
		expect(alert.textContent).toContain('503 Service Unavailable');
		await fireEvent.click(r.getByRole('button', { name: 'Retry' }));
		expect((await r.findByTestId('task-notes')).textContent).toBe('brief');
	});
});
