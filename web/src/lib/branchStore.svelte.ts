/**
 * Global current-branch store.
 *
 * Svelte 5 runes only work inside .svelte / .svelte.ts files. The rest of the
 * app imports `branchStore` and reads/writes `branchStore.current` directly.
 * Any component referencing `branchStore.current` automatically reacts to
 * changes via Svelte's reactivity.
 */

function createBranchStore() {
	let current = $state('main');

	return {
		get current() {
			return current;
		},
		set current(value: string) {
			current = value;
		}
	};
}

export const branchStore = createBranchStore();
