# Plan: Cross-platform path resolution (config, registry, downloads) for all environments

**Status:** implemented on `feature/cross-platform-paths`; shipped in **v2.6.0**
**Goal:** Replace the three `$HOME`-hardcoded, `/tmp`-fallback path sites with a
centralized `src/paths.rs` module backed by the `dirs` crate, so config,
registry, and default downloads resolve correctly on **Linux, macOS, and
Windows**. Adds env-var overrides and a portable mode (run-from-USB). Also
hardens `sanitize_path_component` against Windows-illegal names.
**Provenance:** adapted from fork `phlurblepoot/rust-hf-downloader-streamline-docker`
commit `840b892` ("feat: add Windows compatibility with portable-mode path
resolution"), re-applied onto current main (v2.5.0); the fork's base predates
v2.0.0 and cannot be merged directly.

---

## 0. Why now

v2.5.0 is the **first release shipping Windows and macOS binaries** (CI
artifacts). On Windows the current code is broken by construction:

- `$HOME` is unset → `unwrap_or_else(|_| "/tmp")` → config, registry, and
  downloads all land in `C:\tmp` (or fail).
- `format!("{}/.config/jreb/config.toml", home)` builds mixed-separator paths.
- `sanitize_path_component` accepts Windows-illegal characters
  (`< > : " | ? *`), ASCII control chars, and reserved device names
  (`CON`, `NUL`, `COM1`…), which break file creation or worse.

Local Windows verification is impossible without an MSVC toolchain
(`ring`/`backtrace` need `lib.exe`), so correctness must come from
platform-appropriate path resolution + tests that CI runs per-OS.

## 1. Current-state inventory (v2.5.0, exact sites)

| # | Site | Current behavior | Problem on Windows/macOS |
|---|------|------------------|--------------------------|
| 1 | `src/config.rs:6-9` `get_config_path()` | `$HOME/.config/jreb/config.toml`, fallback `/tmp/.config/...` | HOME unset → `/tmp`; macOS convention is `~/Library/Application Support` |
| 2 | `src/registry.rs:6-9` `get_registry_path()` | `$HOME/models/hf-downloads.toml`, fallback `/tmp/models/...` | HOME unset → `/tmp` |
| 3 | `src/models.rs:401` `AppOptions::default()` | `default_directory: format!("{}/models", home)` | Same; also the *default shown in the options screen* |

Affected tests: `src/config.rs:137-153` isolate the real `HOME` via
`set_var`/`remove_var` — works, but couples tests to process env and races
with any future parallel env-dependent test.

Note the existing asymmetry: **config** lives under `~/.config/jreb/` while
**registry + downloads** live under `~/models/`. The module preserves this
split (config root ≠ data root) — it does not relocate anything on Linux.

## 2. Target resolution order

For both roots, highest priority first:

1. **Env override** — `RUST_HF_DOWNLOADER_CONFIG_DIR` / `RUST_HF_DOWNLOADER_DATA_DIR`
   (non-empty values only). Also the test-isolation mechanism (replaces HOME
   mutation in tests).
2. **Portable mode** — if a `config.toml` exists next to the running
   executable: exe dir is the config root, `<exe_dir>/models` is the data
   root. Enables USB-stick / arbitrary-folder use of the v2.5.0+ binaries.
3. **Platform defaults** via `dirs`: config → `dirs::config_dir()/jreb`;
   data (registry + default downloads) → `dirs::home_dir()/models`.
4. **Last-resort fallback** — `std::env::temp_dir()/jreb` resp. `/models`
   (never the bare temp root).

Resulting user-visible locations:

| Root | Linux | macOS | Windows |
|------|-------|-------|---------|
| config | `~/.config/jreb/config.toml` *(unchanged)* | `~/Library/Application Support/jreb/config.toml` *(moves from `~/.config/jreb` — see P2)* | `%APPDATA%\jreb\config.toml` |
| data | `~/models/` *(unchanged)* | `~/models/` *(unchanged)* | `%USERPROFILE%\models\` |

**Decision (maintainer, 2026-09-26): the config directory stays `jreb` on all
platforms.** Rationale: `~/.config/jreb` is the existing Linux location (since
v0.9.0); a single `APP_DIR` constant keeps one code path and simple docs; on
Windows/macOS (new platforms, no legacy) the short brand name is acceptable.
Do not "fix" this to `rust-hf-downloader` — changing it later would require a
legacy-read migration on every platform.

## 3. Phases

### P1 — `src/paths.rs` module + rewire the three sites

- Add dependency `dirs = "6"`.
- New module `src/paths.rs`:
  - Public API (kept identical to the fork for review-diffability):
    `config_dir()`, `config_path()`, `data_dir()`, `default_download_dir()`,
    `registry_path()`, consts `ENV_CONFIG_DIR`, `ENV_DATA_DIR`.
  - Helpers `env_override(var) -> Option<PathBuf>` (empty string ignored),
    `portable_mode() -> Option<PathBuf>` (exe dir iff `config.toml` marker
    exists there).
  - **Refinement over the fork:** the resolution core is a pure function
    `resolve(env_cfg, env_data, portable) -> (PathBuf /*config*/, PathBuf /*data*/)`
    so unit tests pass values directly instead of mutating process env.
    The public functions are thin wrappers reading env/`current_exe()`.
- Rewire (mechanical, one line each + remove dead `home` code):
  - `config.rs::get_config_path()` → `paths::config_path()`
  - `registry.rs::get_registry_path()` → `paths::registry_path()`
  - `models.rs::AppOptions::default()` → `default_directory:
    paths::default_download_dir().to_string_lossy().into_owned()`
  - Register `mod paths;` in `main.rs`.
- Tests:
  - Fork's suite (filename assertions, env-override precedence, empty-env
    ignored) ported onto the pure `resolve()` core where possible.
  - **Linux no-regression test**: with HOME set and no overrides,
    `config_path()` still ends `~/.config/jreb/config.toml` and
    `registry_path()` ends `~/models/hf-downloads.toml`.
  - Rewrite `config.rs:137-153` HOME-isolation tests to use
    `RUST_HF_DOWNLOADER_CONFIG_DIR` instead of `HOME`.
- **Acceptance:** `cargo test` green; existing Linux users see identical
  paths (no config/registry migration needed on Linux); `cargo clippy`
  clean; version-gated `PathBuf::join` everywhere (no `format!` paths).

### P2 — macOS legacy-config compatibility read

- `config_path()` gains a read-compat step: if the resolved path does not
  exist **and** `~/.config/jreb/config.toml` exists (the pre-v2.6 macOS
  location), return the legacy path for **reads**; writes continue to the
  new location (first save migrates the file).
- Windows unaffected (no legacy users — paths never worked). Linux
  unaffected (same path). Scope: macOS crates.io users on < v2.6.0.
- Test: simulate both locations via env override + temp dirs; assert
  legacy wins when only it exists, new wins when both exist.

### P3 — `sanitize_path_component` hardening (Windows-safe names)

Port the fork's diff onto `src/download.rs:51+` (function unchanged since
the fork's base, expected to apply cleanly):

- Reject ASCII control chars (`0x00-0x1F`, `0x7F`) and `< > : " | ? *`
  on **all** platforms (consistency; these never appear in real HF paths).
- Reject Windows reserved device names — `CON PRN AUX NUL COM1-9 LPT1-9`,
  case-insensitive, with or without extension (`con.gguf`, `LPT3.gguf`) —
  via stem comparison.
- Fork's unit tests verbatim.
- **Acceptance:** all existing sanitizer tests still pass (no behavior
  change for legitimate HF filenames, incl. `.gitattributes` dotfiles).

### P4 — CI test step, docs, release

- `.github/workflows/release.yml`: insert `cargo test --locked` before the
  build step — keeps CI tag-only/zero-inter-release-load while finally
  running the (now cross-OS meaningful) test suite on all three OSes.
- Docs:
  - `README.md`: per-platform path table (§2), env-var overrides, portable
    mode under Installation.
  - `TROUBLESHOOTING.md`: Windows section — HF_TOKEN persistence via env
    var, SmartScreen/unsigned-binary note, ANSI terminal requirement
    (Windows Terminal), and the macOS config relocation note.
  - `AGENTS.md`: `paths.rs` in the module map + resolution-order summary
    (future agents must route all path decisions through it, never
    re-hardcode `HOME`).
- Release per CONTRIBUTING "Release Process": bump **v2.6.0** (minor —
  additive feature), changelog entries, PR → merge → tag → CI artifacts →
  `cargo publish`.

## 4. Verification matrix

| Check | Where | Gate |
|-------|-------|------|
| Unit tests (pure resolver, sanitizer) | local Linux | `cargo test` |
| No-Linux-regression (paths identical) | local Linux + CI ubuntu | unit test in P1 |
| Cross-OS compile | CI tag workflow (3 OSes) | build + `cargo test --locked` |
| Windows runtime smoke | CI artifact, manual | run exe → config/registry under `%APPDATA%`/`%USERPROFILE%`; portable mode via `config.toml` next to exe |
| macOS legacy migration | unit test (P2) + manual spot check | legacy config read on first run |

## 5. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Existing configs bake `default_directory: "/home/x/models"` | Harmless (absolute path still valid); options screen shows it; users can edit |
| macOS config relocation confuses users | P2 compat read + TROUBLESHOOTING note |
| Portable mode triggers unexpectedly (stray `config.toml` next to exe) | Only for explicitly-placed marker file; documented |
| `dirs` adds a dependency | Small, no transitive bloat, de-facto standard; `cargo tree -i dirs` review in PR |
| Env-mutating tests race in parallel threads | Pure `resolve()` core avoids env mutation; remaining env tests use distinct keys |

## 6. Out of scope

- Intel-mac (`x86_64-apple-darwin`) CI targets
- Full XDG spec beyond what `dirs` provides
- HF cache-layout (`~/.cache/huggingface`) compatibility/sharing
- Long-path (>260 char) manifest work on Windows — note in TROUBLESHOOTING only
