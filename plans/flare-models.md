---
status: proposed (feature branch `feature/flare-models`)
---

# FLARE-aware scanning, viewer, downloads, and a `--for flare` serve-path handoff

Date: 2026-10-29 · Feature loop run (grill digest + 3 research lanes, briefs in
`.review/flare-grill/` — untracked session artifacts; durable facts inlined).

**Tier: L** — cross-cutting within one repo (api scanner, TUI viewer, TUI
download flow, CLI flags, new GPU-detection module, docs), four interacting
segments sharing one new `FlareRepoInfo` model; single-run scope, acceptance
pinned by the grill. Not XXL: one repo, no migration, no cross-repo work.

**Drop-in investigation outcome (the ask's 4th clause), reasoning inlined:**
flare `download` does NOT leave downloaded bytes on disk — it stages via
hf-hub, verifies signatures, then *decomposes* the package into a proprietary
content-addressed cache (`~/.cache/flare/packages`: payload tensors re-hashed
into `blobs/<sha256>.safetensors`, a *rebuilt metadata-only* `model.safetensors`
primary, hardlinked companions, sealed read-only files, `refs/`+`sources/`
records — helios `package/cache.rs`). Byte-reproducing that cache means
reimplementing helios or linking `flare-helios` (a git path-dependency on a
fast-moving engine crate) — rejected as fragile duplication of the install
step. Rejected alternative: design-doc-only (the grill chose shipping a real
replacement). Chosen: **serve-path handoff** — flare's `run /explicit/path`
mode bypasses the cache entirely and accepts a plain packed file (or directory
package) in place, so rhd downloads the exact package with its own resumable
engine and prints the serve command. Bonus over flare: rhd honors
`HF_TOKEN`/`HF_ENDPOINT` (flare hardcodes the endpoint and takes tokens only
via `--token-file`).

## 0. Grill digest — decisions (user-settled 2026-10-29)

| # | Decision |
|---|---|
| D1 | Drop-in depth = **serve-path handoff** (see the inlined rationale above): auto-select the payload revision, download package files with the existing engine into a normal directory, print the exact `flare run <path> --gpus <spec>` line. No helios dependency, no writes to `~/.cache/flare/packages`. |
| D2 | GPU→branch = **`nvidia-smi` subprocess + flare's exact PCI device-ID table**; unknown ID / missing nvidia-smi / mixed architectures ⇒ clear error demanding `--revision` (flare's own rule). `CUDA_VISIBLE_DEVICES` integer indices honored. |
| D3 | Viewer = **third display mode** `ModelDisplayMode::Flare` with revision cards. |
| D4 | TUI download = **revision-picker popup** ('d' in Flare mode → popup → Enter enqueues, commit-pinned). |
| D5 | CLI = **new `download --for flare` + `--gpus` flags** (download currently has NEITHER `--for` NOR `--include/--exclude` — those live on `hf-cache sync`; this plan adds them to `download`, scoped, below). |
| D6 | Acceptance = **offline fixtures + bounded live smoke** (metadata-only network; never the ~107 GB payload). |
| D7 | Hard non-goal: **auto-update tracking** (no flare `sources/`/`origins/` equivalent). Included in minimal form because not excluded: search-list FLARE badge (§3.4a — tag-based, covers all three flagship repos) and publisher identity display in the viewer. |

**Kept out with rationale (user may veto):** cryptographic publisher-signature
verification (flare re-verifies at load; rhd surfaces identity + verifies
payload sha256 against the Hub LFS digest — see §3.3e); `hf-cache sync --for
flare` (flare never reads the hub cache — the preset would produce bytes
nothing consumes; `hf-cache`'s preset parser stays `vllm`-only).

## Research facts the design rests on (inlined from the lane briefs)

- `GET {api}/api/models/{id}/refs` → `{branches:[{name,ref,targetCommit}],tags:[…],converts:[…]}` (camelCase, no pagination). Live-verified on all three FLARE repos; tag `v3` shares `gb202`'s `targetCommit` (the alias-dedup case), and the lflare `tree/gb202?recursive=true` directory form (root `model.safetensors` + `weights/**` + `deployments/gb202-tp1/rank-0/**`) was verified live.
- `GET {api}/api/models/{id}/tree/{rev}?recursive=true` → per-revision files with true `lfs.size` + `lfs.oid` (sha256) + `xetHash`; `Link: rel="next"` pagination — exactly what rhd's private `fetch_recursive_tree` (`src/api/client.rs:277`) already implements.
- `GET {api}/{repo}/raw/{rev}/{path}` returns pointer-free content for non-LFS files (config.json, publisher.json); an LFS path would return the pointer text — callers guard using already-fetched tree data.
- The Hub's `/api/models/{id}` `siblings` field reflects main only — irrelevant to rhd (whose `fetch_model_metadata` OVERWRITES `siblings` with the revision-aware recursive tree at the requested revision, `client.rs:148-159`); the actual TUI gap is that the scan passes `DEFAULT_REVISION` (`search.rs:256`), so payload branches are never fetched.
- flare's GPU table (`helios/src/package/naming.rs platform_from_pci`; detection `flare/src/hardware.rs detect_isolated`): PCI device id `0x2E12`→`gb10`; `{0x2B85, 0x2B87, 0x2B8C, 0x2BB1, 0x2BB2, 0x2BB3, 0x2BB4, 0x2BB5, 0x2BB9}`→`gb202`; else error demanding `--revision`. `--gpus` grammar: comma ints + `N-M` ranges, indices relative to `CUDA_VISIBLE_DEVICES` when set; count = tensor-parallel size. Mixed architectures across the selection → error.
- flare's `choose_file` package rule: unique top-level `.safetensors` whose stem is not 64-hex and not `*-data-NN-of-NN`; directory form = root `model.safetensors` + companions. Package names in the wild: `qwen3.8-flash-next-yarn4-tp2.safetensors` (111,434,386,432 B, JB `gb202`), `qwen3.8-flash-next-tp1-c16-mtp5.safetensors` (107,011,338,717 B, LIL `gb202` — and the SAME filename on lflare tags `v1`/`v2` with different shas: the cross-revision collision case), `qwen3.8-flash-next-gb10-tp1-c8-i4-mtp5.safetensors` (LIL `gb10`).
- The user's own repo carries **no** `flare` tag (tags include `flare-runtime-package`); the other two carry `flare`. Detection therefore keys on tags OR root `publisher.json` + config.json markers (`"format"=="flare"` | `"runtime"=="flare"` | `"model_type"=="flare-runtime-package"` | a top-level `"flare"` object — verified live in all three repos' `raw/main/config.json`).

## 1. Non-goals

- No helios/flare-cache writes (D1); no `flare run MODEL` by-name visibility.
- No publisher-signature crypto verification (D7 rationale above).
- No auto-update/version tracking of installed packages (D7).
- No `hf-cache sync --for flare` (D7 rationale; hf-cache preset parser untouched).
- No payload download in any automated test or gate (D6).
- No version bump / changelog entry in this PR (release-flow commits own those, per AGENTS.md).

## 2. Current state (gap analysis)

| Area | Today | Gap for FLARE repos |
|---|---|---|
| TUI scan (`src/ui/app/search.rs:256,277-313`) | metadata at `DEFAULT_REVISION` → `classify_quantizations(siblings)` → Gguf or Standard | `main` holds 3–4 metadata files: no quants → Standard mode shows an empty-ish tree; branches/tags never fetched |
| Revision plumbing (`src/api/client.rs`) | `fetch_model_metadata` fetches the tree AT a requested revision (siblings overwritten); `resolve_revision_sha` exists; CLI `download --revision` works end-to-end | No `/refs` fetch; no public per-revision tree entry point; nothing FLARE-shaped |
| Viewer (`src/ui/render/`) | `ModelDisplayMode::{Gguf,Standard}`; per-mode panels; snapshot-name manifest | No FLARE mode |
| TUI downloads (`src/ui/app/downloads.rs:228,443`; `model_root` :519) | 'd' flows enqueue with `DEFAULT_REVISION` URLs; destinations `base/author/model/…` — no revision component | No revision choice; payload files invisible; cross-revision filename collisions possible |
| CLI (`src/cli/args.rs:189-220` `DownloadArgs`) | `--quant/--file/--all/--output/--revision`; **no** `--for`, `--include/--exclude`, `--gpus` (those exist only on `HfCacheSyncArgs` :258-270, whose preset parser is vllm-only) | Everything FLARE is new surface |
| Existing-file skip (`src/download/mod.rs:387`) | a final path that exists is treated as complete (verify-if-enabled), regardless of revision | Same filename across revisions (lflare v1/v2; directory-form `model.safetensors`) would silently reuse the previous revision's bytes |
| Detection (`src/models/api.rs:34`) | `tags` parsed, display-only | FLARE repos detectable via tags or root `publisher.json` + config markers |

## 3. Target architecture

### 3.1 API + models (foundation)

New `src/api/flare.rs` (pure core + thin fetchers, `api/quant.rs` precedent;
declared in the `src/api/mod.rs` facade as `mod flare;` + selective `pub use`),
DTOs in `src/models/api.rs`:

```rust
pub struct RefEntry { pub name: String, pub target_commit: String }   // serde camelCase
pub struct RepoRefs  { pub branches: Vec<RefEntry>, pub tags: Vec<RefEntry> }

pub enum FlarePackage {
    Packed     { file: RepoFile },
    Directory  { primary: RepoFile, companions: Vec<RepoFile> },  // model.safetensors + weights/** + deployments/**
}
pub struct FlareRevision {
    pub revision: String,          // PREFERRED name: branch, else tag (directory-segment + display)
    pub aliases: Vec<String>,      // e.g. ["v3"] when a tag shares target_commit
    pub kind: RefKind,             // Branch | Tag
    pub target_commit: String,     // 40-hex pin — trees, URLs, registry all use THIS
    pub package: FlarePackage,     // selection per flare's choose_file rule
    pub parsed: ParsedPackageTokens, // tp/c/mtp/i4/arch/variant — display only
}
pub struct FlareRepoInfo {
    pub revisions: Vec<FlareRevision>,        // payload-bearing only, main excluded
    pub config: FlareConfigFacts,             // serving_context / execution_abi / prepared_model_contract / checkpoint — all Option, never errors
    pub publisher: Option<FlarePublisher>,    // namespace, name, development, sha256-hex of public_key bytes (display-only claim, dropped the "same form flare displays" wording)
}
```

- `fetch_repo_refs(client, model_id) -> RepoRefs` — `GET /api/models/{id}/refs`. **Missing/invalid `targetCommit` on a refs entry is NOT degradable** (trees, URLs and registry entries are commit-keyed): that revision is DROPPED from `FlareRepoInfo` with a scan note naming it. The CLI's selected-revision ladder (§3.3b) has an explicit `resolve_revision_sha` fallback for names absent from `/refs`.
- `fetch_revision_tree(client, model_id, revision) -> Vec<RepoFile>` — new pub entry point wrapping `fetch_recursive_tree` at root; `fetch_model_metadata` refactored to call it (dedupe, behavior-neutral).
- `fetch_raw_file(client, model_id, revision, path) -> String` — thin `GET /{repo}/raw/{rev}/{path}`; the CALLER guards LFS (checks the already-fetched tree's `lfs` marker before calling — no extra tree requests inside).
- Detection (pure): `flare_disposition(&ModelMetadata) -> FlareDisposition` — `Yes` on a `flare` tag; `Probable` when root `publisher.json` is in siblings (confirmed via config markers above); else `No`.
- Assembly, pure core + thin wrapper so inline fixtures test the core:
  `assemble_flare_info_from(refs, trees_by_target_commit, main_config_json, publisher_json) -> Result<FlareRepoInfo, FlareScanError>`; payload refs = branches≠`main` + tags, deduped by `target_commit` (alias names preserved); revisions whose tree yields no package are skipped, not errors.
- `parse_flare_package_tokens(filename) -> ParsedPackageTokens` — pure grammar: `tp<N>`, `c<N>`, `mtp<N>`, `i4`, `gb<token>`, free variants; unknown tokens ignored; display-only; unit tests pin all three real package names.

### 3.2 GPU detection — new `src/gpu.rs` (declared in `src/main.rs`)

- `pub enum FlarePlatform { Gb202, Gb10 }` (+ `as_str()`).
- `pub fn platform_from_device_id(id: u16) -> Option<FlarePlatform>` — flare's table above, byte-for-byte, pinned by a unit test asserting GB203 (`0x2C02`) and GB205 (`0x2F04`) are ABSENT.
- `pub fn parse_gpus_spec(s: &str) -> Result<Vec<usize>, String>` — comma ints + `N-M` ranges; empty ⇒ error; `M < N` ⇒ error; duplicates collapse silently; UUID-ish tokens rejected with a message pointing at `--revision`.
- `pub fn resolve_cuda_visible(env: Option<&str>) -> Option<Vec<usize>>` — integer `CUDA_VISIBLE_DEVICES` honored; any `GPU-…`/`MIG-…`/empty token ⇒ `None` (undetectable → demand `--revision`).
- `pub enum GpuDetectError { NoNvidiaSmi, Unparseable(String), UnknownIds(Vec<GpuId>), Mixed(Vec<String>), IndexOutOfRange { selected: usize, count: usize } }` — **structured, repo-agnostic**: its `Display` never mentions revisions; the CLI layer appends "available revisions: …" from `/refs`. (Fixes the layering finding: detection owns no repository knowledge.)
- `pub fn detect_platform(selected: &[usize]) -> Result<FlarePlatform, GpuDetectError>` — one `nvidia-smi --query-gpu=index,pci.device_id,compute_cap,name --format=csv,noheader` subprocess (5 s timeout; `tokio::task::spawn_blocking` from async contexts); indices resolved through `CUDA_VISIBLE_DEVICES` when set; out-of-range selection ⇒ `IndexOutOfRange`; device id parsed from either the 32-bit `0x<dev><vendor>` or bare `0x<dev>` rendering (take the 16-bit half ≠ `0x10DE`; both renderings fixture-tested); all selected GPUs must map to one platform else `Mixed`; unknown ⇒ `UnknownIds` (carrying name+compute_cap for the message).
- Pure parts unit-tested from inline CSV-string fixtures; the subprocess shell stays thin (live-smoke covered).

### 3.3 CLI — `download --for flare` (all flags are NEW on this subcommand)

a. **Args** (`src/cli/args.rs`, `DownloadArgs`): `--for <PRESET>` (new `parse_flare_preset` accepting **only** `flare` — `hf-cache`'s shared `parse_preset` stays vllm-only and untouched, so `hf-cache sync --for flare` remains unparseable), `--gpus <SPEC>` (`parse_gpus_spec`), `--include/--exclude <GLOB>` (repeatable; **`requires = "for_preset"`** — general include-on-download is out of scope this round). Clap conflicts: `--for` conflicts with `--quant`, `--file`, `--all` (mutually exclusive selectors — error, not precedence).

b. **Revision resolution** (mirrors flare's precedence `--revision` > detection). `--revision` accepting "any branch/tag/sha" is honest only with this ladder: (i) a 40-hex value is used DIRECTLY as the pin (skip refs validation; tree fetched at the sha; registry stores the sha; directory segment = the sha — see §3.3d); (ii) a branch/tag name present in `/refs` pins to its `targetCommit`; (iii) a name absent from `/refs` or lacking a commit falls back to the existing `resolve_revision_sha(client, model_id, revision)` (`client.rs:190`) — failure is a hard error before any fetch. `--gpus` detection (§3.2) resolves through the same ladder. Neither `--revision` nor a resolvable `--gpus` ⇒ error listing the repo's payload revisions (from `/refs`). The commit is the PIN everywhere: trees are fetched at `target_commit`, download URLs are `resolve_url(model, path, target_commit)`, and the registry `DownloadMetadata.revision` stores the **40-hex commit** (valid per its docs; the friendly name is display-only). Branch movement between scan and download can therefore not mix commits.

c. **Selection is semantic, not glob tables**: `--for flare` selects the package file set via `assemble_flare_info_from`'s rule (packed: the single file; directory: `model.safetensors` + `weights/**` + `deployments/**` + `config.json` + `publisher.json`). Explicit `--include/--exclude` filter WITHIN that set. `patterns.rs` glob machinery is reused for matching; the vllm preset tables are untouched.

d. **Revision-scoped destinations via prefixed filenames — full lifecycle parity, zero engine changes** (the collision fix): FLARE frontends pass each file as `filename = "<revision-dir>/<repo-relpath>"` (e.g. `gb202/weights/layer-000.safetensors`), where `<revision-dir>` is the preferred revision name — or the 40-hex sha for an explicitly pinned run. This works because every path in the lifecycle derives from `filename`: the engine joins `base_path.join(sanitized_filename)` subdir-preserving (`src/download/mod.rs:227`, the `tokenizer/config.json` precedent), the registry's `validate_and_sanitize_path(base, model_id, filename)` produces the same path (`src/registry.rs:290-292`), and TUI resume reconstructs `model_root_or(...) + metadata.filename` identically (`src/ui/app/downloads.rs:292-299`) — landing everything at `base/author/model/<revision-dir>/<relpath>` with registry, resume, and disk in parity. Bonus: the complete-downloads mirror keys on the prefixed filename, so cross-revision same-name files never collide there either. `--include/--exclude` glob matching applies to the REPO-relative path BEFORE prefixing (documented); the registry/TUI display shows the prefixed local path (that is the local truth).
   **Pre-flight (pure, before enqueue):** (i) on-disk size vs expected tree size per file — mismatch ⇒ hard error naming file, expected vs actual bytes, and the directory; (ii) **moved-branch guard** — existing registry entries for this model whose `filename` starts with the same `<revision-dir>/` but whose `revision` (commit) differs from this run's pin ⇒ hard error ("branch moved: directory holds commit A, run targets B — remove the directory or pin `--revision <sha>`"). Equal-size, same-commit re-runs proceed (engine's existing-file skip is idempotent; verification re-checks the digest when enabled). Fixture tests: lflare `v1` then `v2` (same package filename, different shas ⇒ two complete files in two dirs); moved `gb202` (commit A recorded, run targets B ⇒ error); interrupted nested-file round-trip (registry `local_path` incl. revision segment → resume re-enqueues the identical path). The ordinary (non-FLARE) download layout is unchanged.

e. **sha256 verification, stated correctly**: the resume journal PINS `expected_sha256` from the LFS oid (identity check for resume, `journal.rs:77-80`); the actual payload hashing is the verification worker's, queued when a digest exists and verification is enabled — `--no-verify` disables it, and non-LFS files (directory-form `config.json`/`publisher.json`) carry no oid. Requirement on both new frontends: forward `lfs.oid` into `expected_sha256` so LFS payloads ARE digest-verified by default. No publisher-signature verification (D7).

f. **Handoff line, gated**: printed by the human reporter ONLY when (i) the run exited success, (ii) the selected set equals the FULL package set (no `--include` filtering — otherwise a note "partial package selection — no serve command"), and (iii) every package file's outcome is Downloaded or AlreadyExists. Suppressed on interruption or any failure. Packed form: `flare run <quoted final path of the primary> [--gpus <spec verbatim>]`. Directory form: explicit-path acceptance of a *directory package* by `flare run` is an INFERENCE from flare's `choose_file` canonical `<dir>/model.safetensors` shape (the sourced fact covers packed explicit-path files only) — Phase 3 must verify it against the flare source/docs, and until verified the directory-form handoff prints the package directory plus guidance ("serve with `flare run <dir>` if your build accepts directory packages") instead of asserting a runnable command. `--gpus` omitted when the user passed none (flare defaults to the first GPU); paths single-quote-escaped (POSIX) with a Windows no-op note. Path source: a new pure `download::final_path_for(base, relpath)` (extracted from the engine's sanitize+join step, unit-tested for parity with the engine-built path). NDJSON (additive-only schema): `Event::Resolved` gains `revision: Option<String>` (display name) + `commit: Option<String>`, `skip_serializing_if = None` — absent for existing commands; `cli/events_tests.rs` serialization snapshots updated.

### 3.4 TUI viewer — `ModelDisplayMode::Flare`

- `models/ui.rs`: `ModelDisplayMode::Flare`; `FocusedPane::FlareRevisions` (`accepts_download() == true`, `accepts_verify() == false` — pinned pane-set test updated).
- `state.rs`: `flare_info: Arc<RwLock<Option<FlareRepoInfo>>>` (per-model; cleared wherever quant/tree state is cleared today — the `clear_model_details` block at `search.rs:341` — together with the popup state) + `flare_platform: Arc<RwLock<Option<FlarePlatform>>>` (**session-scoped**: detected once via `spawn_blocking`, NEVER cleared — hardware does not change per model; revision cards derive their star by comparing the session platform against the card's arch token). **Ownership rule (stale-scan fix):** the flare scan task captures the model id and publishes ONLY if the currently selected model still matches. A reversed-completion test (slow A, fast B) pins it.
- Scan wiring in `search.rs`: after metadata, `flare_disposition` → (Probable ⇒ confirm via `fetch_raw_file(config.json)`) → fetch `/refs` + per-revision trees (concurrent join) at `target_commit` ⇒ `assemble_flare_info_from` ⇒ mode `Flare`. Errors degrade to Standard mode + status note, never a dead end.
- `ApiCache` (models/cache.rs): new `flare` value map + timestamp map + `CacheMapFor` impl — same get-or-fetch pattern (verified real at `cache.rs:67-69,149`).
- `render/flare.rs` (new panel, `gguf.rs` precedent): revision cards — revision + aliases, kind badge, short commit, package file(s), size via `fmt.rs` full-form rules, parsed tokens, detected-platform star when `flare_platform` matches the card's arch token; config-facts header; publisher line. `RenderParams` gains `FlarePanelContext`; render pure; snapshots registered in the `test_utils.rs` manifest (both-directions guard) in the same phase.
- Mouse: the pane registers hit-rects via `RenderOutput.mouse.panels` (order: Results list, then FlareRevisions, then existing bottom panes — AGENTS.md mouse section updated).

**3.4a Search-list badge (D7 minimal form):** `render/models_list.rs` shows a `[FLARE]` badge when any model tag equals `flare` or starts with `flare-` (covers all three flagship repos — the JB repo's tag is `flare-runtime-package`; no extra fetch). Unit test with the three real tag vectors.

### 3.5 TUI download — revision-picker popup

- `PopupMode::FlareRevision` (+ cursor + the revision list snapshot, bound to the same model id as `flare_info`).
- `events/`: 'd' in Flare mode opens the popup; ↑/↓ move; Enter enqueues; Esc cancels; Tab cycles Models ↔ FlareRevisions (GGUF precedent).
- `downloads.rs`: `trigger_flare_download(revision)` — FileSpecs from `FlareRepoInfo` with **commit-pinned** `resolve_url`s, `expected_sha256` from `lfs.oid` (§3.3e), **revision-prefixed filenames + destinations per §3.3d** (same lifecycle parity and pre-flight, including the moved-branch guard); enqueue via the existing `EnqueuePolicy::tui_repository` (constructor verified at `src/engine/enqueue.rs:383`, already used by the repository flow). Registry entries store the commit sha; note `src/registry.rs:303` writes `revision: None` when a revision equals `"main"` — FLARE payload revisions are never `main`, so the field round-trips.
- Popup rendering in `render/popups.rs`; snapshots + manifest update. TUI prints no `flare run` hint — intentional (D1's handoff rides D5, the CLI).

### 3.6 Docs & wiring

- Module wiring is explicit: `mod flare;` + re-exports in `src/api/mod.rs`; `mod gpu;` in `src/main.rs` (AGENTS.md map rows for `src/api/flare.rs`, `src/gpu.rs`, `src/ui/render/flare.rs`, mouse order, new CLI flags).
- `README.md` (FLARE section, flags table, TUI keys), `TESTING.md` (test-target rows for the new surfaces), `plans/README.md` index — same commit as the plan change.
- No version bump, no changelog entries (release commits own those).

## 4. Phases (each lands green: `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + the phase's scoped tests — one filter per invocation, per TESTING.md)

1. **API + models foundation** (Lane A): DTOs, `fetch_repo_refs`/`fetch_revision_tree`/`fetch_raw_file`, detection + `assemble_flare_info_from` pure core, token + fingerprint parsers, `mod` wiring. Fixtures are INLINE const JSON in `src/api/flare_tests.rs` (repo convention is code-carried fixtures — `tests/common/mod.rs`, `src/cli/testutil.rs`; no new on-disk fixture dir): recorded `/refs` + per-revision trees + config/publisher for the three real repos (LIL: `main/gb202/gb10`; lflare: `main/gb202` + tags `v1/v2/v3` incl. the same-filename-different-sha pair; JB: `main/gb202`), plus the synthetic two-revision collision case.
   Accept: `cargo test flare` · `cargo test api`.
2. **GPU module** (Lane B, parallel with 1): `src/gpu.rs` per §3.2, inline CSV fixtures, pinned table test incl. GB203/205 absence + both device-id renderings + `parse_gpus_spec`/`resolve_cuda_visible` edge cases (empty, malformed, range inversion, MIG/UUID tokens, out-of-range).
   Accept: `cargo test gpu`.
3. **CLI preset** (Lane C, after 1+2 merge): new flags + conflicts (args surface tests, help snapshots), revision resolution + commit pinning, semantic selection, revision-scoped destinations + pre-flight conflict check (two-revision fixture test through the resolve layer), `final_path_for` extraction + parity test, gated handoff reporter **unit tests for packed, directory, partial-suppression, and conflict-error cases**, `Event::Resolved` optional fields + serialization tests.
   Accept: `cargo test cli_surface_tests` · `cargo test args_tests` · `cargo test help_snapshot` · `cargo test --test cli_download` · `cargo test events`.
4. **TUI viewer** (Lane D, after 1 merges; parallel with 3): display mode + pane + pinned pane-set test, state + ownership rule + reversed-completion test, scan wiring, ApiCache map, `render/flare.rs` + snapshots + manifest, badge (§3.4a), mouse areas.
   Accept: `cargo test render` · `cargo test snapshot` (insta review).
5. **TUI download popup** (Lane D cont.): `PopupMode::FlareRevision`, key routing, `trigger_flare_download` (commit-pinned URLs, sha forwarding, revision-scoped destinations, pre-flight), registry round-trip tests.
   Accept: `cargo test downloads` · `cargo test enqueue`.
6. **Docs + live smoke** (in-session): README, AGENTS.md, TESTING.md, plans index; §5 smoke; full `cargo test --locked`.

## 5. Acceptance

Commands (every phase + ship):

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Live smoke (documented here, executed at ship; metadata-only, no payload bytes):

1. TUI: select each of `local-inference-lab/Qwen3.8-Flash-Next-FLARE` (gb202+gb10 cards), `lflare/Qwen3.8-Flash-Next-FLARE-Abliterated` (v1/v2 tags + gb202(=v3) alias, directory-form package), `JohannesBertens/Qwen3.8-Flash-Next-FLARE-1M-TP2` (gb202 packed card + detected-platform star on GB202 hardware). 'd' → popup → Esc (no download).
2. CLI resolution-only: `rhd download <repo> --for flare --revision gb202 --include 'no-such-glob'` ⇒ zero-files-selected error AFTER exercising `/refs` + per-revision tree + package selection (metadata pipeline proven live; no payload).
3. CLI detection on GB202 hardware: `rhd download JohannesBertens/Qwen3.8-Flash-Next-FLARE-1M-TP2 --for flare --gpus 0,1 --include 'no-such-glob'` ⇒ auto-selects `gb202` (revision + commit echoed), same zero-file termination.
4. One `HF_ENDPOINT=https://hf-mirror.com` run of step 2 (informational mirror-parity note, not a gate).

## 6. Risks

- **GB203/GB205 mis-selection** would ship a broken package: device-ID-only mapping, pinned absence test, unknown⇒`--revision` error.
- **`pci.device_id` rendering variance**: both 32-bit and bare forms parsed (fixture-tested both); verified on real hardware in smoke step 3.
- **Mirror parity for `/refs`+`/tree`**: informational smoke only; failure degrades to the `--revision` path.
- **Scan latency** (refs + ≤4 per-revision trees): shallow trees (≤63 entries), cached with TTL; non-FLARE repos pay at most one config.json raw fetch when `publisher.json` is present.
- **API drift on `/refs`**: serde defaults; a refs entry without a usable `targetCommit` drops that revision from the scan (§3.1) — the selected-revision ladder still resolves via `resolve_revision_sha`.

## 7. Lane decomposition & merge order

- Wave 1 (parallel): **A** = §3.1 (`flare/api` branch); **B** = §3.2 (`flare/gpu` branch). Disjoint files.
- Wave 2 (parallel, after wave 1 merges): **C** = §3.3 (`flare/cli`); **D** = §3.4+§3.5 (`flare/tui`). Disjoint files.
- Every lane: 60-min timeout, commit per increment, push after every commit, merged into `feature/flare-models` in-session the moment its gates pass.

## 8. Interface contract (wave-2 lanes code against this)

§3.1 signatures (`fetch_repo_refs`, `fetch_revision_tree`, `fetch_raw_file`, `flare_disposition`, `assemble_flare_info_from`, `fetch_flare_info`, `parse_flare_package_tokens`) + the `FlareRevision { revision, aliases, kind, target_commit, package, parsed }` shape (target_commit is load-bearing), and §3.2 (`FlarePlatform`, `platform_from_device_id`, `parse_gpus_spec`, `resolve_cuda_visible`, `GpuDetectError`, `detect_platform`) are the contract; wave-1 tests pin them. Drift resolves in favor of wave 1's merged types, plan amended in-commit. The §3.3d destination rule and §3.3e sha-forwarding requirement bind BOTH wave-2 lanes.

## 9. Plan-gate record

- **r2 (7f6daeb, fix-confirmation round):** codex-reviewer BLOCK (8/9 ADDRESSED; new: N1-P0 missing-commit fallback contradiction, N2-P1 registry/resume path parity, N3-P1 moved-branch stale reuse, N4-P1 explicit-sha vs refs validation, N5-P2 platform lifetime) · copilot-reviewer BLOCK (4/4 ADDRESSED; new P2: name-vs-commit scoping = N3, missing `args_tests`/`help_snapshot` gates) · gemini38-reviewer OK-with-notes (both P1s ADDRESSED; P2-A directory-explicit-path claim is inference, P2-B/P2-C notes) · qwen-reviewer OK-with-notes (10/10 ledger ✓, citation nits, §9 codex artifact gap).
- **r3 (this revision):** N1 — commitless refs entries DROP the revision + `resolve_revision_sha` fallback ladder (§3.1, §3.3b i-iii); N2 — destinations via revision-prefixed filenames riding the existing filename-derived lifecycle (engine join `mod.rs:227`, registry `validate_and_sanitize_path`, resume reconstruct) + interrupted nested round-trip fixture (§3.3d); N3 — moved-branch registry guard in pre-flight + fixture (§3.3d); N4 — explicit 40-hex `--revision` used directly as pin, dir segment = sha (§3.3b i); N5 — `flare_platform` session-scoped, per-model clears apply to `flare_info`/popup only (§3.4); gemini P2-A — directory-form handoff labeled inference with Phase-3 verification + fallback wording (§3.3f); copilot — phase-3 gates += `args_tests` + `help_snapshot` (§4.3); qwen — citation fixes (`mod.rs:227`, full paths), r1/r2 codex artifacts persisted under `.review/flare-plan-gate/`.
- **r1 (98552ff, all four lenses):** codex-reviewer BLOCK (5×P0: wrong CLI baseline/preset-validator ownership; journal-identity≠payload-verification; revision-unsafe shared destinations; DTO missing commit pin; handoff completeness/final-path gap — plus 4×P1: NDJSON fields, GPU error ownership/edge cases, TUI scan ownership, phase gate commands) · copilot-reviewer BLOCK (P1 gate commands; P2 module wiring, fixture placement, raw-file LFS guard) · gemini38-reviewer OK-with-notes (P1: inline drop-in rationale, badge landing spot; P2: handoff test pin, TUI-hint intent, fingerprint wording, per-repo fixture naming) · qwen-reviewer OK-with-notes (14/14 spot-checks ✓; P1: phase-3 command matches nothing, §2 root-cause mechanism misdescribed; P2: sha claim scope, registry revision nuance, fixture convention, TESTING.md row, PopupMode nit).
- **r2 (this revision):** every P0/P1 addressed — CLI surface rewritten as new flags + per-command validators + conflicts (§3.3a); verification claim corrected + sha-forwarding requirement (§3.3e); revision-scoped destinations + pre-flight conflict rejection + collision fixture (§3.3d, §4.1, §4.3); `target_commit` pinning through trees/URLs/registry (§3.1, §3.3b); gated handoff + `final_path_for` path source + quoting + no-`--gpus` behavior (§3.3f); NDJSON optional fields specified (§3.3f); structured repo-agnostic GPU errors + edge-case fixtures (§3.2); TUI stale-scan ownership rule + reversed-completion test (§3.4); gate commands one-filter-per-invocation (§4); drop-in rationale inlined (header); badge work item (§3.4a); fixtures moved inline + per-repo named (§4.1); TESTING.md row + module wiring (§3.6); root-cause wording fixed (research facts, §2).
