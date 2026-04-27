/**
 * Global current-branch store. Mirrors selection to localStorage so a
 * page refresh lands on the same branch. The layout calls `hydrate(known)`
 * once it has the live branch list — if the persisted branch is gone, we
 * fall back to main.
 */

const KEY = 'ctxone:branch';

function load(): string {
	if (typeof localStorage === 'undefined') return 'main';
	return localStorage.getItem(KEY) ?? 'main';
}

function save(name: string) {
	if (typeof localStorage !== 'undefined') localStorage.setItem(KEY, name);
}

function createBranchStore() {
	let current = $state('main');

	return {
		get current() {
			return current;
		},
		set current(value: string) {
			current = value;
			save(value);
		},
		hydrate(known: string[]) {
			const persisted = load();
			if (known.includes(persisted)) {
				current = persisted;
			} else {
				current = 'main';
				save('main');
			}
		}
	};
}

export const branchStore = createBranchStore();
