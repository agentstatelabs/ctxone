import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./api', () => ({ hubFetch: vi.fn() }));

import { hubFetch } from './api';
import { addTask, editTask, getTask } from './plansApi';

const fetchMock = vi.mocked(hubFetch);

function ok(body: unknown): Response {
	return new Response(JSON.stringify(body), {
		status: 200,
		headers: { 'Content-Type': 'application/json' }
	});
}

function lastCall(): { path: string; init: RequestInit | undefined } {
	const [path, init] = fetchMock.mock.calls.at(-1)!;
	return { path: path as string, init };
}

beforeEach(() => {
	fetchMock.mockReset();
});

describe('getTask', () => {
	it('reads one task on the given branch', async () => {
		fetchMock.mockResolvedValue(ok({ id: 't-002', notes: 'line 1\nline 2' }));
		const t = await getTask('lens v2', 't-002', 'feat/x');
		expect(lastCall().path).toBe('/api/plans/lens%20v2/tasks/t-002?ref=feat%2Fx');
		expect(t.notes).toBe('line 1\nline 2');
	});
});

describe('editTask', () => {
	it('PATCHes only the fields given, with the ref', async () => {
		fetchMock.mockResolvedValue(ok({ id: 't-002', has_notes: true, changed: ['notes'] }));
		const t = await editTask('p', 't-002', { notes: 'new\nbrief' }, 'main');
		const { path, init } = lastCall();
		expect(path).toBe('/api/plans/p/tasks/t-002');
		expect(init?.method).toBe('PATCH');
		expect(JSON.parse(init?.body as string)).toEqual({ notes: 'new\nbrief', ref: 'main' });
		expect(t.changed).toEqual(['notes']);
	});

	it("throws the Hub's validation message", async () => {
		const msg = 'notes contain control or unsafe formatting characters';
		fetchMock.mockResolvedValue(new Response(msg, { status: 400, statusText: 'Bad Request' }));
		await expect(editTask('p', 't-002', { notes: 'x\u202Ey' })).rejects.toThrow(msg);
	});
});

describe('addTask', () => {
	it('sends notes, not description', async () => {
		fetchMock.mockResolvedValue(ok({ id: 't-003', title: 'Do it', has_notes: true }));
		await addTask('p', { title: 'Do it', notes: 'why\nhow' }, 'main');
		const body = JSON.parse(lastCall().init?.body as string);
		expect(body).toEqual({ title: 'Do it', notes: 'why\nhow', ref: 'main' });
		expect(body).not.toHaveProperty('description');
	});
});
