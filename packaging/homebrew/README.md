# Homebrew tap

CtxOne is distributable via Homebrew once the release workflow publishes
a rendered `ctxone.rb` to the `ctxone/homebrew-tap` repository.

## For users

```bash
brew tap ctxone/tap
brew install ctxone
```

Or in one step:

```bash
brew install ctxone/tap/ctxone
```

## For maintainers

1. **Create the tap repository once:** a GitHub repo at `ctxone/homebrew-tap`
   with the structure:

   ```
   homebrew-tap/
   └── Formula/
       └── ctxone.rb
   ```

2. **Create a PAT** with repo write access for `ctxone/homebrew-tap` and
   save it as the `HOMEBREW_TAP_TOKEN` secret on the main repo.

3. **Enable the `homebrew-tap` workflow** — it runs on every tag push,
   reads the freshly-published GitHub release, computes sha256 sums for
   each macOS / Linux tarball, renders `ctxone.rb.template` with those
   values, and commits it to the tap repo.

## Template substitutions

`ctxone.rb.template` uses placeholders the workflow replaces with real
values:

| Placeholder | Example |
|---|---|
| `{{VERSION}}` | `0.60.0` |
| `{{URL_DARWIN_ARM64}}` | `https://github.com/ctxone/ctxone/releases/download/v0.60.0/ctxone-v0.60.0-aarch64-apple-darwin.tar.gz` |
| `{{SHA_DARWIN_ARM64}}` | `abc123...` |
| `{{URL_DARWIN_X86_64}}` | (matching macOS Intel URL) |
| `{{SHA_DARWIN_X86_64}}` | (sha256) |
| `{{URL_LINUX_X86_64}}` | (Linux x86_64 URL) |
| `{{SHA_LINUX_X86_64}}` | (sha256) |
| `{{URL_LINUX_ARM64}}` | (Linux arm64 URL) |
| `{{SHA_LINUX_ARM64}}` | (sha256) |

The template does NOT include Windows — Homebrew is macOS and Linux only.
Windows users should use `install.ps1`.
