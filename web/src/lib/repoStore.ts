/**
 * Shared writable store for the currently selected ASD repo.
 * All code-lens pages read from this store so switching repos
 * in the sidebar immediately updates every view.
 */
import { writable } from 'svelte/store';

export const selectedRepo = writable<string>('');
