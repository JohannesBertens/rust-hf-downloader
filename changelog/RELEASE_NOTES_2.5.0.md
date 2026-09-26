# Release Notes - Version 2.5.0

**Release Date**: 2026-09-25

## CI: tag-triggered release builds

### Problem
The repository had no CI at all: every release was built by hand on the
maintainer's Linux box, and the Windows/macOS binaries from the
`phlurblepoot/rust-hf-downloader-streamline-docker` fork analysis could not
be reproduced or verified. Local Windows cross-checks are impossible without
an MSVC toolchain (`ring`/`backtrace` need a C compiler), so portability
breakage would only surface in user bug reports.

### Changes
- **`.github/workflows/release.yml`**: pushing a `vX.Y.Z` tag builds
  `cargo build --release --locked` on `ubuntu-latest`, `windows-latest`, and
  `macos-latest` and uploads the binaries as workflow artifacts named
  `rust-hf-downloader-{Linux,Windows,macOS}`.
- **Minimal by design**: no tests, fmt, clippy, or cache in the release path;
  no CI runs between releases (the tag is the only trigger). `--locked`
  builds against the committed `Cargo.lock` for reproducibility.

### Verification
Smoke-tested by pushing a throwaway `v2.4.2-test` tag: all three matrix
jobs succeeded and produced artifacts (Linux 3.7 MB, Windows 3.0 MB,
macOS 3.4 MB). Known quirks handled/documented:

- A tag whose workflow file exists only in the tagged commit (not on any
  remote branch) does **not** trigger the run — tag commits already on
  `main`.
- `macos-latest` runners are arm64; there are no Intel-mac artifacts.

### Documentation
- README: version header bumped, "Prebuilt binaries (CI artifacts)"
  installation section, changelog table entry.
- CONTRIBUTING: "Release Process" (bump → changelog → merge → tag → CI
  artifacts).
- AGENTS.md: CI section for future agents.

### Motivation / credit
The Windows/macOS build gap was identified while analyzing the fork
`phlurblepoot/rust-hf-downloader-streamline-docker` (commit `840b892`),
which added a full 3-OS CI matrix. This release ports the minimal core of
that idea; cross-platform path resolution (`paths.rs`, `dirs` crate) from
the same fork remains a candidate for a future release.
