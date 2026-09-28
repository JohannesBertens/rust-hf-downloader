# Release Notes - Version 2.10.0

**Release Date**: 2026-09-28

## New features

### `rust-hf-downloader update` — built-in self-update

The one-liner installers, expressed in Rust: update the installed binary
without re-running a shell command.

```bash
rust-hf-downloader update          # check, download, verify, swap in place
rust-hf-downloader update --check  # report only; exit code 70 when newer exists
rust-hf-downloader update --json   # NDJSON events for scripts
rust-hf-downloader upgrade         # alias
```

How it works:

- New releases ship a `latest.json` manifest (version + per-platform SHA256),
  served via `releases/latest/download/latest.json` — a GitHub CDN redirect
  with **no api.github.com rate limit** (the same trick the installers use)
- The asset for the running platform is downloaded with progress (human or
  JSON), **verified against the manifest's SHA256**, extracted, and swapped
  in atomically via the `self_replace` crate (rename on Unix; rename-aside on
  Windows — identical semantics to `install.sh` / `install.ps1`)
- Mirrors are first-class: `RHD_UPDATE_BASE` overrides the release base URL
  entirely, matching the installers' `RHD_DOWNLOAD_BASE` contract
- A checksum mismatch discards the download and exits `71` without touching
  the installed binary; `--check` exits `70` when an update exists (usable
  from scripts and the agent skill)
- `--force` reinstalls even when current — the full download/verify/swap path
  is exercised by the e2e suite with a same-version asset

Bootstrap note: this release is the first with `latest.json`; binaries on
older releases keep using the one-liner installers.

### Version badge in the TUI top bar

The filter toolbar now shows the running version (`v2.10.0`, subtle gray)
pinned flush against the right edge of the top bar:

```
│Sort: Downloads ▼  |  Min Downloads: 10.0K  |  Min Likes: 100  |  [Popular]       v2.10.0│
```

- Not part of the clickable filter areas
- On narrow terminals the badge outranks the decorative preset indicator so
  the version stays visible; it is the future home of update notifications

## Internal

- Dependencies: `self-replace` (swap), `flate2`+`tar` (unix extraction),
  `zip` (windows extraction) — extraction codecs are platform-gated
- New e2e suite (`tests/update_e2e.rs`, 6 tests): mock release server; the
  real binary is copied to a temp dir and swaps *itself* — up-to-date,
  `--check` exit 70, full `--force` swap, tampered-checksum rejection with
  binary-untouched assertion, JSON event schema, missing-platform-asset
- Unit tests: strict `VersionTriple` parsing/ordering, manifest serde
  (kebab-case formats, unknown format rejection), target-triple mapping
- Documentation audit: `plans/README.md` rewritten as a complete plans index
  (active / shipped / removed-feature archive) with a keep-it-current
  convention; README and `changelog/README.md` gained the missing 2.8.0 and
  2.9.0 entries; AGENTS.md module maps cover the current sources
