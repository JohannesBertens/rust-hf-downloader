# Release Notes - Version 2.8.0

**Release Date**: 2026-09-28

## New features

### GitHub Releases with prebuilt binaries

Tag pushes no longer stop at workflow artifacts: each `vX.Y.Z` tag now
publishes a **GitHub Release** containing prebuilt binaries for five
platform targets, a `SHA256SUMS` checksum file, and the one-liner
installers.

| Asset triple | Platform |
|---|---|
| `x86_64-unknown-linux-gnu` | Linux x86_64 |
| `aarch64-unknown-linux-gnu` | Linux arm64 (**new**) |
| `aarch64-apple-darwin` | macOS Apple Silicon |
| `x86_64-apple-darwin` | macOS Intel (**new**) |
| `x86_64-pc-windows-msvc` | Windows x64 |

Release assets are downloadable without a GitHub login (workflow artifacts
required one). The release body carries the install one-liners and a manual
download table; GitHub auto-generates the change log from the merged PRs.

### One-liner installer / upgrader

**Linux / macOS** (any POSIX shell):
```bash
curl -fsSL https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.sh | sh
```

**Windows** (PowerShell 5.1 or later):
```powershell
irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1 | iex
```

Both scripts:

- auto-detect OS and architecture and pick the matching release asset
  (clear error messages for unsupported platforms)
- resolve the newest version via GitHub's `releases/latest/download`
  redirect — no api.github.com call, so no rate limiting and no `jq`
  dependency
- verify the downloaded archive's SHA256 against the release's
  `SHA256SUMS` before installing anything
- install without admin rights to `~/.local/bin` (Linux/macOS) or
  `%LOCALAPPDATA%\Programs\rust-hf-downloader` (Windows) and manage the
  user `PATH`
- **upgrade in place when re-run**: same version → no-op, newer version →
  atomic replace (Windows renames a running exe aside first); `--force` /
  `-Force` reinstalls anyway
- support pinning (`--version vX.Y.Z` / `-Version vX.Y.Z`), `--dry-run`,
  `--uninstall`, custom install dirs, and a `RHD_DOWNLOAD_BASE` override
  for mirrors

## Internal

- CI: `release.yml` restructured into a build matrix (stable, per-target
  asset names — required for the `latest/download` redirect) plus a
  release job; Linux arm64 and Intel-mac binaries are cross-compiled on
  native runners (tests still run on the three native targets).
- The installers are exercised by the same scripts users run; both were
  tested end-to-end (install, upgrade, up-to-date no-op, force,
  uninstall, tampered-checksum rejection) on POSIX sh and PowerShell 7.
