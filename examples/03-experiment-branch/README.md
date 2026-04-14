# Example 03 — Experiment branch workflow

Use CtxOne's branching to try a risky memory change (bulk prime, new
pinned set, reorganization) without touching `main`. Diff the two and
decide whether to keep it.

## What it does

1. Creates an experiment branch from main.
2. Runs some operation on the experiment branch (the example primes a
   large doc as pinned).
3. Diffs main vs experiment to see what changed.
4. Runs a recall on both branches to compare output quality.
5. Leaves you to decide: keep, discard, or merge.

## Run it

```bash
./experiment.sh ./docs/ARCHITECTURE.md
```

Or with any markdown file:

```bash
./experiment.sh ~/some-big-doc.md
```

## Sample output

```
=== Creating branch 'exp-20260414-1430' ===
Branch 'exp-20260414-1430' created from 'main' at sg_e762...

=== Priming on the experiment branch ===
pinned 7 sections from ./docs/ARCHITECTURE.md under source 'experiment'

=== Diff main vs experiment ===
+ AddKey       /memory/pinned/experiment/the-one-sentence-pitch
+ AddKey       /memory/pinned/experiment/why-o-log-n-beats-flat-memory-files
+ AddKey       /memory/pinned/experiment/the-four-kinds-of-memory
+ AddKey       /memory/pinned/experiment/how-recall-ranks-results
...

14 changes

=== Recall on main ===
No memories found for 'recall ranking'

10 pinned + 0 topic matches, 0 tokens sent (flat would be ~0, 0.0x savings)

=== Recall on experiment ===
[PINNED] How recall ranks results
  When you (or an agent) runs `recall "topic"`, here's what the Hub does:
  ...

10 pinned + 0 topic matches, 620 tokens sent (flat would be ~1250, 2.0x savings)

=== Decision time ===
The experiment branch is at: exp-20260414-1430

To KEEP: merge it to main (currently manual via the engine)
To DISCARD: just switch back — export CTX_BRANCH=main
```

## Why this is useful

- **Safe experimentation** — you can break things without breaking main
- **A/B comparison** — recall the same topic on both branches, compare
- **Cheap iteration** — branches are metadata, not data copies
- **Blame preserved** — diff shows exactly what the experiment added or
  changed

## Caveats

- There's no `ctx merge` command yet. Merging an experiment back to main
  requires direct engine access, or you can use the experiment branch
  indefinitely by setting `CTX_BRANCH=exp-...`.
- `ctx diff` output is structural (which paths changed) not semantic
  (what the values mean).
