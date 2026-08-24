# Cutting a CTXone release

Releases are built locally to keep CI minutes free. The one-command flow is
`scripts/release.sh vMAJOR.MINOR.PATCH`.

## What ships where

- **Source:** `git.internal.example/agentstategroup/ctxone`
  → mirrored to `github.com/agentstatelabs/ctxone` (read-only).
- **Release artifacts:** `github.com/agentstatelabs/ctxone-releases/releases`
  (a tarball per target, plus a release entry per version).
- **Homebrew tap:** `git.internal.example/agentstategroup/homebrew-ctxone`
  → mirrored to `github.com/agentstatelabs/homebrew-ctxone`. End-user
  command: `brew tap agentstatelabs/ctxone && brew install ctxone`.

## One-time prereqs

```sh
# Rust targets
rustup target add x86_64-apple-darwin

# cross-rs for Linux targets (uses Docker images per triple)
cargo install cross --git https://github.com/cross-rs/cross

# Docker Desktop — must be running when you build Linux targets

# gh CLI — must have both accounts auth'd:
#   - agentstatelabs: write access on agentstatelabs/ctxone-releases
#   - any account with write on the homebrew-ctxone tap
gh auth status

# The brew tap clone must exist as a sibling of CTXone:
#   ../homebrew-ctxone
# with origin pointing at GitLab.
git clone ssh://git@git.internal.example:2222/agentstategroup/homebrew-ctxone.git \
  ../homebrew-ctxone
```

## Cutting a release

From a clean working tree on `main`:

```sh
scripts/release.sh v0.9.12
```

The script:

1. Bumps `Cargo.toml` workspace version, commits, tags, pushes to GitLab.
2. Builds `ctxone-hub` and `ctx` for four targets:
   - `aarch64-apple-darwin` (native)
   - `x86_64-apple-darwin` (native cross via Apple toolchain)
   - `x86_64-unknown-linux-gnu` (cross-rs + Docker)
   - `aarch64-unknown-linux-gnu` (cross-rs + Docker)
3. Tarballs each as `ctxone-<ver>-<target>.tar.gz`.
4. Creates the GitHub release and uploads all four tarballs.
5. Patches `Formula/ctxone.rb` in the sibling tap clone (version field + all
   four URL+sha256 pairs), commits, pushes to GitLab. The GitLab → GitHub
   push mirror replicates within seconds.

About ~7 minutes wall-clock the first time; less on incremental rebuilds.

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
| `SKIP_TAG=1` | use HEAD as-is; don't bump Cargo.toml or tag |
| `SKIP_LINUX=1` | only build the two macOS targets |
| `SKIP_FORMULA=1` | leave the brew tap alone (e.g. uploads only) |
| `ONLY_TARGETS=a,b` | build a comma-separated subset |

Example: re-upload just the aarch64-darwin tarball without touching the
formula:

```sh
SKIP_TAG=1 SKIP_FORMULA=1 ONLY_TARGETS=aarch64-apple-darwin \
  scripts/release.sh v0.9.11
```

`gh release upload --clobber` is used, so re-runs overwrite stale assets
rather than 422-ing.

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

## What the script does *not* do

- Cut a new homepage on `agentstatelabs/ctxone-site`.
- Write a CHANGELOG entry — bump `CHANGELOG.md` by hand before running.
- Cross-build for `musl` libc (the current Linux tarballs target glibc).
- Sign or notarize macOS binaries.

These are the next-mile improvements if/when they become worth automating.
