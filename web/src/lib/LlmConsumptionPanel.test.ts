import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/svelte';
import LlmConsumptionPanel from './LlmConsumptionPanel.svelte';
import type { SessionSnapshot } from './api';

function snap(overrides: Partial<SessionSnapshot> = {}): SessionSnapshot {
	return {
		session_id: 'alice@example.com',
		session_tokens_used: 0,
		session_tokens_saved: 0,
		total_graph_size_chars: 0,
		total_graph_size_tokens: 0,
		cumulative_ratio: 0,
		llm_input_tokens: 0,
		llm_output_tokens: 0,
		llm_cache_read_tokens: 0,
		llm_cache_create_tokens: 0,
		llm_call_count: 0,
		last_model: null,
		last_provider: null,
		...overrides
	};
}

describe('LlmConsumptionPanel', () => {
	it('renders empty state when no LLM usage reported', () => {
		const { getByTestId } = render(LlmConsumptionPanel, { snapshot: snap() });
		expect(getByTestId('llm-empty')).toBeTruthy();
	});

	it('renders token counts from the snapshot', () => {
		const { getByTestId } = render(LlmConsumptionPanel, {
			snapshot: snap({
				llm_input_tokens: 12500,
				llm_output_tokens: 3200,
				llm_cache_read_tokens: 8900,
				llm_cache_create_tokens: 450,
				llm_call_count: 17,
				last_model: 'claude-sonnet-4.5',
				last_provider: 'anthropic',
				cumulative_ratio: 5.0
			})
		});

		expect(getByTestId('llm-input').textContent).toContain('12,500');
		expect(getByTestId('llm-output').textContent).toContain('3,200');
		expect(getByTestId('llm-cache-read').textContent).toContain('8,900');
		expect(getByTestId('llm-cache-create').textContent).toContain('450');
		expect(getByTestId('llm-calls').textContent).toContain('17');
		expect(getByTestId('llm-last-model').textContent).toContain('claude-sonnet-4.5');
		expect(getByTestId('llm-last-model').textContent).toContain('anthropic');
	});

	it('shows cache hit rate when cache tokens are present', () => {
		const { getByTestId } = render(LlmConsumptionPanel, {
			snapshot: snap({
				// 8900 / (8900 + 12500) ≈ 42%
				llm_input_tokens: 12500,
				llm_output_tokens: 3200,
				llm_cache_read_tokens: 8900,
				llm_call_count: 1,
				last_model: 'claude-sonnet-4.5'
			})
		});

		expect(getByTestId('cache-hit-rate').textContent).toMatch(/42%/);
	});

	it('renders a cost estimate using the per-model multiplier', () => {
		const { getByTestId } = render(LlmConsumptionPanel, {
			snapshot: snap({
				llm_input_tokens: 1_000_000,
				llm_output_tokens: 1_000_000,
				llm_call_count: 1,
				last_model: 'claude-sonnet-4.5',
				last_provider: 'anthropic',
				cumulative_ratio: 3.0
			})
		});

		// claude-sonnet-4.5: 1M input + 1M output = $3 + $15 = $18.00
		expect(getByTestId('cost-estimated').textContent).toContain('$18.00');
	});

	it('shows without-CTXone cost and measured savings when ratio > 0', () => {
		const { getByTestId } = render(LlmConsumptionPanel, {
			snapshot: snap({
				llm_input_tokens: 1_000_000,
				llm_output_tokens: 1_000_000,
				llm_call_count: 1,
				last_model: 'claude-sonnet-4.5',
				cumulative_ratio: 5.0
			})
		});

		const without = getByTestId('cost-without').textContent!;
		expect(without).toMatch(/\$/);
		// Measured savings shown as a percentage reduction. Only input is
		// scaled by the ratio, so the blended % is below the raw (1 − 1/5 = 80%)
		// but still a positive percent.
		const savings = getByTestId('measured-savings').textContent!;
		expect(savings).toMatch(/%/);
		expect(savings).not.toMatch(/×/);
	});

	it('uses the estimated fallback ratio when the session has no recall counters', () => {
		const { getByTestId } = render(LlmConsumptionPanel, {
			snapshot: snap({
				llm_input_tokens: 1_000_000,
				llm_output_tokens: 1_000_000,
				llm_call_count: 1,
				last_model: 'claude-sonnet-4.5',
				cumulative_ratio: 0, // no own recall savings…
				fallback_ratio: 5.0 // …but the workspace aggregate lends one
			})
		});

		const dt = getByTestId('measured-savings').closest('.row')!.querySelector('dt')!;
		expect(dt.textContent).toContain('Estimated savings');
		const savings = getByTestId('measured-savings').textContent!;
		expect(savings).toContain('≈');
		expect(savings).toMatch(/%/);
	});

	it('still shows token savings for an unpriced model (no pricing needed)', () => {
		const { getByTestId, queryByTestId } = render(LlmConsumptionPanel, {
			snapshot: snap({
				llm_input_tokens: 100,
				llm_output_tokens: 50,
				llm_call_count: 1,
				last_model: 'some-obscure-model-v3',
				cumulative_ratio: 4.0
			})
		});

		// Headline savings is token-based → 4× → 75%, shown despite no pricing.
		const savings = getByTestId('measured-savings').textContent!;
		expect(savings).toContain('75%');
		// No dollar figure for an unpriced model, but no scary "cost not tracked".
		expect(queryByTestId('cost-estimated')).toBeNull();
		expect(queryByTestId('cost-missing')).toBeNull();
	});

	it('shows no savings row when there is neither a ratio nor pricing', () => {
		const { queryByTestId } = render(LlmConsumptionPanel, {
			snapshot: snap({
				llm_input_tokens: 100,
				llm_output_tokens: 50,
				llm_call_count: 1
				// no last_model, no ratio
			})
		});

		// Nothing to claim → the row is absent rather than a misleading 0%.
		expect(queryByTestId('measured-savings')).toBeNull();
		expect(queryByTestId('cost-estimated')).toBeNull();
	});
});
