# Release Notes - Version 2.6.0

**Release Date**: 2026-09-26

## Cross-platform path resolution

Implements `plans/cross-platform-paths.md` (P1–P4), adapted from fork
`phlurblepoot/rust-hf-downloader-streamline-docker` commit `840b892`.

### Problem
All three path sites hardcoded `$HOME` with a `/tmp` fallback
(`src/config.rs`, `src/registry.rs`, `src/models.rs`), so on Windows —
where `$HOME` is unset — config, registry, and downloads all landed in
`/tmp`. Paths were built with `format!` (`/` separators), and the path
sanitizer accepted Windows-illegal characters and reserved device names.
v2.5.0 was the first release shipping Windows binaries, making the gap
user-visible.

### Changes
- **New `src/paths.rs`** — single resolution point for config, registry,
  and default download dir. Order: env override
  (`RUST_HF_DOWNLOADER_CONFIG_DIR` / `RUST_HF_DOWNLOADER_DATA_DIR`) →
  portable mode (`config.toml` next to the exe ⇒ exe dir is config root,
  `<exe>/models` is data root) → `dirs`-crate platform defaults →
  namespaced temp fallback. Decision: the config directory stays `jreb`
  on all platforms (recorded in the plan).
- **Legacy read fallback** — when the canonical config is absent but the
  pre-v2.6 `~/.config/jreb/config.toml` exists, it is read; the next save
  writes the canonical location (automatic migration). Relevant on macOS
  (config moved to `~/Library/Application Support/jreb/`); no-op on Linux
  (identical paths). An explicit env override disables the fallback.
- **Sanitizer hardening** — `sanitize_path_component` additionally rejects
  ASCII control characters, Windows-illegal chars (`< > : " | ? *`), and
  reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`;
  case-insensitive, with or without extension) on every platform, with
  false-positive guard tests (`config.json`, `console.log` pass).
- **Tests** — pure `resolve()` core tested without env mutation;
  Linux no-regression test pins the exact pre-2.6 paths; env-sensitive
  tests serialised through a shared `ENV_MUTEX` (cargo runs tests as
  parallel threads sharing one process env).
- **CI** — release workflow now runs `cargo test --locked` on all three
  OSes before building artifacts.

### User-visible locations

| | Linux | macOS | Windows |
|---|---|---|---|
| Config | `~/.config/jreb/config.toml` *(unchanged)* | `~/Library/Application Support/jreb/config.toml` *(migrated)* | `%APPDATA%\jreb\config.toml` |
| Data | `~/models/` *(unchanged)* | `~/models/` *(unchanged)* | `%USERPROFILE%\models\` |

### Documentation
README per-platform path table + env overrides + portable mode;
TROUBLESHOOTING "Platform-Specific Issues" section (SmartScreen, ANSI
terminals, HF token persistence, long paths, macOS config migration);
AGENTS.md module map entry; CONTRIBUTING release process now includes
`cargo publish`.
