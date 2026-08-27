# Cutting a CTXone release

Releases are built and published by CI. `scripts/release.sh vMAJOR.MINOR.PATCH`
bumps the version, tags, and pushes — it **builds nothing**. Pushing the tag is
the entire trigger; `.github/workflows/release.yml` produces every platform
build, the GitHub release, and the Homebrew formula.

There is exactly **one publisher** on purpose. The script used to cross-compile
and upload the tarballs itself, duplicating what CI already did on every tag —
two publishers racing on one release is how you get assets whose `sha256`s
disagree with what CI built (the asd v0.9.38 incident).

## What ships where

- **Source:** `git.internal.example/agentstategroup/ctxone`
  → mirrored to `github.com/agentstatelabs/ctxone` (read-only).
- **Release artifacts:** `github.com/agentstatelabs/ctxone-releases/releases`
  (a tarball per target, plus a release entry per version).
- **Homebrew tap:** `git.internal.example/agentstategroup/homebrew-ctxone`
  → mirrored to `github.com/agentstatelabs/homebrew-ctxone`. End-user
  command: `brew tap agentstatelabs/ctxone && brew install ctxone`.

## One-time prereqs

Nothing is compiled locally, so there is no toolchain to install — no extra
`rustup` targets, no `cross`, no Docker, and no sibling tap clone. CI holds the
credentials that publish the release and the formula.

```sh
# gh CLI — only to WATCH the run; the release is created by CI, not from here.
gh auth status
```

## Cutting a release

From a clean working tree on `main`, level with `origin/main`:

```sh
scripts/release.sh v1.0.8
```

The script:

1. **Preflight** — refuses a dirty tree, or a branch behind `origin/main`
   (releasing from a stale main either fails the push or quietly reverts
   upstream commits).
2. **Bumps the version in lockstep** across `Cargo.toml`, `Cargo.lock`,
   `bindings/python/pyproject.toml`, `web/package.json` and
   `website/package.json`, runs `cargo check --workspace --release`, and commits
   as `release: vX`. All five must agree or the `version-guard` CI job fails the
   tag pipeline.
3. **Tags** `vX` on HEAD (annotated), reusing an existing tag only if it already
   points at HEAD.
4. **Pushes `main` + the tag to GitLab only.** Never push the tag to GitHub by
   hand: GitLab's `publish-github` job is fail-closed on `scripts/leak-scan.sh`,
   and a push from a workstation bypasses that gate, putting unscanned commits
   on the public mirror. GitLab CI mirrors them, and that mirror is what fires
   the GitHub release workflow.

Then watch CI — nothing further runs locally:

```sh
gh run watch -R agentstatelabs/ctxone
```

Release entry: `https://github.com/agentstatelabs/ctxone-releases/releases/tag/vX`

Once CI has published, `brew upgrade ctxone` picks up the new formula.

> `CHANGELOG.md` is **not** written by the script. Add the entry by hand before
> cutting the tag.

## After the release: bump the site footer

The marketing site carries a hardcoded version string that nothing derives
from this repo. It will not update itself.

In `CTXone-site`, `website/src/components/SiteFooter.astro`:

```html
<div class="version">CTXone v1.0.0</div>
```

Bump it, commit, push. The site deploys in two hops (GitLab CI mirrors to
GitHub, GitHub Actions builds Pages), so confirm the live page rather than
the pipeline — a green pipeline only means the mirror landed.

This is not hypothetical: agentstategraph.dev advertised `0.9.21` while the
real release was `0.9.24`, stale by three patches, because this step had no
home in a checklist.

## Partial / recovery flags

| env var | effect |
|---------|--------|
| `SKIP_SYNC_CHECK=1` | don't require being level with `origin/main` (offline, or a deliberate out-of-band tag) |
| `SKIP_BUMP=1` | tag HEAD as-is without touching the five version files |

There are no build-related flags any more — the script does not build, so there
is no target subset to select and no upload to re-run. **To re-publish assets,
re-run the GitHub Actions workflow**; do not upload them from a workstation.

## Traps and rollback

- **`brew upgrade` won't downgrade.** If the new version is *less than* the
  installed version (semver), `brew upgrade ctxone` is a no-op. Use
  `brew reinstall ctxone` (and remove a stale
  `/opt/homebrew/Cellar/ctxone/<ver>.reinstall` keg if `brew reinstall` errors
  with "Could not rename ctxone keg").
- **Mirror lag for the formula.** GitLab → GitHub usually replicates within
  seconds, but if you need the formula on GitHub *now*, the script does an
  immediate `git push origin main`; you can also force-trigger via the
  GitLab API:
  `POST /projects/<id>/remote_mirrors/<mirror_id>/sync` with a `PRIVATE-TOKEN`.
- **Rolling back a release.** `gh release delete v<X> -R agentstatelabs/ctxone-releases`
  removes assets + the release entry. Tag removal:
  `git push origin :refs/tags/v<X>` on both source and tap.

- **A dev binary pinned in the launchd plist survives `brew upgrade`.** While
  testing an unreleased hub, `~/Library/LaunchAgents/com.ctxone.hub.plist` may
  point `ProgramArguments[0]` at a locally built binary (e.g.
  `~/.ctxone/bin/ctxone-hub-dev`) instead of `/opt/homebrew/bin/ctxone-hub`.
  Brew then upgrades the Cellar while the *running service keeps the old dev
  build* — `brew list --versions ctxone` looks right and the hub reports a stale
  version, which reads as a failed upgrade. After releasing, point the plist
  back at `/opt/homebrew/bin/ctxone-hub`, carry over any
  `EnvironmentVariables` the dev run added (e.g. `CTXONE_REQUIRE_IDENTITY`),
  then reload:

  ```sh
  # confirm what the service is ACTUALLY running
  ps -o command= -p "$(launchctl list | awk '/com.ctxone.hub/{print $1}')"

  # after editing the plist back to the brew path:
  launchctl unload ~/Library/LaunchAgents/com.ctxone.hub.plist   # SIGTERM -> stats flush
  launchctl load   ~/Library/LaunchAgents/com.ctxone.hub.plist
  curl -s localhost:3001/api/health
  ```

  Always stop the hub with `launchctl unload`, never `kill -9`: session token
  stats flush on graceful shutdown (and every 30s), so a hard kill loses
  everything since the last flush.

## What the script does *not* do

- Build anything. Every artifact comes from `.github/workflows/release.yml`.
- Cut a new homepage on `agentstatelabs/ctxone-site`.
- Write a CHANGELOG entry — bump `CHANGELOG.md` by hand before running.

CI itself does not yet cross-build for `musl` libc (the Linux tarballs target
glibc), nor sign or notarize the macOS binaries. Those are the next-mile
improvements if/when they become worth automating.
