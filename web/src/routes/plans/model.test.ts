import { describe, expect, it } from 'vitest';
import type { Task } from '$lib/plansApi';
import {
	MAX_NOTES_BYTES,
	formatBytes,
	hasNotes,
	notesBytes,
	packedTitle,
	taskMatches
} from './model';

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

describe('hasNotes', () => {
	it('reads has_notes from a listing', () => {
		expect(hasNotes(task({ has_notes: true }))).toBe(true);
		expect(hasNotes(task())).toBe(false);
	});

	it('reads the notes from a task read on its own', () => {
		expect(hasNotes(task({ notes: 'brief' }))).toBe(true);
		expect(hasNotes(task({ notes: null }))).toBe(false);
		expect(hasNotes(task({ notes: '' }))).toBe(false);
	});
});

describe('packedTitle', () => {
	it('splits at the first separator', () => {
		expect(packedTitle('Fix login — it 500s — on Safari')).toEqual({
			title: 'Fix login',
			brief: 'it 500s — on Safari'
		});
	});

	it('is null without a separator or with a blank side', () => {
		expect(packedTitle('Fix login')).toBeNull();
		expect(packedTitle('Fix login—tight dash')).toBeNull();
		expect(packedTitle('  — only a brief')).toBeNull();
		expect(packedTitle('Only a title —  ')).toBeNull();
	});
});

describe('notesBytes / formatBytes', () => {
	it('counts UTF-8 bytes, as the Hub does', () => {
		expect(notesBytes('abc')).toBe(3);
		expect(notesBytes('—')).toBe(3);
		expect(notesBytes('é\n')).toBe(3);
		expect(MAX_NOTES_BYTES).toBe(16384);
	});

	it('formats bytes and KiB', () => {
		expect(formatBytes(512)).toBe('512 B');
		expect(formatBytes(2355)).toBe('2.3 KiB');
		expect(formatBytes(MAX_NOTES_BYTES)).toBe('16.0 KiB');
	});
});

describe('taskMatches', () => {
	it('matches notes when the task carries them', () => {
		expect(taskMatches(task({ notes: 'Check the Safari path' }), 'safari')).toBe(true);
		expect(taskMatches(task({ has_notes: true }), 'safari')).toBe(false);
	});

	it('matches a brief packed into the title', () => {
		expect(taskMatches(task({ title: 'Fix login — Safari 500s' }), 'safari')).toBe(true);
	});
});
