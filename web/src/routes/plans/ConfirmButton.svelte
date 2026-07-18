<script lang="ts">
	/**
	 * Two-click confirm for destructive actions. First click arms the
	 * button ("Confirm …?") for 3s; the second click within that window
	 * fires `onconfirm`. Arming is purely local — nothing mutates until
	 * the second click.
	 */
	let {
		label,
		confirmLabel = 'Click again to confirm',
		danger = false,
		disabled = false,
		menuItem = false,
		onconfirm
	}: {
		label: string;
		confirmLabel?: string;
		danger?: boolean;
		disabled?: boolean;
		/** Renders in the flat menu-item style instead of a bordered button. */
		menuItem?: boolean;
		onconfirm: () => void;
	} = $props();

	let armed = $state(false);
	let timer: ReturnType<typeof setTimeout> | null = null;

	function click() {
		if (disabled) return;
		if (!armed) {
			armed = true;
			if (timer) clearTimeout(timer);
			timer = setTimeout(() => (armed = false), 3000);
			return;
		}
		if (timer) clearTimeout(timer);
		armed = false;
		onconfirm();
	}

	$effect(() => () => {
		if (timer) clearTimeout(timer);
	});
</script>

<button
	type="button"
	class:danger
	class:armed
	class:menu-item={menuItem}
	{disabled}
	onclick={click}
>
	{armed ? confirmLabel : label}
</button>

<style>
	button {
		background: var(--lens-surface-raised);
		border: 1px solid var(--lens-border);
		color: var(--lens-text);
		padding: 0.3rem 0.7rem;
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-xs);
		cursor: pointer;
		transition: background var(--lens-dur-fast) var(--lens-ease),
			border-color var(--lens-dur-fast) var(--lens-ease);
		white-space: nowrap;
	}
	button:hover:not(:disabled) {
		border-color: var(--lens-border-strong);
	}
	button:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
	button.danger {
		color: var(--lens-danger);
	}
	button.armed {
		background: var(--lens-warn-tint);
		border-color: var(--lens-warn-border);
		color: var(--lens-warn);
	}
	button.armed.danger {
		background: var(--lens-danger-tint);
		border-color: var(--lens-danger-border);
		color: var(--lens-danger);
	}
	button.menu-item {
		background: transparent;
		border: none;
		text-align: left;
		width: 100%;
		padding: 0.4rem 0.6rem;
		border-radius: var(--lens-radius-sm);
		font-size: var(--lens-font-size-sm);
	}
	button.menu-item:hover:not(:disabled) {
		background: var(--lens-surface-raised);
	}
	button.menu-item.armed {
		background: var(--lens-warn-tint);
	}
	button.menu-item.armed.danger {
		background: var(--lens-danger-tint);
	}
</style>
