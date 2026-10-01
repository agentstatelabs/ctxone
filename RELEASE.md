# Cutting a CTXone release

Releases are built and published by CI. `scripts/release.sh vMAJOR.MINOR.PATCH`
bumps the version, tags, and pushes — it **builds nothing**. Pushing the tag is
the entire trigger: the GitLab tag pipeline mirrors it to GitHub, where
`.github/workflows/release.yml` produces every platform build and the GitHub
release; the same pipeline then renders the Homebrew formula and publishes the
version to the sites. See [What CI does after the tag](#what-ci-does-after-the-tag).

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

Once the `homebrew` job has run (about 10–15 minutes after the tag),
`brew upgrade ctxone` picks up the new formula. Then clear the macOS privacy
prompt straight away — see
[After `brew upgrade` on macOS](#after-brew-upgrade-on-macos-answer-the-privacy-prompt).

> `CHANGELOG.md` is **not** written by the script. Add the entry by hand before
> cutting the tag.

## What CI does after the tag

Everything after the push is the GitLab **tag pipeline**, and nothing in it
needs a hand. The release is fully out when its last job, `site-version`, is
green:

1. **Checks** — `fmt`, `clippy`, `build`, `test`, `frontend`, plus
   `version-guard` (every component version must equal the tag).
2. **`publish-github`** — leak-scans, then mirrors `main` and the tag to
   GitHub. The tag fires `release.yml`, which builds all five targets (~10 min)
   and publishes the release in `ctxone-releases`.
3. **`homebrew`** — starts 10 minutes later (delayed, so it doesn't hold a
   runner), polls for the release assets, renders `Formula/ctxone.rb` with their
   `sha256`s and commits it to the GitLab tap, which mirrors to GitHub.
4. **`site-version`** — after `homebrew`, so the sites never advertise a release
   that isn't installable yet. Moves `ctxone` in
   `agentstatelabs.com/releases.json` forward to the tag (never backward). The
   ctxone.com footer and agentstatelabs.com read that file at page load, so no
   site deploy is needed. The job only exists when `SITE_RELEASES_TOKEN` is set:
   if it's missing from the pipeline, the site was not updated.

Confirm the version the sites will show:

```sh
curl -s https://agentstatelabs.com/releases.json   # "ctxone": "vX"
```

The version in `CTXone-site`'s `website/src/components/SiteFooter.astro`
(`<span data-release="ctxone">`) is only the fallback for visitors whose fetch
of `releases.json` fails. It is not part of the release; refresh it whenever
the site is next touched. Bumping it by hand used to be a release step, and it
drifted: agentstategraph.dev once advertised `0.9.21` three patches after
`0.9.24` shipped.

## After `brew upgrade` on macOS: answer the privacy prompt

macOS ties Files & Folders permission to the binary, and CI does not sign the
macOS builds (see the end of this file), so to macOS every upgraded
`ctxone-hub` is a new app. The first time the new hub reads a `.ctxproject` or
runs git in a repo under `~/Documents`, `~/Desktop` or `~/Downloads`, macOS
shows **"ctxone-hub would like to access files in your Documents folder"**, and
that read **blocks until someone answers**. The dialog is easy to miss behind
other windows.

What you see while it is pending:

- **v1.0.12 and later:** the hub gives up on detection after 3 s
  and every `ctx` command in such a repo stops with
  `can't tell which workspace <dir> belongs to: project detection did not
  finish within 3s …` (exit `75`). Once 16 detections are stuck, the hub
  refuses new ones instantly with `busy`.
- **CLI v1.0.11 and earlier:** the CLI gave up after 1.5 s and silently ran
  against the `default` workspace. Nothing said so; plans and branches came
  back "not found" and writes landed in `default`.

Trigger and clear it as part of the upgrade, not mid-work:

```sh
brew upgrade ctxone
# restart the service so the new binary is the one running (see below)
cd ~/Documents/<any repo with a .ctxproject>
ctx status        # "Namespace: unknown — …did not finish within 3s" = prompt pending
```

Click **Allow**, then rerun `ctx status`: it should name the project. If the
prompt was denied, `ctx status` reports `cannot read …/.ctxproject: Operation
not permitted`. Grant access in **System Settings → Privacy & Security → Files
and Folders → ctxone-hub** (or add the binary under **Full Disk Access**), then
retry. Until then, `--namespace <workspace>` / `CTX_NAMESPACE` skips detection.

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
- **Mirror lag for the formula.** The `homebrew` job commits to the GitLab tap,
  and GitLab → GitHub usually replicates within seconds. If you need the
  formula on GitHub *now*, force the tap's mirror via the GitLab API:
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

- Build or publish anything. The tarballs and GitHub release come from
  `.github/workflows/release.yml`; the formula and the site version from the
  tag pipeline's `homebrew` and `site-version` jobs.
- Cut a new homepage on `agentstatelabs/ctxone-site`.
- Write a CHANGELOG entry — bump `CHANGELOG.md` by hand before running.

CI itself does not yet cross-build for `musl` libc (the Linux tarballs target
glibc), nor sign or notarize the macOS binaries. Those are the next-mile
improvements if/when they become worth automating.
