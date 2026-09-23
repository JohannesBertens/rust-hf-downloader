# Plan: Remove the headless/CLI mode entirely

**Status:** executed 2026-09-23 on branch `test-and-cleanup` (release 2.0.0)
**Branch target:** `test-and-cleanup`
**Goal:** reduce the binary to a pure TUI application by deleting every line of code that exists only for the `--headless` CLI path.

All usage claims below were verified by grep on 2026-09-23 (branch `test-and-cleanup`, commit `386cbc0`).

---

## 1. Inventory — what is CLI-only

| Item | Size | Evidence |
|---|---|---|
| `src/headless.rs` (incl. its 11 unit tests) | 1,415 lines | referenced only from `main.rs` |
| `src/cli.rs` | 75 lines | referenced only from `main.rs` |
| `src/main.rs` headless branch | ~228 of 268 lines | `if cli_args.headless { ... }` block |
| `clap` dependency | Cargo.toml | used only by `cli.rs` + one `clap::Parser` call in `main.rs` |
| `examples/headless/` (4 files) | ci-example.yml, download-examples.sh, search-examples.sh, timing-test.sh | CLI-only examples |
| README "CLI Mode" section | lines 107–305 (~200 lines) + TOC entry + feature bullets | CLI usage docs |
| `models.rs` → `ModelMetadata.gated: serde_json::Value` field | 1 field | only consumer is `headless::check_gated_model` |
| ~total | **~1,730 lines src + docs** | |

**Shared code that stays** (verified used by TUI): `api.rs` (all fetch/parse fns incl. `fetch_models_filtered`), `download.rs`, `verification.rs`, `registry.rs`, `http_client.rs`, `rate_limiter.rs`, `config.rs`, `utils.rs`, `models.rs` types (`QueueState` is also used by `ui/app/state.rs`), all of `ui/`.

**Not newly dead after removal:** nothing else — every other symbol used by the headless branch has a TUI caller. `serde_json` remains a direct dependency only because of the `gated` field and API-test fixtures (see §4 decisions).

## 2. Ordered steps

### Step 1 — `src/main.rs`: strip to TUI-only entry point
Delete `mod cli;` and `mod headless;` declarations and the entire `if cli_args.headless { ... }` block (lines ~20–248). Result (~35 lines):

```rust
mod api;
mod config;
mod download;
mod http_client;
mod models;
mod rate_limiter;
mod registry;
mod ui;
mod utils;
mod verification;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
    use crossterm::execute;
    use std::io::stdout;
    execute!(stdout(), EnableMouseCapture)?;

    let terminal = ratatui::init();
    let result = ui::App::new().run(terminal).await;
    ratatui::restore();

    execute!(stdout(), DisableMouseCapture)?;
    result
}
```

This also drops the CLI-only duplicate of the download-manager bootstrap, SIGINT/SIGTERM handlers, and exit-code plumbing (`headless::EXIT_*`). The TUI builds its own manager/workers in `ui/app.rs` — unaffected.

### Step 2 — delete files
- `git rm src/cli.rs src/headless.rs`
- `git rm -r examples/headless/`

### Step 3 — dependencies
- `cargo remove clap` (also updates Cargo.lock)
- `serde_json`: **keep** as a direct dependency in the minimal variant (still used by `models.rs` `gated` field type and API test fixtures; it remains in the tree via reqwest's `json` feature anyway). See §4 option B if you want it gone too.

### Step 4 — `models.rs`: remove the `gated` field (recommended)
Its only reader was `check_gated_model` (dies in step 2). The struct has no `deny_unknown_fields`, so serde will simply ignore the API's `gated` key on deserialize. Update the ~3 fixture sites that construct `ModelMetadata` in `api.rs` tests (e.g. `gated: serde_json::Value::Null`). After this, `serde_json` is no longer referenced from `src/` outside tests → keep it as dev-only or drop entirely (`cargo remove serde_json`, reqwest keeps it transitively).
  - Trade-off: if a future "gated" badge in the TUI is plausible, keep the field and the direct dep instead. Removing it now is trivially reversible later.

### Step 5 — docs
- README.md: delete the `## CLI Mode` and `### CLI Reference` sections (lines 107–305), the TOC entry (line 63), and reword line 3 + feature bullets from "TUI and CLI" to TUI-only.
- Cargo.toml `description`: `"TUI and CLI for searching and downloading HuggingFace models"` → `"TUI for searching and downloading HuggingFace models"`.
- changelog/: add entry per existing `RELEASE_NOTES_*.md` convention (breaking change: CLI removed).
- plans/README.md: add a link to this plan noting headless was later removed (keep `add-headless.md` as history).

### Step 6 — test fallout & compensation
- The 11 `headless.rs` tests disappear with the file → suite goes **62 → 51**.
- Optional parity: add 3–4 unit tests for `utils::format_size` (the surviving twin of the deleted `headless::format_file_size`) covering the KB/MB/GB boundaries the old tests pinned.
- All 15 snapshot tests and the 23 `api.rs` tests are unaffected (api tests only need the §4 fixture fix).

## 3. Verification checklist
1. `cargo build` — clean
2. `cargo test` — 51/51 pass (or 54–55 with the `format_size` parity tests)
3. `cargo clippy --all-targets` — no new warnings
4. `grep -rn "headless\|clap\|dry_run\|EXIT_" src/` — zero hits
5. `cargo tree -i clap` — error "package ID not found" (fully removed)
6. Manual smoke: `cargo run` launches the TUI

## 4. Decision points
| # | Decision | Recommended |
|---|---|---|
| A | Remove `examples/headless/` entirely vs. keep as reference | **Remove** (git history preserves them) |
| B | `serde_json`: keep direct dep vs. drop with `gated` field | **Drop field + dep** (nothing in `src/` needs it afterwards) |
| C | Replace clap `--version/--help` with a tiny manual handler | Skip — out of scope; cargo metadata still carries the version |
| D | Split into 2 commits (code removal, docs) vs. 1 | 2 commits: `refactor!: remove headless/CLI mode` + `docs: drop CLI documentation` |

## 5. Risks / behavior changes
- **Breaking** for automation users: no more `search`/`download`/`list`/`resume`/`--json`/`--dry-run` — this is the point of the change; call it out in the changelog entry (semver: major bump).
- No functional risk to the TUI: the two paths only share read-only library code; the CLI's parallel download-manager bootstrap in `main.rs` is deleted, the TUI's in `ui/app.rs` is untouched.
- Rollback: single `git revert` of the removal commits; the branch `test-and-cleanup` preserves full history.
