# Token Savings — How the Ratio Is Computed

CTXone's core pitch is **fewer tokens per turn, more useful context**. This
doc explains how the savings are measured, how to interpret the ratio, and
how to maximize it.

For the business case and enterprise math, see [TOKEN_ECONOMICS.md](TOKEN_ECONOMICS.md).
For the architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

## The baseline: flat memory

Imagine you stored everything CTXone knows as a single JSON file and shipped
that file as context to the model on every turn. That's the **flat memory
baseline**.

Concretely, the Hub computes the baseline by:

1. Serializing the entire memory graph at `/` to JSON
2. Measuring the character count of that string
3. Dividing by 4 (the standard tokens-per-character estimate for English)

```python
flat_tokens = len(json.dumps(graph_root_as_nested_dict)) / 4
```

This is an upper bound on what "carry everything on every turn" would cost.

## The actual cost: per-response

Every time a recall runs, the Hub measures the length of the response it
actually returned (pinned sections + topic matches) and divides by 4. That's
`ctx_tokens_sent`.

```python
sent_tokens = len(response_string) / 4
```

## The ratio

```
savings_ratio = flat_tokens / sent_tokens
```

A ratio of `13.0x` means: for this specific query, you sent 13× fewer tokens
than you would have with flat memory.

The response includes three fields:

```json
{
  "ctx_tokens_sent": 34,
  "ctx_tokens_estimated_flat": 451,
  "ctx_savings_ratio": 13.26
}
```

## How to read the numbers in `ctx` commands

After a recall, the CLI prints:

```
5 pinned + 2 topic matches, 34 tokens sent (flat would be ~451, 13.3x savings)
```

- **`5 pinned + 2 topic matches`** — how much the Hub returned and why
- **`34 tokens sent`** — actual cost of this response
- **`flat would be ~451`** — what carrying the whole graph would cost
- **`13.3x savings`** — the ratio for this single recall

`ctx stats` shows the cumulative session totals:

```
CTXone Token Savings
  graph size:   451 tokens
  tokens sent:  98
  tokens saved: 1706
  savings:      18.4x
```

- **`graph size`** — the flat-memory baseline right now
- **`tokens sent`** — total tokens returned across all recall / context /
  remember calls in this Hub session
- **`tokens saved`** — `(number_of_recalls × flat_size) - tokens_sent`
- **`savings`** — overall ratio

**Important caveat:** the Hub tracks *Hub-session* totals, not LLM-session
totals. It resets when you restart the Hub. `ctx stats` gives you the
running cumulative savings for the current running Hub process.

## What drives the ratio up

**Tight queries.** `recall "BSL-1.1 licensing"` is tighter than
`recall "stuff about our project"`. The more specific, the fewer incidental
matches, the higher the ratio.

**Small pinned set.** Pinned memories take half the budget. If you pin 50
sections, each recall ships 50 sections — still a huge savings vs flat, but
lower ratio than pinning just 5.

**Large total graph.** The bigger your graph, the more dramatic the savings.
A 10,000-fact graph recalling 3 facts is a 3,000× savings even with a
generous pinned set.

**Focused contexts.** `ctx remember "..." --context licensing` groups facts
into `/memory/licensing/*`. Recalls can then hit that one sub-tree cleanly.

## What drives the ratio down

**Very small graph.** If you have 5 facts total and recall them all, the
ratio will be near 1.0. That's expected — savings don't kick in until you
have more than you need.

**Overly-broad recall queries.** `recall "project"` on a project-heavy
graph matches everything and returns everything. Same tokens as flat.

**Over-pinning.** If you pin your entire README plus five other docs,
pinned content alone is near-flat. The budget math works: pinned takes half,
topic gets the other half, but if pinned is already huge, "half" is huge.

## A concrete example

Start with the demo data:

```bash
$ ctx demo
...
Seeded 21 facts.
  recall "licensing"    →  2 matches, 34 tokens sent vs 451 flat (13.0x savings)
  recall "architecture" →  1 matches, 13 tokens sent vs 451 flat (32.8x savings)
  recall "tokens"       →  1 matches, 26 tokens sent vs 451 flat (17.4x savings)
  recall "Lens"         →  1 matches, 25 tokens sent vs 451 flat (17.5x savings)
```

21 facts total = 451 flat tokens. A recall returning just the relevant 1–2
facts averages 24 tokens. Ratio: 18.4× cumulative.

Now prime a pinned doc:

```bash
$ ctx prime ./docs/VISION.md --pin --source project
pinned 5 sections from ./docs/VISION.md under source 'project'
```

Recall the same topics:

```bash
$ ctx recall "licensing"
[PINNED] The Insight
  ...
[PINNED] The Product
  ...
[PINNED] The Roadmap
  ...

--- topic matches ---
CTXone is licensed under BSL-1.1...
The engine (AgentStateGraph) is BSL-1.1...

5 pinned + 2 topic matches, 620 tokens sent (flat would be ~1191, 1.9x savings)
```

The ratio dropped to 1.9×. That's not a bug — you're now carrying the entire
VISION.md on every call, which is the price you pay for having critical
project context always available. The ratio is still >1.0, meaning you're
still saving tokens vs pure flat memory, and every response carries the
context the agent actually needs.

**Rule of thumb:** ratio > 5× means pinned is tight and recall is focused.
Ratio < 2× means you've pinned a lot and should review whether every pinned
section is really critical.

## Why 4 tokens per character?

It's a rough estimate. Actual tokenization depends on the model and the
content (code tokenizes differently than prose). 4 chars/token is the
standard back-of-envelope for English text used by most model providers.

For precise accounting you'd need to call the model's tokenizer. CTXone
uses 4 because:

1. It's fast (no tokenizer dependency)
2. It's conservative enough that the reported "tokens sent" is in the right
   order of magnitude
3. Both sent and flat use the same estimator, so the *ratio* is accurate
   even if the absolute numbers are rough

If you need exact counts, run `ctx recall --exact`. The CLI re-tokenizes
the response locally using tiktoken's cl100k_base encoding (GPT-3.5 /
GPT-4 family) and prints both the fast estimate and the exact count
side by side:

```
0 pinned + 2 topic matches, 34 tokens sent (flat would be ~451, 13.0x savings)
  exact (cl100k_base): 75 sent, 553 flat, 7.4x savings
```

The exact numbers are often **smaller than the 4-char estimate** because
BPE tokenizers compress common words and punctuation efficiently. The
ratio is therefore usually *more conservative* under `--exact`, which
is the right direction — you never want to inflate savings claims.

You can also tokenize arbitrary text directly:

```bash
$ ctx tokens "The quick brown fox jumps over the lazy dog"
43 chars
9 tokens (cl100k_base, exact)
10 tokens (4-char estimate)

$ echo "any text from stdin" | ctx tokens -
```

**Caveat:** cl100k_base is OpenAI's tokenizer. Claude, Gemini, and Grok
use different proprietary tokenizers, so the exact counts won't match
those models byte-for-byte. The ratio is still meaningful as a
consistent reference point.

## Why not vector similarity?

Vector search returns results ranked by embedding distance. That works, but:

- Recall is opaque — you can't tell why a result was returned
- Results drift when you re-embed with a newer model
- Token savings are harder to compute because the "relevance threshold" is
  fuzzy

Structural search + confidence scoring + pinned context gives you:

- Blame-able results (every fact has a commit trail)
- Predictable ranking (token matches + pinned-first)
- Clean token math (you know exactly what went into each response)

See [ARCHITECTURE.md](ARCHITECTURE.md#what-the-hub-is-not) for more on this
design choice.
