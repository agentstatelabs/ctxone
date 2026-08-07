# Homebrew tap

CtxOne is distributable via Homebrew. Each release publishes a rendered
`ctxone.rb` to the `agentstatelabs/homebrew-ctxone` tap (sourced from
`git.internal.example/agentstategroup/homebrew-ctxone`, mirrored to GitHub).

## For users

```bash
brew tap agentstatelabs/ctxone
brew install ctxone
```

Or in one step:

```bash
brew install agentstatelabs/ctxone/ctxone
```

## For maintainers

The tap is published by `scripts/release.sh`, not by a CI workflow. There is
no `HOMEBREW_TAP_TOKEN` secret and no `homebrew-tap` GitHub Action — those
were removed. `scripts/release.sh` is the single tap publisher.

Prereqs and the full flow live in [`RELEASE.md`](../../RELEASE.md). In short,
the release script:

1. Builds `ctxone-hub` and `ctx` for the four macOS / Linux targets and
   uploads the tarballs to the GitHub release.
2. Patches `Formula/ctxone.rb` in the sibling `../homebrew-ctxone` clone —
   the version field plus all four URL + sha256 pairs — then commits and
   pushes to GitLab. The GitLab → GitHub push mirror replicates within
   seconds so `brew` sees the new formula.

Homebrew covers macOS and Linux only. Windows users should use `install.ps1`.
