# Plan: Add a CLI one-shot download mode (`download` subcommand)

**Status:** implemented on `feature/cli-download`; shipped in **v2.3.0** (see §9–§10)
**Goal:** `rust-hf-downloader download <model_id> [selectors] [flags]` downloads a model
non-interactively with CLI feedback (human progress or JSON Lines), designed for scripts and
AI-agent skill usage. The TUI remains the default when the binary is started with no arguments.

---

## 0. History lesson — why the v1 CLI died (and how this plan avoids it)

The v1.x headless CLI (`--headless` + search/download/list/resume subcommands) was removed in
v2.0.0 (commit `0cc1835`, ~1,730 lines deleted). Stated reasons:

| v1 failure mode | Mitigation in this plan |
|---|---|
| Download-manager bootstrap **duplicated** in `main.rs` (drifted from the TUI's copy) | Extract a single shared `engine::DownloadManager`; TUI *and* CLI consume it (§4.1). No second copy can drift because there is no second copy. |
| CLI **feature sprawl** (search/list/resume/download, 4 subcommands, 1,415 lines) | Scope: **one subcommand, one job** — one-shot download. No search/list/resume. |
| Hand-rolled JSON output | Typed `serde` event enum → `serde_json::to_string` per event; schema snapshot-tested with insta (§6). |
| Stale version string, ignored `--sort` flag | Version comes from `env!("CARGO_PKG_VERSION")` and a unit test pins it; flags are minimal and parser-driven. |
| Completion detection via polling heuristics (`sleep(100ms)`, `consecutive_idle_checks`, string matching on status messages) | Deterministic drain: dropping `download_tx` ends the manager loop; verification drain signal with an in-flight counter (§4.3). |

---

## 1. Current architecture of the queue/download pipeline

### 1.1 Component map (v2.2.0)

```
┌──────────────────────────── TUI (src/ui/) ─────────────────────────────┐
│ App (ui/app/state.rs) owns ALL shared state as Arc<...> fields:       │
│   download_tx/download_rx  mpsc channel (transport = the "queue")     │
│   download_progress        Arc<Mutex<Option<DownloadProgress>>>       │
│   download_queue           Arc<Mutex<QueueState>>       (HUD counter) │
│   download_queue_items     Arc<Mutex<Vec<QueueItemSummary>>> (HUD)    │
│   complete_downloads       Arc<Mutex<HashMap<String, DownloadMetadata>>> │
│   verification_queue/_size Arc<Mutex<Vec<…>>> / AtomicUsize           │
│   verification_progress    Arc<Mutex<Vec<VerificationProgress>>>      │
│   download_registry        Arc<Mutex<DownloadRegistry>> (mirror)      │
│   status_tx/status_rx      mpsc<String> (free-text status lines)      │
│                                                                        │
│ ui/app/downloads.rs: confirm_download() / confirm_repository_download()│
│   → resolve files → seed registry → download_tx.send(msg) ──┐         │
│ render loop: try_lock() the state above, ~50ms event loop    │         │
└──────────────────────────────────────────────────────────────┼─────────┘
                                                               │ DownloadMessage
                                                               │ = (model_id,
                                                               ▼  filename, path, sha256, token, size)
   [Download manager task]  ← spawned inline in App::run() (ui/app.rs)
     loop {
       recv() from download_rx (lock only around recv)
       QueueState.remove(1, size); queue_items.remove(...)
       start_download(DownloadParams{…}).await      // SERIAL: one file at a time
     }

   download::start_download (src/download.rs)  — UI-AGNOSTIC CORE
     1. sanitize filename, build  https://huggingface.co/{id}/resolve/main/{file}
     2. delete stale *.incomplete (restart-from-scratch policy)
     3. if final file exists → registry=Complete → queue_verification → return
     4. download_chunked():
          Range bytes=0-0 probe → total size (+ /raw/ fallback on 404)
          registry insert/update (sync file I/O, global ~/models/hf-downloads.toml)
          chunk calc: size/target_chunks clamped [5MiB, 100MiB]
          spawn num_chunks tasks, semaphore = concurrent_threads (8)
            each: Range GET → seek+write at offset → RATE_LIMITER.acquire
                  → update shared DownloadProgress (200ms throttle)
          await all → rename .incomplete → final
     5. queue_verification(item) if hash known & verification enabled
     6. retries: max_retries × on transient (timeout/connect) errors
     7. errors reported ONLY as status_tx strings ("Error: …", "AUTH_ERROR:{id}")
        → start_download returns ()   ← key gap for CLI exit codes

   [Verification worker task]  ← spawned inline in App::run()
     loop { pop queue → spawn verify_file (semaphore 4) … sleep(100ms) }
     verify_file: spawn_blocking sync-read SHA256 (~2 GiB/s) → registry
     status → "✓ Hash verified" / "✗ Hash mismatch" strings; never terminates
```

### 1.2 Properties that matter for a CLI consumer

- **The engine is already headless-capable.** `start_download(DownloadParams)` depends on
  channels/mutexes, not on the TUI. Progress and status are *observable shared state* — exactly
  what a CLI monitor loop needs.
- **The "queue" is an mpsc channel consumed serially.** One file at a time; parallelism exists
  only *within* a file (8 concurrent chunk connections). CLI should keep this semantics.
- **Completion is not signalled, only observable.** The manager loop runs until the channel
  closes; the TUI never closes it. The verification worker runs forever. There is no
  "everything is done" event — this is the central gap a one-shot CLI must close (§4.3).
- **Outcomes are strings.** Auth failure is `"AUTH_ERROR:{model_id}"`; download failure is
  `"Error: …"`. A CLI needs typed outcomes for exit codes (§4.2).
- **Global mutable config.** `DOWNLOAD_CONFIG` / `VERIFICATION_CONFIG` atomics + `RATE_LIMITER`
  are process-global and fed from `AppOptions` via `App::sync_options_to_config()` (a method on
  the TUI App struct — needs extracting, §4.4).
- **Global file state.** Registry at `$HOME/models/hf-downloads.toml` (sync read-modify-write,
  no locking); config at `$HOME/.config/jreb/config.toml`. CLI and TUI share both — good for
  coherence, racy if run concurrently (§7).
- **URLs are hardcoded** to `https://huggingface.co` in `api.rs` and `download.rs` — blocks
  both mirrors and hermetic integration tests (§4.5).

---

## 2. Feature spec — `download` subcommand

### 2.1 Usage

```
rust-hf-downloader download <MODEL_ID> [selectors] [options]

Selectors (what to download):
  --quant <Q>        Quantization type, e.g. Q4_K_M, Q8_0 (GGUF models)
  --file <PATH>...   Exact repo-relative file path (repeatable)
  --all              Entire repository (all files from the tree API)

Options:
  -o, --output <DIR>   Base directory   [default: config default_directory → ~/models]
      --token <TOKEN>  HF token         [default: $HF_TOKEN → config]
      --no-verify      Skip SHA256 verification
      --json           JSON Lines events on stdout (progress → stderr)
  -q, --quiet          No progress output; errors + final summary only
  -h, --help           Help
```

Running the binary with **no arguments still launches the TUI** (dispatch rule in §4.6).

### 2.2 Resolution semantics

Reuse `api::fetch_model_metadata` (recursive tree) and `api::fetch_model_files`
(quantization-aware GGUF listing incl. multipart + subdirs). New pure function:

```rust
// src/cli.rs
struct FileSpec { filename: String, size_bytes: u64, sha256: Option<String> }

enum ResolveError {
    Ambiguous { available: Vec<FileSpec> },   // actionable, structured
    NoFilesMatch { selector: String, available: Vec<FileSpec> },
    ApiError(String),
}

fn resolve_files(metadata: &ModelMetadata,
                 quants: &[QuantizationGroup],
                 selector: Selector) -> Result<Vec<FileSpec>, ResolveError>;
```

Rules:
- `--file` — exact matches against `metadata.siblings`; unknown path → `NoFilesMatch` with the
  available list.
- `--quant` — case-insensitive match against quant groups' `quant_type`; 0 hits →
  `NoFilesMatch` (list available quant types).
- `--all` — all files with a size (directories filtered, same rule as the TUI today).
- No selector — succeed only if the repo has exactly **one** downloadable file; otherwise fail
  with `Ambiguous` including the full structured file list. Never silently download a whole
  repo or "guess" a quant — for AI-skill usage a machine-readable ambiguity error beats a
  40 GB surprise. The error JSON includes the available files so an agent can re-invoke with
  the right selector in one round-trip.
- GGUF multipart archives are separate files with individual SHA256s (existing semantics —
  they are NOT concatenated); `--quant` naturally selects all parts of that quant.

### 2.3 Output

**Human mode (default):** progress to **stderr**, summary to **stdout**.

```
$ rust-hf-downloader download bartowski/Qwen2.5-7B-GGUF --quant Q4_K_M
Resolving bartowski/Qwen2.5-7B-GGUF … 3 file(s), 4.61 GB
[1/3] Qwen2.5-7B-Q4_K_M-00001-of-00002.gguf  47%  █████████░░░░░░  2.2/4.6 GB  62.4 MB/s  eta 40s
[1/3] downloaded in 78s → ~/models/bartowski/Qwen2.5-7B-GGUF/…
[1/3] ✓ verified sha256
…
Done: 3 file(s), 4.61 GB → 2 downloaded, 1 skipped (exists), 3 verified, 0 failed
```

Single-line `\r` progress rewrites, only when stderr `is_terminal()` (std::io::IsTerminal,
stable 1.70 — fine for the 1.75 pin). Reuses `utils::format_size` / `format_number`.

**JSON mode (`--json`):** NDJSON events on **stdout** (one JSON object per line, flushed
immediately); progress events go to stderr or are suppressed (`--json` implies progress only
as events, one per file per 500ms throttle). Stable, additive-only schema:

```jsonc
{"type":"resolved","model":"…","files":[{"filename":"…","size_bytes":123,"sha256":"…"}],"total_bytes":456}
{"type":"download_start","filename":"…","index":1,"count":3,"size_bytes":123}
{"type":"progress","filename":"…","downloaded_bytes":1,"total_bytes":123,"speed_mbps":62.4,"percent":0.8}
{"type":"file_complete","filename":"…","status":"downloaded"|"already_exists","bytes":123}
{"type":"verification_start","filename":"…"}
{"type":"verification_result","filename":"…","ok":true}
{"type":"verification_result","filename":"…","ok":false,"expected_sha256":"…","actual_sha256":"…"}
{"type":"done","summary":{"files":3,"downloaded":2,"skipped":1,"verified":3,"failed":0,"total_bytes":4947802324}}
{"type":"error","code":"ambiguous|no_files_match|auth_required|not_found|network|hash_mismatch","message":"…","available":[…]}
```

The `error` event is **always the last line** when the run fails; scripts can `tail -1 | jq`.
Event enum is a real serde type (§6 keeps it snapshot-tested — no hand-rolled JSON this time).

### 2.4 Exit codes

| Code | Meaning |
|---|---|
| 0 | All requested files present on disk (downloaded or already existed); verification passed or skipped |
| 1 | Download failed after retries, or any hash mismatch |
| 2 | Authentication required (gated repo / bad token) |
| 64 | Usage error or resolution ambiguity (`EX_USAGE` convention; clap errors are intercepted via `try_parse()` so it doesn't emit its default `2` and collide with auth) |
| 130 | Interrupted by SIGINT (§5 Phase 5) |

### 2.5 Auth token resolution order

`--token` flag → `$HF_TOKEN` → config file (`~/.config/jreb/config.toml`) → anonymous.
(Note: `AppOptions::default()` already seeds from `$HF_TOKEN`; the file value currently
*replaces* it in `load_config()` — the CLI applies the explicit precedence order above so a
flag/env always beats the file.)

---

## 3. What can be reused (inventory)

| Component | Verdict | Notes |
|---|---|---|
| `download::start_download` + chunked engine (`download_chunked`, per-chunk tasks, semaphore, rate limiter hooks) | **Reuse, small refactor** | Change return `()` → `FileOutcome` (§4.2); URL via `api_base()` (§4.5) |
| `DownloadParams` / `DownloadMessage` tuple + mpsc channel transport | **Reuse as-is** | CLI speaks the same queue protocol as the TUI |
| `DOWNLOAD_CONFIG` / `VERIFICATION_CONFIG` atomics, `RATE_LIMITER` | **Reuse as-is** | Fed from `AppOptions` via extracted `apply_options` (§4.4) |
| `api::fetch_model_files`, `fetch_model_metadata`, `fetch_multipart_sha256s`, multipart/quant parsing | **Reuse as-is** (+`api_base`) | This is the whole file-resolution layer |
| `registry.rs` (load/save/incomplete/complete) | **Reuse as-is** | Shared file with TUI; CLI downloads appear in the TUI's resume/complete views |
| `verification.rs` worker + `queue_verification` + `verify_file` | **Reuse, +drain counter** | In-flight counter for deterministic completion (§4.3) |
| `http_client.rs` (auth client builder) | **Reuse as-is** | |
| `config.rs` / `AppOptions` | **Reuse as-is** | Config file drives engine tuning for CLI too |
| `download::validate_and_sanitize_path`, `sanitize_path_component` | **Reuse as-is** | Path-traversal safety for free |
| `utils::format_size` / `format_number` | **Reuse as-is** | CLI progress rendering |
| `models.rs` types (`DownloadProgress`, `QueueState`, `VerificationQueueItem`, …) | **Reuse as-is** (+ new enums §4) | |
| Manager loop currently inline in `App::run` | **Extract → `engine.rs`** | Both frontends consume one implementation |
| `App::sync_options_to_config` | **Extract → free fn** | §4.4 |
| `ui/*` render/events/state | **Untouched** | Regression risk ≈ zero if the extraction is mechanical |

**Not reused / not resurrected from v1:** search/list/resume subcommands, `--dry-run`,
`ProgressReporter`, `HeadlessError`, examples/headless. The one v1 idea worth keeping is the
exit-code discipline, now driven by typed outcomes instead of string parsing.

---

## 4. What needs to change

Ordered so each step compiles and the full suite stays green (small PRs, one per phase).

### 4.1 Extract the download manager → `src/engine.rs` (NEW ~180 lines)

Today the manager loop is spawned inside `App::run()`; the removed v1 CLI **duplicated** it in
`main.rs` and they drifted. This time there is exactly one implementation:

```rust
// src/engine.rs
pub struct ManagerDeps {          // the Arc bundle App already holds today
    pub download_rx: DownloadReceiver,
    pub download_queue: Arc<Mutex<QueueState>>,
    pub download_queue_items: Arc<Mutex<Vec<QueueItemSummary>>>,
    pub download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    pub complete_downloads: Arc<Mutex<CompleteDownloads>>,
    pub status_tx: mpsc::UnboundedSender<String>,
    pub verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    pub verification_queue_size: Arc<AtomicUsize>,
}

pub struct ManagerHandle {
    join: tokio::task::JoinHandle<Vec<FileOutcome>>,   // resolves when channel closed+drained
    deps: ManagerDeps,                                  // cloneable shared state for monitors
}

pub fn spawn_manager(deps: ManagerDeps) -> ManagerHandle {
    // exactly the loop from App::run(), plus: collects FileOutcome per file
}
```

- `App::run` is changed to build `ManagerDeps` from its own fields and call
  `spawn_manager` (the TUI keeps its `download_tx` alive forever → manager runs until exit —
  same behavior as today, minus the inline copy).
- The CLI builds the same bundle, sends N messages, **drops `download_tx`**, and
  `manager_handle.join.await` yields `Vec<FileOutcome>` — deterministic completion, no polling,
  no sleep hacks.
- Verification worker spawn is *also* centralized in `engine.rs`
  (`spawn_verification_worker(deps)`) so neither frontend re-implements it.

### 4.2 `start_download` returns a typed outcome (`src/download.rs` + `src/models.rs`)

```rust
// src/models.rs
#[derive(Debug, Clone)]
pub enum FileOutcome {
    Complete { filename: String, bytes: u64 },
    AlreadyExists { filename: String, verified: bool },
    AuthRequired { model_id: String },
    Failed { filename: String, reason: String },
}
```

`pub async fn start_download(params: DownloadParams) -> FileOutcome` — the ~6 early-return
sites that currently only `status_tx.send("Error: …")` / `send("AUTH_ERROR:…")` additionally
return the corresponding variant. **Strings keep flowing** (TUI UX unchanged, popup logic
untouched); the return value is additive. The manager collects outcomes; the TUI ignores them,
the CLI maps them to events/exit codes.

### 4.3 Deterministic verification drain (`src/verification.rs`)

Problem: "all verifications finished" is currently only observable by polling
`verification_queue_size == 0 && verification_progress.is_empty()` — racy, because the worker
removes an item from the queue *before* spawning `verify_file` (which registers progress).
The v1 CLI papered over this with `sleep(100ms)` + `consecutive_idle_checks`.

Fix (minimal, race-free):
- Add `in_flight: Arc<AtomicUsize>` to the worker.
- In the pop branch, **while still holding the queue lock**: `in_flight.fetch_add(1)` before
  removing the item, then spawn; the spawned task does `fetch_sub(1)` on exit (use a guard
  struct so panics decrement too).
- Expose `fn verification_idle(&self) -> bool { queue_size == 0 && in_flight == 0 }` on the
  engine's state bundle.

After `manager.join` resolves, every `queue_verification` call has already happened (they are
awaited inside `start_download`, which the manager awaits), so the CLI can then simply
`wait_until(verification_idle)` — a closed, race-free condition.

### 4.4 Extract `apply_options(&AppOptions)` (`src/download.rs` or `src/config.rs`)

Move the body of `App::sync_options_to_config()` to a free function; `App` delegates to it.
The CLI calls `apply_options(&options)` after merging flags (§2.5), so engine tuning
(threads, chunks, retries, rate limit, verification) follows the config file — exactly like the
TUI.

### 4.5 `HF_ENDPOINT` base-URL indirection (`src/api.rs`, `src/download.rs`)

```rust
pub fn api_base() -> String {
    std::env::var("HF_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://huggingface.co".into())
        .trim_end_matches('/').to_string()
}
```

Replace the ~8 hardcoded `https://huggingface.co/...` format!s in `api.rs`
(list/filter/tree/metadata) and `download.rs` (`/resolve/main/` + `/raw/main/` fallback — note
the fallback's `url.replace("/resolve/main/", "/raw/main/")` must keep working, so build both
from `api_base()`). Two wins: hermetic integration tests against a local mock server (§6), and
mirror support (`HF_ENDPOINT=https://hf-mirror.com`) for free — same convention as
`huggingface_hub`.

### 4.6 CLI frontend (`src/cli.rs` NEW ~300–400 lines incl. tests) + dispatch (`src/main.rs`)

```rust
// src/main.rs (after)
#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = cli::Cli::try_parse();          // clap derive, one subcommand
    match cli {
        Err(e) => { e.print(); std::process::exit(64); }
        Ok(cli::Cli { command: Some(cmd) }) => {
            // No color_eyre/ratatui/mouse setup on this path
            cli::run(cmd).await;              // exits with §2.4 codes
        }
        Ok(cli::Cli { command: None }) => tui_main().await,
    }
}
```

`cli::run` orchestration (in order):
1. `load_config()` → merge `--token`/`-o` overrides → `apply_options` (§4.4); `--no-verify`
   → `DOWNLOAD_CONFIG.enable_verification.store(false)`.
2. Resolve: `fetch_model_metadata` + (if GGUF) `fetch_model_files` → `resolve_files` (§2.2) →
   emit `resolved` event. On `ResolveError` → `error` event + exit 64.
3. Pre-register files in the registry + validate paths — reuse the same sequence as
   `App::confirm_download` (registry seeding, `validate_and_sanitize_path` per file, model
   subdir layout `base/author/model/…`). Consider extracting that ~40-line seeding block into a
   shared helper `engine::register_pending(files, base, token)` so TUI and CLI cannot diverge
   (same drift lesson as §4.1).
4. Build `ManagerDeps`, `spawn_manager`, send all `DownloadMessage`s, drop `download_tx`.
5. Monitor loop (single task, `tokio::select!`): every 200–500ms snapshot
   `download_progress` (try_lock) + drain `status_rx` → render progress (human or JSON
   throttled); intercept `AUTH_ERROR:` strings defensively for messages, but **decisions come
   from `FileOutcome`** (§4.2).
6. `join.await` → emit `file_complete` events; then wait `verification_idle` (§4.3) while
   streaming verification progress (from `verification_progress` vec + the "✓/✗" status lines
   — upgrade: emit `verification_result` events from the outcomes/registry diff).
7. Emit `done`/`error` + exit with mapped code.

Verification results for JSON events: after drain, compare registry entries before/after
(`Complete` → `HashMismatch` transition) — or extend `verify_file` to return its result into a
small `mpsc<VerifyOutcome>` that both frontends may ignore. Prefer the mpsc (typed, no
diffing), it is ~15 lines.

### 4.7 Files touched summary

```
src/engine.rs      NEW    manager + verification-worker bootstrap, drain signals (~180)
src/cli.rs         NEW    args (clap), resolve_files, orchestration, reporters (~350)
src/main.rs        MOD    dispatch (~25 → ~45 lines)
src/models.rs      MOD    +FileOutcome, +VerifyOutcome, +Selector/FileSpec
src/download.rs    MOD    start_download → FileOutcome; api_base URLs
src/api.rs         MOD    api_base() (~10 lines + format! swaps)
src/verification.rs MOD   in_flight counter + result channel (~25)
src/config.rs      MOD    apply_options extraction (move only)
src/ui/app.rs      MOD    App::run delegates to engine::spawn_manager (-40 lines)
src/ui/app/state.rs MOD   sync_options_to_config delegates (move only)
Cargo.toml         MOD    +clap (lean features), +dev-deps hyper 0.14 (server) + serde_json
```

Dependency notes:
- `clap = { version = "4.5", default-features = false, features = ["std","help","usage","error-context","suggestions","derive"] }` — MSRV 1.74 ≤ project pin 1.75. Lean features keep the v1 "clap cost" objection small; `--help` quality is worth it for AI-skill discoverability. (Fallback if MSRV/pinning bites: `pico-args` — decision point, not a blocker.)
- `serde_json` — direct dep again (it is already in the tree via reqwest; typing the event enum needs it).
- `hyper 0.14 {server,tcp,http1}` **dev-dependency only** for the mock server — version-aligned with the lockfile (reqwest 0.11 → hyper 0.14), so no new code ships in the binary.

---

## 5. Implementation phases

Each phase = one PR, compiles green, suite green.

| Phase | Content | Effort |
|---|---|---|
| **1. Engine extraction** | §4.1 + §4.4 + §4.2 + §4.3 refactors; TUI behavior bit-identical; existing 56+ tests pass; add engine unit tests (drain determinism, idle-signal race regression) | ~1 day |
| **2. HF_ENDPOINT** | §4.5; unit test for base override; TUI unchanged | ~0.5 h |
| **3. CLI core** | §4.6 args + resolution + orchestration + **human** reporter + exit codes | ~0.5 day |
| **4. JSON reporter** | event enum + `--json` + throttle + final-line error contract | ~0.5 day |
| **5. Signals & polish** | SIGINT (§below), `is_terminal` gating, token precedence fix, version-pin test | ~0.5 day |
| **6. Integration harness** | §6 mock server + end-to-end tests | ~1 day |
| **7. Docs** | README "CLI" section (~40 lines, incl. JSON schema + AI-skill example), CHANGELOG, AGENTS.md architecture line for `engine.rs`/`cli.rs`, `examples/cli/skill-snippet.md` | ~0.5 h |

**SIGINT semantics (Phase 5):** on first Ctrl+C, set a cancel flag; the manager stops taking
new items, the in-flight file's chunk tasks are aborted, `.incomplete` files are swept, and we
exit 130 with a `done`/`error` event listing unfinished files. Registry entries for unfinished
files stay `Incomplete` — re-running the same command restarts them from scratch (existing
restart-from-scratch policy; there is no cross-run range-resume today and this feature does
not add it). MVP shortcut if time-boxed: plain "exit 130 immediately" — document that
`.incomplete` files are cleaned on next run anyway.

---

## 6. Testing strategy

### 6.1 Unit tests (in-module, `cargo test --lib`)

- `resolve_files`: single-file repo (implicit OK), ambiguity error lists all files,
  `--quant` hit/miss/case-insensitivity, `--file` exact/multiple/unknown, `--all` skips
  directories, multipart handling. No network — pure function over fixtures.
- CLI parsing: defaults, flag precedence (`--token` vs env vs file with HOME isolation —
  pattern already used in `config.rs` tests), `--file` repetition, **version string ==
  `env!("CARGO_PKG_VERSION")`** (the v1 drift regression).
- Exit-code mapping table `FileOutcome`/`ResolveError` → code.
- `api_base()`: default, env override, trailing-slash trimming (HOME/env isolation).
- Engine: manager resolves when `download_tx` dropped; outcomes collected in order;
  `verification_idle` regression test for the remove-vs-spawn race (spawn worker against a
  queue with one item; assert idle is never true between pop and progress-registration —
  deterministic now because `in_flight` is incremented under the queue lock).
- JSON events: `insta` snapshots of every event type rendered via `serde_json` (schema
  stability guard; insta is already a dev-dep).

### 6.2 Integration tests — `tests/cli_download.rs` (NEW)

Real binary via `env!("CARGO_BIN_EXE_rust-hf-downloader")` + `hyper` mock HF server + env
isolation:

- `HF_ENDPOINT=http://127.0.0.1:<port>` — mock serves:
  - `GET /api/models/{id}` → metadata JSON (fixture)
  - `GET /api/models/{id}/tree/main[/{path}]` → file list with `lfs.oid` = SHA256 of fixture
    bytes and real sizes
  - `GET /{id}/resolve/main/{file}` with **Range support** → slices of in-memory bytes
    (this is what the chunked engine speaks)
  - `GET /{id}/raw/main/{file}` for the 404-fallback path
- `HOME=<per-test tmpdir>` isolates: registry (`$HOME/models/hf-downloads.toml`), config
  (`$HOME/.config/jreb/config.toml`), default download dir. **Key trick:** write a small
  `config.toml` into the fake HOME with `min_chunk_size = 1024` (etc.) so tiny fixtures still
  exercise the **multi-chunk** code path without new CLI flags.
- Per-test unique tmp dirs (existing pattern in `config.rs`/`verification.rs` tests) so
  `cargo test` parallelism is safe.

Cases:
1. Happy path: single GGUF file, multi-chunk, correct sha256 → exit 0, bytes on disk exact,
   `file_complete` + `verification_result ok:true` + `done` events, registry `Complete`,
   final layout `base/author/model/file`.
2. Already exists: pre-place file → exit 0, `status:"already_exists"`, re-verified.
3. Hash mismatch: tree advertises oid of different bytes → exit 1, mismatch event with both
   hashes, registry `HashMismatch`.
4. Gated/401 on resolve → exit 2, `auth_required` error event.
5. Ambiguity: repo with several files, no selector → exit 64, `error.available` lists files.
6. `--quant` and `--all` selection correctness (N files queued).
7. Transient failure: server drops connection mid-body once → retry path → success (fixture
   via configurable "kill after K bytes" handler).
8. `--no-verify` → no verification events, exit 0.
9. JSON contract: every stdout line parses; last line on failure is the `error` event.
10. 404 → `/raw/` fallback path exercised (resolve returns 404, raw serves pointer content).

### 6.3 Script/AI-skill level

- `tests/` or `examples/cli/`: a bash + `jq` script run in CI (or as an ignored-by-default
  `#[ignore]` test) that consumes `--json` output the way a skill would — this validates the
  *consumer* contract, not just producer output.
- Manual smoke matrix: `binary` (TUI still launches), `binary download …` in a pipe
  (`--json` output unaffected by non-tty stderr), `cargo clippy --all-features -- -D warnings`,
  `cargo fmt --check`.

### 6.4 Regression safety

- Phases 1–2 are behavior-preserving refactors of TUI code paths; existing insta TUI snapshots
  and 56+ unit tests gate them. If any snapshot changes, the extraction was not mechanical —
  investigate rather than accept.

---

## 7. Risks & open decisions

| Risk | Assessment | Mitigation |
|---|---|---|
| Registry file race (CLI + TUI concurrently, or two CLI runs) | Read-modify-write on a shared TOML, no locking | Document as limitation; entries keyed by URL so worst case is a lost status update, not corruption of downloads. Optional follow-up: atomic write (temp+rename) + advisory lock (`fs2`) |
| TUI regression during engine extraction | Medium likelihood, high impact | Phase 1 is mechanical-only, snapshot tests gate it; keep `App` field ownership unchanged (engine borrows the Arc bundle, does not own it) |
| Verification drain race reintroduced | Low after §4.3 | Dedicated regression test (§6.1); the invariant is structural (counter under queue lock), not heuristic |
| clap MSRV vs Rust 1.75 pin (Ubuntu 22.04) | Low (clap 4.5 = 1.74) but the project has had to pin transitive deps before | Pin `clap = "=4.5.x"` + lockfile update; fallback `pico-args` (decision point at Phase 3) |
| `AUTH_ERROR` string protocol remains load-bearing for TUI popup | It stays untouched; CLI treats strings as display-only | Outcome enum is the CLI's only decision source |
| Duplicate-download coherence (file exists → skip) already handled by engine | None — reused as-is | Exit 0 + `already_exists` is script-friendly |
| Hidden base-path assumption: registry lives under `$HOME/models` even when `-o` is elsewhere | Pre-existing behavior; CLI inherits it | Accept; note in README (registry location is global, download dir is per-invocation) |
| Filename URL-encoding for paths with spaces/special chars | Pre-existing (reqwest normalizes most); not introduced by this change | Note as known behavior; future hardening |

**Open decisions to confirm before Phase 3:**
1. clap vs pico-args (recommendation: clap lean).
2. Should `--json` progress events default to 500ms throttle or be configurable (`--progress-interval-ms`)? (Recommendation: fixed 500ms, no flag — YAGNI.)
3. Implicit single-file download when repo has exactly one file: allowed (recommended) or always require explicit selector?
4. Binary name/subcommand naming: `download` (recommended; matches v1 muscle memory).

---

## 8. Non-goals / future work

- No search/list/resume/queue-status subcommands (the TUI and plain `curl`-able HF APIs cover
  these; revisit only if a skill needs them).
- No cross-run range resume of `.incomplete` files (engine restarts from scratch by design).
- No concurrent-file downloads (queue is serial by design; parallelism is intra-file).
- Later candidates: `--include/--exclude` globs, `verify` subcommand for on-disk models,
  registry file locking, JSON schema published in `docs/`.

---

## 9. Implementation record (2026, branch `feature/cli-download`)

All phases implemented; 106/106 tests green (93 unit + 13 integration), clippy/fmt clean.

| Phase | Commit | Notes |
|---|---|---|
| 1+2 engine + HF_ENDPOINT | `refactor: extract shared download engine` | verbatim manager extraction; FileOutcome; in-flight counter; apply_options; api_base/resolve_url |
| 3–5 CLI core + JSON + signals | `feat(cli): one-shot download subcommand` | clap lean features; streaming outcome/verify channels added to the engine so file_complete events don't wait for full drain; SIGINT aborts manager, exit 130 |
| 6 integration harness | `test(cli): end-to-end integration tests` | hyper mock (Range-aware); flushed out 3 real bugs, see below |
| 7 docs | `docs:` | README CLI section, changelog Unreleased, AGENTS.md architecture |

**Deviations from the draft:**

1. `FileOutcome::AlreadyExists` carries `bytes` instead of a `verified: bool` — verification
   results arrive as typed `VerifyOutcome`s on the new verify channel instead.
2. Added `outcome_tx`/`outcome_rx` to `EngineState` (not in the draft): per-file outcomes stream
   as each file finishes so JSON `file_complete` events do not wait for the manager to drain.
3. Integration tests surfaced and fixed three latent bugs (each regression-tested):
   - `validate_and_sanitize_path` falsely rejected a not-yet-existing base directory
     (first-run bug that also affected the TUI path when `~/models` was missing);
   - hash-mismatch writes now go to the **disk** registry (the in-memory mirror could be empty
     in CLI runs and was saved over the file);
   - CLI tallies double-counted fast downloads (streamed outcomes re-added after the join
     recount); the authoritative recount now runs after the final channel drain.
4. JSON `progress` throttle is 500 ms fixed (per the draft's recommendation); the monitor tick
   is 400 ms.
5. `dl` alias added for the subcommand; summary gained a `hash_mismatch` field (additive).

---

## 10. Follow-up: `search` subcommand (added on this branch)

Originally listed under non-goals (§8), search was added after the download
loop shipped, once the skill-flow gap was concrete: agents could *download*
(self-describing via the `ambiguous` error) but had to drop to `curl` to
*discover* model IDs. Design constraints (from the v1 post-mortem in §0):

- **Query-only**: one bounded API call via the existing `fetch_models_filtered`;\
  no engine state, no channels, no new bootstrap — immune to the v1 drift class.
- **Drift-proof mapping**: CLI `--sort`/`--direction` parse into local value\
  enums with unit-tested `From` impls into the shared `models::SortField`/\
  `SortDirection` (v1's `--sort` was silently ignored; here that is a test failure).
- **Output rule**: queries emit one JSON **array** (`--json`); NDJSON events\
  remain pipeline-only (`download`). Failure prints a single error event line.
- **Config parity**: unspecified flags fall back to `default_sort_*`/\
  `default_min_*` — same defaults as the TUI toolbar.
- **Exit contract**: successful query with zero hits exits 0 with `[]`.
- `fetch_models_filtered` gained a `limit` parameter (clamped 1..=500) so\
  `--limit` actually reaches the API instead of truncating afterwards.

Coverage: flag→enum mapping/aliases/precedence unit tests, JSON array insta
snapshot, and integration tests against the mock `/api/models` endpoint
(filters, client-side name sort both directions, limit forwarded, empty
results, usage errors exit 64, network error exit 1).
