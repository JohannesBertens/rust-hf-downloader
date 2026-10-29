---
status: proposed (feature branch `feature/flare-models`)
---

# FLARE-aware scanning, viewer, downloads, and a `--for flare` serve-path handoff

Date: 2026-10-29 · Feature loop run (grill digest + 3 research lanes, briefs in
`.review/flare-grill/` — session artifacts; the durable facts are inlined below).

**Tier: L** — cross-cutting within one repo (api scanner, TUI viewer, TUI
download flow, CLI preset, new GPU-detection module, docs), four interacting
segments sharing one new `FlareRepoInfo` model; single-run scope, acceptance
pinned by the grill. Not XXL: one repo, no migration, no cross-repo work.

## 0. Grill digest — decisions (user-settled 2026-10-29)

| # | Decision |
|---|---|
| D1 | Drop-in depth = **serve-path handoff**: rhd auto-selects the payload revision, downloads package files with the existing resumable engine into a normal directory, then prints the exact `flare run <path> --gpus <spec>` line (flare's explicit-path mode bypasses its cache). No helios dependency, no writes to `~/.cache/flare/packages`. |
| D2 | GPU→branch = **`nvidia-smi` subprocess + flare's exact PCI device-ID table**; unknown ID / missing nvidia-smi / mixed architectures ⇒ clear error demanding `--revision` (flare's own rule). `CUDA_VISIBLE_DEVICES` integer indices honored. |
| D3 | Viewer = **third display mode** `ModelDisplayMode::Flare` with revision cards. |
| D4 | TUI download = **revision-picker popup** ('d' in Flare mode → popup → Enter enqueues, revision-pinned). |
| D5 | CLI = **`download --for flare` preset + `--gpus` flag** on the existing subcommand. |
| D6 | Acceptance = **offline fixtures + bounded live smoke** (metadata-only network; never the ~107 GB payload). |
| D7 | Hard non-goal: **auto-update tracking** (no flare `sources/`/`origins/` equivalent). Included in minimal form because not excluded: search-list FLARE badge (tags are already in results), publisher identity display in the viewer. |

**Kept out with rationale (user may veto at plan review):** cryptographic
publisher-signature verification (rhd already verifies every file's sha256
against the Hub LFS oid in the resume journal, and flare re-verifies publisher
signatures at load; rhd displays identity only); `hf-cache sync --for flare`
(flare never reads the hub cache — the preset would produce bytes nothing
consumes).

## Research facts the design rests on (inlined from the lane briefs)

- `GET {api}/api/models/{id}/refs` → `{branches:[{name,ref,targetCommit}],tags:[…],converts:[…]}` (camelCase, no pagination). Live-verified on the three FLARE repos.
- `GET {api}/api/models/{id}/tree/{rev}?recursive=true` → per-revision files with true `lfs.size` + `lfs.oid` (sha256) + `xetHash`; `Link: rel="next"` pagination (rhd's `fetch_recursive_tree` already implements this loop).
- `GET {api}/{repo}/raw/{rev}/{path}` returns pointer-free content for non-LFS files (config.json, publisher.json).
- `/api/models/{id}` `siblings` reflects **main only** (a `?revision=` query does NOT switch it) — the TUI's main-only scan is the root cause of the FLARE bug.
- flare's GPU table (`helios/src/package/naming.rs platform_from_pci`): PCI device id `0x2E12`→`gb10`; `{0x2B85, 0x2B87, 0x2B8C, 0x2BB1, 0x2BB2, 0x2BB3, 0x2BB4, 0x2BB5, 0x2BB9}`→`gb202`; else error demanding `--revision`. `--gpus` grammar: comma ints + `N-M` ranges, indices relative to `CUDA_VISIBLE_DEVICES` when set; count = tensor-parallel size.
- flare's `choose_file` package rule: unique top-level `.safetensors` whose stem is not 64-hex and not `*-data-NN-of-NN`; directory form = root `model.safetensors` + companions under `weights/`, `deployments/`.
- Package forms in the wild: packed single file (`qwen3.8-flash-next-yarn4-tp2.safetensors`, 111.4 GB) and directory canonical (`model.safetensors` + 55 companions, paths load-bearing).

## 1. Non-goals

- No helios/flare-cache writes (D1); no `flare run MODEL` by-name visibility.
- No publisher-signature crypto verification (D7 rationale above).
- No auto-update/version tracking of installed packages (D7).
- No `hf-cache sync --for flare` (D7 rationale above).
- No payload download in any automated test or gate (D6).
- No version bump / changelog entry in this PR (release-flow commits own those, per AGENTS.md).

## 2. Current state (gap analysis)

| Area | Today | Gap for FLARE repos |
|---|---|---|
| TUI scan (`src/ui/app/search.rs`) | `fetch_model_metadata(id, "main")` → `classify_quantizations(siblings)` → Gguf or Standard mode | `main` holds 3–4 metadata files: no quants → Standard mode shows an empty-ish tree; branches/tags never fetched |
| Revision plumbing (`src/api/client.rs`) | `fetch_model_metadata` takes a revision and feeds `fetch_recursive_tree` (private); `resolve_revision_sha` exists; CLI `download --revision` works end-to-end | No `/refs` fetch; no per-revision tree entry point; nothing FLARE-shaped |
| Viewer (`src/ui/render/`) | `ModelDisplayMode::{Gguf,Standard}`; per-mode panels | No FLARE mode |
| TUI downloads (`src/ui/app/downloads.rs`) | 'd' flows enqueue with `DEFAULT_REVISION` URLs | No revision choice; payload files invisible |
| CLI (`src/cli/args.rs`, `download_cmd.rs`) | `--revision`, `--include/--exclude`, `--for vllm` (glob tables) | No `--gpus`, no `flare` preset, no handoff line |
| Detection | `ModelMetadata.tags` parsed but only used for display | FLARE repos detectable via tags or root `publisher.json` + `config.json` markers |

## 3. Target architecture

### 3.1 API + models (foundation)

New `src/api/flare.rs` (pure core + thin fetchers, mirroring the `api/quant.rs`
precedent) and DTOs in `src/models/api.rs`:

```rust
// models/api.rs
pub struct RefEntry { pub name: String, pub target_commit: String }   // serde camelCase
pub struct RepoRefs  { pub branches: Vec<RefEntry>, pub tags: Vec<RefEntry> }

pub enum FlarePackage {
    Packed  { file: RepoFile },                       // single top-level .safetensors
    Directory { primary: RepoFile, companions: Vec<RepoFile> }, // model.safetensors + weights/* + deployments/**
}
pub struct FlareRevision {
    pub revision: String,          // preferred name: branch, else tag
    pub aliases: Vec<String>,      // e.g. ["v3"] when a tag shares target_commit
    pub kind: RefKind,             // Branch | Tag
    pub package: FlarePackage,     // selection per flare's choose_file rule
    pub parsed: ParsedPackageTokens, // tp/c/mtp/i4/arch/variant — display only
}
pub struct FlareRepoInfo {
    pub revisions: Vec<FlareRevision>,        // payload-bearing only, main excluded
    pub config: FlareConfigFacts,             // from main's config.json (see below)
    pub publisher: Option<FlarePublisher>,    // namespace, name, development, key fingerprint
}
```

- `fetch_repo_refs(client, model_id) -> RepoRefs` — `GET /api/models/{id}/refs`.
- `fetch_revision_tree(client, model_id, revision) -> Vec<RepoFile>` — **new pub entry point** wrapping the existing `fetch_recursive_tree` at root; `fetch_model_metadata` is refactored to call it (dedupe, no behavior change).
- `fetch_raw_file(client, model_id, revision, path) -> String` — `GET /{repo}/raw/{rev}/{path}` (non-LFS files only; error if the tree marks it LFS).
- Detection (pure): `flare_disposition(&ModelMetadata) -> FlareDisposition` — `Yes` when `tags` contains `"flare"`; `Probable` when root `publisher.json` is in siblings (then confirmed by `config.json` markers below); else `No`. The confirm markers: JSON `"format"=="flare"` | `"runtime"=="flare"` | `"model_type"=="flare-runtime-package"` | a top-level `"flare"` object — this is required because the user's own repo (`JohannesBertens/…-1M-TP2`) carries **no** `flare` tag; its `config.json` has `model_type: "flare-runtime-package"` + a `flare` block.
- Assembly, split pure/network so fixtures test the core:
  `assemble_flare_info_from(refs, trees_by_revision, main_config_json, publisher_json) -> Result<FlareRepoInfo, FlareScanError>` and a thin `fetch_flare_info(client, model_id)` wrapper. Payload refs = branches≠`main` + tags, deduped by `target_commit` (tag aliasing a branch head → alias name). Revisions without a package (`choose_file` finds nothing) are skipped, not errors. `FlareConfigFacts`: `serving_context`, `execution_abi`, `prepared_model_contract`, `checkpoint` (missing keys ⇒ None fields, never an error).
- Publisher fingerprint: sha256 over the raw `public_key` byte array — same 64-hex form flare displays.
- `parse_flare_package_tokens(filename) -> ParsedPackageTokens` — pure grammar: `tp<N>`, `c<N>`, `mtp<N>`, `i4`, `gb<token>`, free variant strings; unknown tokens ignored (display-only, pinned by unit tests against all three real package names).

### 3.2 GPU detection — new `src/gpu.rs`

- `pub enum FlarePlatform { Gb202, Gb10 }` (+ `as_str()`).
- `pub fn platform_from_device_id(id: u16) -> Option<FlarePlatform>` — flare's table above, byte-for-byte, pinned by a unit test (source comment: helios `platform_from_pci`).
- `pub fn parse_gpus_spec(s: &str) -> Result<Vec<usize>, String>` — comma ints + `N-M` ranges (flare grammar; empty → error; UUID forms rejected with a message pointing at `--revision`).
- `pub fn resolve_cuda_visible(env: Option<&str>) -> Option<Vec<usize>>` — integer `CUDA_VISIBLE_DEVICES` tokens honored; any `GPU-…`/`MIG-…` token ⇒ `None` (treated as undetectable → demand `--revision`), matching flare's only-env handling honestly.
- `pub fn detect_platform(selected: &[usize]) -> Result<FlarePlatform, GpuDetectError>` — one `nvidia-smi --query-gpu=index,pci.device_id,compute_cap,name --format=csv,noheader` subprocess (5 s timeout, run under `tokio::task::spawn_blocking` from async contexts); indices resolved through `CUDA_VISIBLE_DEVICES` when set; device id parsed from either `0x<id><vendor>` or `0x<id>` rendering (mask the half ≠ `0x10DE`); all selected GPUs must map to one platform (flare's mixed-arch rule); unknown/missing ⇒ `GpuDetectError` whose Display tells the user to pass `--revision` and lists the payload revisions. RTX 5080/5070 (GB203/205) MUST NOT auto-map (device-ID table only — the pinned test asserts they are absent).
- Pure parts unit-tested from fixture CSV strings; the subprocess shell stays thin and untested (covered by the live smoke).

### 3.3 CLI — `download --for flare`

- `args.rs`: `DownloadArgs` gains `--gpus <SPEC>` (value_parser `parse_gpus_spec`); `parse_preset` accepts `"flare"` alongside `"vllm"` (help text updated; help-snapshot tests refreshed).
- Selection is **semantic, not glob tables**: `--for flare` resolves the revision, fetches that revision's tree, and selects the package file set via `assemble_flare_info_from`'s package rule (packed: the single file; directory: `model.safetensors` + `weights/**` + `deployments/**` + `config.json`/`publisher.json`). Explicit `--include/--exclude` then filter **within** that set (composability; documented). The vllm glob-table machinery in `patterns.rs` is untouched.
- Revision resolution order (mirrors flare): `--revision` > `MODEL@REV`? (not applicable — clap model arg stays plain) > auto-detect from `--gpus`. `--for flare` with neither `--revision` nor a resolvable `--gpus` ⇒ error listing the repo's payload revisions (from `/refs`).
- Validation: the chosen revision must exist in `/refs` (branches ∪ tags); the resolution also **displays** the exact revision + target commit in the human reporter and in the existing NDJSON run events (no new event types — the schema is additive-only and nothing new is needed).
- sha256 verification: unchanged engine behavior — the resume journal already pins `sha256` from the LFS oid, so every payload file is verified against the Hub's digest natively.
- Handoff line (D1): after the run completes, the human reporter prints the exact serve command derived from the **resolved final paths** of the downloaded files — `flare run <download_dir>/<package-file> --gpus <spec>` for packed form, `flare run <download_dir> --gpus <spec>` for directory form (paths are load-bearing; rhd preserves relative paths, so the directory form lands runnable). NDJSON output stays machine-clean (no hint line).

### 3.4 TUI viewer — `ModelDisplayMode::Flare`

- `models/ui.rs`: `ModelDisplayMode::Flare`; `FocusedPane::FlareRevisions` (`accepts_download() == true`, `accepts_verify() == false` — update the pinned pane-set test).
- `state.rs`: `flare_info: Arc<RwLock<Option<FlareRepoInfo>>>` + scan wiring in `search.rs`: after metadata lands, run detection (tags → probable → confirm via `fetch_raw_file(config.json)`); FLARE ⇒ fetch refs + per-revision trees (concurrently, `futures` join) ⇒ `assemble_flare_info_from` ⇒ set mode `Flare`. Errors degrade to Standard mode with a status-line note, never a dead end (matches the existing Standard-fallback philosophy).
- `ApiCache` (models/cache.rs): new `flare: HashMap<String, FlareRepoInfo>` value map + timestamp map + `CacheMapFor` impl — same TTL/get-or-fetch pattern as metadata/quantizations/file_trees.
- `render/flare.rs` (new panel, `gguf.rs` precedent): revision cards — revision + aliases, kind badge (branch/tag), target commit (short), package file(s), size via `fmt.rs` surface rules (full form in this panel, HUD-compact stays in the HUD — the do-not-unify contract), parsed tokens (TP/concurrency/MTP), detected-platform star when `state.flare_platform` is `Some`; a config-facts header (serving context, ABI, contract, checkpoint) and the publisher line (namespace, name, development flag, fingerprint). `RenderParams` gains `FlarePanelContext`; render is pure; snapshots registered in the `test_utils.rs` manifest (both-directions guard) — `snapshot-name manifest` must be updated in the same phase.
- Mouse: the new pane registers hit-rects via `RenderOutput.mouse.panels` (registration order: after Results list, before the GGUF/Standard bottom panes — document in AGENTS.md mouse section).

### 3.5 TUI download — revision-picker popup

- `PopupMode::FlareRevision` (new variant; popup state: cursor + the revision list snapshot from `flare_info`).
- `events/`: 'd' in Flare mode (pane or Models list while a FLARE model is selected) opens the popup; ↑/↓ move; Enter enqueues; Esc cancels. Tab cycles Models ↔ FlareRevisions mirroring GGUF mode.
- `downloads.rs`: new `trigger_flare_download(revision)` — builds FileSpecs from `FlareRepoInfo` with revision-pinned `resolve_url`s, then the existing repository-download enqueue path (`EnqueuePolicy::tui_repository`); registry entries carry the chosen revision in the existing `revision` field. Directory-form packages enqueue all package files.
- Popup rendering in `render/popups.rs` (resume/search/download-path precedent); snapshots + manifest update.

### 3.6 Docs

- `README.md`: FLARE section (what rhd shows, the preset, the handoff, the device-ID table, `--revision` override), CLI flags table rows (`--gpus`, `--for flare`), TUI keys (Flare mode, popup).
- `AGENTS.md`: module-map lines (`src/api/flare.rs`, `src/gpu.rs`, `src/ui/render/flare.rs`), mouse registration-order note, options/none.
- `plans/README.md`: this plan's index row (same commit as the plan).
- No version bump, no changelog entries (release commits own those).

## 4. Phases (each lands green: fmt + clippy -D warnings + scoped tests pass)

1. **API + models foundation** (Lane A): DTOs, `fetch_repo_refs` / `fetch_revision_tree` / `fetch_raw_file`, detection + `assemble_flare_info_from` pure core, token/publisher-fingerprint parsers, fixtures `tests/fixtures/flare/` (refs/tree-main/tree-gb202/tree-gb10/tree-v3/config-main/publisher for the three real repos, recorded shapes), unit tests from fixtures.
   Accept: `cargo test flare` · `cargo test api` · fmt/clippy.
2. **GPU module** (Lane B, parallel with 1): `src/gpu.rs` per §3.2, pure-parser tests from fixture CSV lines, pinned device-ID table test (incl. GB203/205 absence).
   Accept: `cargo test gpu` · fmt/clippy.
3. **CLI preset** (Lane C, after 1+2 merge): `--gpus`, `--for flare` resolution + selection + handoff line, args/help-snapshot/report tests, error paths (unknown GPU, missing revision, mixed arch).
   Accept: `cargo test --test cli_download cli_surface` (scoped) · `INSTA_UPDATE=always` review of help snapshots · fmt/clippy.
4. **TUI viewer** (Lane D1, after 1 merges; parallel with 3): display mode, state, scan wiring, ApiCache map, `render/flare.rs` + snapshots + manifest, mouse areas.
   Accept: `cargo test ui` · insta snapshots reviewed · fmt/clippy.
5. **TUI download popup** (Lane D2, after 4): `PopupMode::FlareRevision`, key routing, `trigger_flare_download`, registry revision field, popup render + snapshots.
   Accept: `cargo test ui downloads enqueue` · fmt/clippy.
6. **Docs + live smoke** (in-session): README, AGENTS.md, plans index; run the §5 smoke; full `cargo test --locked`.

## 5. Acceptance

Commands (every phase + ship):

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Live smoke (documented here, executed at ship; metadata-only, no payload bytes):

1. TUI: search + select each of `local-inference-lab/Qwen3.8-Flash-Next-FLARE` (expect gb202+gb10 cards), `lflare/Qwen3.8-Flash-Next-FLARE-Abliterated` (expect v1/v2 tags + gb202=v3 alias, directory-form package), `JohannesBertens/Qwen3.8-Flash-Next-FLARE-1M-TP2` (expect gb202 packed card; detected-platform star on GB202 hardware). 'd' → popup → Esc (no download).
2. CLI resolution-only: `rhd download <repo> --for flare --revision gb202 --include 'no-such-glob'` exits with the resolve-failure path having exercised `/refs` + per-revision tree + package selection (zero files selected ⇒ documented error), proving the metadata pipeline against the live Hub under `HF_ENDPOINT` default.
3. CLI detection on GB202 hardware: `rhd download JohannesBertens/Qwen3.8-Flash-Next-FLARE-1M-TP2 --for flare --gpus 0,1 --include 'no-such-glob'` ⇒ auto-selects `gb202` (shown in output), same zero-file termination.
4. `HF_ENDPOINT=https://hf-mirror.com rhd …` smoke of step 2 once, to note mirror parity (informational, not a gate).

## 6. Risks

- **GB203/GB205 mis-selection** would ship a broken package: mitigated by device-ID-only mapping + the pinned absence test + unknown⇒`--revision` error.
- **`pci.device_id` rendering variance** (`0x10DE2BB5` vs `0x2BB5`): parser accepts both (mask the ≠`0x10DE` half); verified in the live smoke on real hardware.
- **Mirror parity for `/refs` + `/tree`** unprobed on third-party mirrors: informational smoke only; failure degrades to the `--revision` path.
- **Scan latency** (refs + N per-revision trees on selection): FLARE repos have ≤4 payload revisions; trees are shallow (≤60 files); cached with TTL like metadata. Non-FLARE repos pay at most one `config.json` raw fetch when `publisher.json` is present (rare, flare-specific).
- **API drift on `/refs` shape**: DTO uses serde defaults + unknown fields ignored; a missing `targetCommit` degrades to alias-dedup off.

## 7. Lane decomposition & merge order

- Wave 1 (parallel): **A** = §3.1 (api + models + fixtures) — branch `flare/api`; **B** = §3.2 (`gpu.rs`) — branch `flare/gpu`. Disjoint file sets.
- Wave 2 (parallel, after wave 1 merges): **C** = §3.3 CLI — branch `flare/cli` (touches `cli/`, help snapshots); **D** = §3.4+§3.5 TUI — branch `flare/tui` (touches `models/ui.rs`, `ui/`, cache). Disjoint file sets.
- Every lane: 60-min timeout, commit per increment, push after every commit; merged into `feature/flare-models` in-session the moment its gates pass.

## 8. Interface contract (wave-2 lanes code against this)

The exact signatures in §3.1 (`fetch_repo_refs`, `fetch_revision_tree`, `fetch_raw_file`, `flare_disposition`, `assemble_flare_info_from`, `fetch_flare_info`, `parse_flare_package_tokens`) and §3.2 (`FlarePlatform`, `platform_from_device_id`, `parse_gpus_spec`, `resolve_cuda_visible`, `detect_platform`) are the contract; wave-1 tests pin them. Any wave-1↔wave-2 drift is resolved in favor of wave 1's merged types, with the plan amended in the same commit.
