# Release Notes - Version 2.9.0

**Release Date**: 2026-09-28

## New features

### Installers take over existing `cargo install` copies

Users who originally installed via `cargo install rust-hf-downloader` ended
up with two binaries: the cargo copy in `~/.cargo/bin` and the one-liner's
copy in `~/.local/bin` — and because rustup *prepends* `~/.cargo/bin` to
`PATH`, the stale cargo copy kept winning. The installers now resolve this
automatically:

- **In-place takeover**: if `rust-hf-downloader` is found in
  `$CARGO_HOME/bin` (`%USERPROFILE%\.cargo\bin` on Windows), the release
  binary is installed right there — same location, new version, exactly one
  binary. `cargo uninstall rust-hf-downloader` is invoked first so cargo's
  install records (`.crates.toml` / `.crates2.json`) are handed over through
  the sanctioned path instead of going stale (the Cargo Book forbids editing
  them by hand). Without cargo on `PATH`, the binary is simply replaced.
- **cargo-binstall convention**: with no existing copy but `$CARGO_HOME/bin`
  present and on `PATH`, the installers default to it — the canonical bin
  dir for Rust CLI tools, already at the front of `PATH`.
- **Shadow detection (volta-style)**: after installing, the installers
  re-resolve `rust-hf-downloader` the way the shell would (first `PATH`
  match) and print an actionable warning if a different copy (Homebrew,
  scoop, a manually placed binary, …) shadows the new one.

```bash
curl -fsSL https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.sh | sh
```
```powershell
irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1 | iex
```

## Fixes

- `install.ps1`: the post-install PATH resolution no longer depends on
  `Get-Command` (whose PATH cache does not reliably refresh after runtime
  `PATH` updates); it scans `PATH` entries directly, and handles the
  Unix `PATH` vs Windows `Path` variable-name difference.

## Internal

- Both installers were exercised against simulated cargo environments
  (takeover with/without cargo available, binstall-convention default,
  shadow warning, HOME-only fallback, uninstall) on POSIX sh and in a
  PowerShell container.
