# Plan: `hf-cache sync` — L1, the HuggingFace-cache drop-in writer

**Status:** implemented on `feat/hf-cache-sync` (M1–M5: `hf-cache sync`/`path`, staging→publish pipeline, interop hardening items R1–R6, container docs; M4 `trees/`+`.no_exist` writers and M6 TUI toggle remain future work)
**Goal:** A single new subcommand that populates the **real** HuggingFace hub
cache (`~/.cache/huggingface/hub`) using the existing download engine, so
that `vllm serve <repo_id>` (and transformers, and `hf download`'s resume
logic) find a byte-identical, revision-pinned snapshot and make **zero
network calls** — with our chunked parallel downloads, SHA256 verification,
and rate limiting doing the fetching.

**Scope is L1 only.** Research, the L0/L1/L2 comparison, xet analysis, and
the deferred `HF_ENDPOINT` proxy live in
[hf-cache-vllm-dropin.md](hf-cache-vllm-dropin.md) (§0 = research summary,
§4 = L2). This document is the implementation plan.

---

## 1. Scope and non-goals

**In scope**

- `rust-hf-downloader hf-cache sync <MODEL_ID> …` — write
  `models--<org>--<name>/{refs,blobs,snapshots}` (+ `.no_exist`, `trees`,
  `CACHEDIR.TAG`) into the standard hub cache location.
- `rust-hf-downloader hf-cache path <MODEL_ID> [--revision …]` — print the
  snapshot path (pure path math + `refs/` lookup; scripting helper).
- Revision pinning (branch/tag/SHA via existing `parse_revision`), pattern
  selection with `hf download` parity, `--for vllm` preset, dry-run,
  JSON/human reporters, rate limiting, token handling.
- Container-oriented docs and examples (shared-volume prefetch, baked image).

**Non-goals (documented, not built)**

- `serve` / `HF_ENDPOINT` proxy (L2 — see dropin plan §4).
- Datasets/spaces repo types, `refs/pr/*`, upload, `--local-dir` mode.
- Cache *management* (`ls`/`prune`/`rm`) — `hf cache` owns that; we only
  guarantee our output scans cleanly under it.
- Changing the TUI download flow (optional destination toggle is a stretch
  milestone, not part of the core).

---

## 2. User-facing contract

### 2.1 CLI surface

```
rust-hf-downloader hf-cache sync <MODEL_ID> [FILE…]      # FILE = repo-relative paths
    --revision <REV>        # branch | tag | 40-hex SHA   (default: main)
    --for <PRESET>          # "vllm" (see §2.3)
    --include <GLOB>        # repeatable, hf-download semantics (fnmatch)
    --exclude <GLOB>        # repeatable
    --cache-dir <DIR>       # default: paths::hf_hub_cache() (§4.1)
    --no-symlinks           # copy files into snapshots/ (hub fallback mode)
    --force                 # re-download even if the blob already exists
    --dry-run               # list what would be fetched/skipped; no writes
    --token / --json / --quiet / --no-verify /
    --rate-limit[-mbps] / --no-rate-limit                 # existing shared flags

rust-hf-downloader hf-cache path <MODEL_ID> [--revision <REV>] [--cache-dir <DIR>]
```

### 2.2 Selection precedence (deterministic, documented)

1. Positional `FILE…` given → exactly those files (must exist in the tree).
2. Else `--include`/`--exclude` given → fnmatch over the full tree
   (`*` matches `/`, POSIX case-sensitive — Python `fnmatch.fnmatch`
   parity; §7).
3. Else `--for vllm` → the preset from §2.3.
4. Else → **whole repo** (parity with `hf download`), with a hint printed:
   `tip: use --for vllm to fetch only what vLLM reads`.

`--exclude` applies on top of every mode. Empty selection after filtering →
`EXIT_USAGE` with the available file list.

### 2.3 `--for vllm` preset (data table, one place in code)

| | Patterns |
|---|---|
| allow | `*.safetensors  *.json  *.txt  *.model  *.jinja` |
| ignore | `original/**  *.bin  *.pt  *.gguf  *.onnx  onnx/**  *.msgpack  *.h5  *.ot  *.tflite  *.md  .gitattributes` |

Covers config/tokenizer/vocab/merges/sentencepiece/chat-template/index files
and sharded safetensors in subfolders (`*` crosses `/`), while skipping what
vLLM never reads. Mirrors vLLM's own `allow_patterns` logic
(`default_loader.py`: safetensors → bin → pt; index-file refinement) minus
the fallback formats. Presets live in a `const` table — data, not code, so
vLLM drift is a one-line fix.

### 2.4 Output & exit codes

- Human mode: per-file progress (existing), then a summary and, as the
  **last line**, the snapshot path (hf CLI parity):
  `…/hub/models--Qwen--Qwen2.5-7B-Instruct/snapshots/<sha>`.
- `--json`: existing NDJSON event stream, extended with
  `SyncPlanned { files, skipped, total_bytes }`, `FilePublished { path,
  blob }`, and `SyncComplete { snapshot_path, revision, sha }` events.
- Exit codes: `0` success (including fully-cached no-op);
  `EXIT_USAGE` bad model id / empty selection / unknown revision (404);
  `EXIT_FAILURE` download or verification failure. Verification mismatch of
  any selected LFS file fails the sync (no partial publish of that file).

---

## 3. Normative cache-layout contract

Written layout (verified against huggingface_hub docs; see dropin plan §0.2):

```
<CACHE_DIR>/CACHEDIR.TAG                                  # M2: backup-tool marker
<CACHE_DIR>/.locks/models--a--b/<file>.lock               # only while publishing
<CACHE_DIR>/models--a--b/
    refs/<branch-or-tag>          # file containing the 40-hex commit SHA
    blobs/<oid>                   # LFS: sha256 (64-hex) | non-LFS: git blob sha1 (40-hex)
    blobs/<oid>.incomplete        # never left behind by us (staging model, §5)
    snapshots/<commit-sha>/<path> # relative symlink → ../../blobs/<oid>
    .no_exist/<commit-sha>/<path> # M4: empty marker files
    trees/<commit-sha>.json       # M4: hub ≥2.x file-list cache (schema-gated)
    .rhd-staging/<a>/<b>/<path>   # OURS: engine working dir (see §5.2)
```

Rules:

- **R1 blob naming**: LFS file → `lfs.oid` (sha256 of content — the same
  value our verification worker already checks). Non-LFS → tree API `oid`
  (40-hex git blob sha); if absent in the API response, compute
  `sha1("blob <len>\0" ++ content)` at publish time (new `sha1` dep,
  RustCrypto). Blob names are opaque to readers but must match so `hf`
  tooling, later hub downloads, and cross-revision dedup behave identically.
- **R2 refs**: written only when the user requested a branch/tag (never for
  a raw SHA request — hub behavior). Content = resolved commit SHA, no
  trailing newline trimming surprises (hub writes the raw SHA).
- **R3 snapshots**: `snapshots/<sha>/<repo-relative-path>`, symlink target
  is the **relative** `../../blobs/<oid>` (works regardless of where the
  cache is mounted — critical for containers/NFS). Nested repo paths create
  parent directories inside the snapshot dir.
- **R4 symlink-less mode**: `--no-symlinks` or symlink-creation failure
  (Windows w/o dev mode) → file is copied/hardlinked into `snapshots/`
  (hub's degraded mode; warn once, respect `HF_HUB_DISABLE_SYMLINKS=1` env
  as the default for the flag).
- **R5 atomicity**: a file is *published* only after its bytes are complete
  AND verified — `rename(2)` staging→`blobs/<oid>`, then symlink. Concurrent
  readers (a running vLLM in offline mode) never observe partial files.
- **R6 idempotency**: re-running `sync` with the same revision is a no-op
  network-wise: blob present + size match → skip download, ensure symlink;
  missing/incorrect symlink → relink; `refs/` diverged (upstream moved) →
  fetch the new commit's files, add snapshot dir, overwrite `refs/`
  (previous snapshot kept — hub semantics).

---

## 4. Module plan

### 4.1 `paths.rs` — `hf_hub_cache()`

```rust
/// Resolve the HuggingFace hub cache dir.
/// Precedence (huggingface_hub parity):
///   --cache-dir flag > $HF_HUB_CACHE > $HUGGINGFACE_HUB_CACHE (deprecated,
///   warn once) > $HF_HOME/hub > <dirs cache>/huggingface/hub
pub fn hf_hub_cache(flag: Option<&str>) -> PathBuf
```

Also `pub fn write_cachedir_tag(cache_root: &Path)` — create `CACHEDIR.TAG`
per bford.info spec if absent (copy the exact bytes from huggingface_hub's
constant during implementation — do not hand-type the signature).

### 4.2 `api.rs` / `models.rs` — metadata extensions

- `ModelMetadata` += `#[serde(default)] pub sha: Option<String>` (top-level
  commit SHA of `/api/models/{id}` responses).
- `ModelFile`/`RepoFile` += `#[serde(default)] pub oid: Option<String>`
  (tree entries; git blob sha for non-LFS, sha256 for LFS). Serde defaults
  keep every existing fixture parsing.
- New `pub async fn resolve_revision_sha(model_id, revision, token)
  -> Result<String, reqwest::Error>` hitting
  `GET {api}/api/models/{id}/revision/{rev}` → top-level `sha`. (Mirrors
  `HfApi.resolve_revision`; `fetch_model_metadata`'s info call stays
  revision-less, so this is the authoritative SHA source for refs/snapshots.)
- `fetch_recursive_tree` already returns the full tree with LFS info — no
  pagination change needed (hub `Link` pagination is handled by the
  recursive walk).

### 4.3 New `src/hf_cache.rs` — layout writer (pure core + fs ops)

```rust
pub fn repo_dir_name(model_id: &str) -> String;          // "models--a--b"; "models--x" when no namespace
pub fn blob_name(file: &RepoFile, staging: &Path) -> String; // R1 (oid or computed sha1)
pub fn snapshot_dir(cache: &Path, model_id: &str, sha: &str) -> PathBuf;
pub fn snapshot_symlink_target(oid: &str) -> String;      // "../../blobs/<oid>"

pub struct SyncPlan {
    pub sha: String,                     // resolved commit SHA
    pub fetch: Vec<FetchItem>,           // (repo_path, blob_oid, size, sha256/lfs)
    pub up_to_date: Vec<String>,         // blob already present & size-matches
}

pub fn plan(cache: &Path, tree: &[RepoFile], selection: &Selection,
            sha: &str, force: bool) -> SyncPlan;

pub fn publish_one(repo_dir: &Path, item: &FetchItem,
                   staging_path: &Path, symlinks: bool) -> Result<(), PublishError>;
// rename staging→blobs/<oid> (same FS: staging lives inside repo_dir), then
// create snapshot symlink (R3/R4), acquiring .locks/<repo>/<file>.lock (R5).

pub fn write_refs(repo_dir: &Path, revision_ref: Option<&str>, sha: &str) -> Result<(), …>;
pub fn write_no_exist_markers(repo_dir: &Path, sha: &str, probed: &[&str]) -> …;  // M4
pub fn write_trees_json(repo_dir: &Path, sha: &str, tree: &[RepoFile]) -> …;      // M4, schema-gated
```

No I/O in `plan()` beyond `metadata()` calls; everything else is pure and
unit-tested against fixture trees.

### 4.4 `src/patterns.rs` — Python-fnmatch parity

Small matcher with `fnmatch` semantics (`*` → `.*` incl. `/`, `?` → `.`,
`[seq]`/`[!seq]`, case-sensitive on POSIX). Test vectors lifted from the
CPython `fnmatch` docs plus our own cross-`/` cases. Used by selection
(§2.2) and shared with `--include/--exclude`.

### 4.5 `cli.rs` — subcommand wiring

- `Commands::HfCache(HfCacheArgs)` with nested `enum HfCacheCommand {
  Sync(SyncArgs), Path(PathArgs) }`.
- `run_hf_cache_sync` orchestrates (pipeline in §5); reuses the existing
  reporter, token merge, `apply_rate_limit_overrides`,
  `crate::config::apply_options`.
- **No changes to `engine.rs`, `download.rs`, `verification.rs`.**

### 4.6 Deliberately untouched

- `engine::register_pending` is **not** called: the flat-download registry
  is TUI-resume state; cache syncs would pollute it. (Revisit if the TUI
  gains a cache destination — stretch M6.)

---

## 5. Execution pipeline (the core design)

### 5.1 Why staging + publish (instead of direct-to-blob writes)

The engine's destination math is `base/author/model/<file>` with
traversal-safe sanitization, resume via `<final>.incomplete`, and
skip-if-exists with re-verification (`download.rs` L129–364). Rewiring
`DownloadMessage`'s `base_path`+filename to address blobs by hash would
touch the tuple type shared with the TUI and bend
`validate_and_sanitize_path` around its own author/model logic.

Instead: **the engine stays untouched and downloads into a staging dir
inside the repo folder; `hf_cache.rs` publishes atomically after
verification.**

```
<hub-cache>/models--a--b/.rhd-staging/a/b/<repo-path>     ← engine writes here
                          │  (rename, same filesystem — instant, no copy)
                          ▼
<hub-cache>/models--a--b/blobs/<oid>
                          │  symlink
                          ▼
<hub-cache>/models--a--b/snapshots/<sha>/<repo-path>      +  refs/<rev> = <sha>
```

Costs accepted: one extra directory entry per repo (cleaned on success);
rename must stay on one filesystem — guaranteed because staging is *inside*
the repo dir.

### 5.2 `run_hf_cache_sync` step-by-step

1. Validate model id / revision (reuse `valid_model_id`, `parse_revision`).
2. `fetch_model_metadata` (tree + LFS) and `resolve_revision_sha` in
   parallel. 404 on the revision endpoint → `EXIT_USAGE` unknown revision.
3. Build `Selection` (§2.2) → `plan()` → emit `SyncPlanned`. Empty fetch set
   + non-empty up-to-date set → write refs if missing, print path, exit 0.
4. `--dry-run` → print the plan table (hf parity: per-file bytes) and exit.
5. Ensure `CACHEDIR.TAG`, create `models--…/{blobs,snapshots/<sha>,.rhd-staging}`.
6. Acquire the sync lock: `<repo_dir>/.rhd-staging/.sync.lock` via
   create-with-`O_EXCL`; stale detection by mtime (>24h ⇒ steal with warning).
   Serializes concurrent `hf-cache sync` runs on the same repo; hub's own
   `.locks/` files are taken per-file during publish for coexistence with a
   concurrently-running `hf download` (exact hub lock filename verified
   against installed hub during M4 — see open questions).
7. Engine bootstrap exactly like `run_download`:
   `EngineState::new()` → `spawn_manager` → `spawn_verification_worker` →
   send one `DownloadMessage` per fetch item with
   `base_path = <repo_dir>/.rhd-staging`, `filename = <repo-path>`,
   `expected_sha256 = lfs.oid`, `total_size` from the tree → drop the sender
   → await the manager join (`Vec<FileOutcome>`).
8. Await `verification_idle()`, drain `verify_rx` (`Vec<VerifyOutcome>`).
9. **Publish gate** per file:
   - `FileOutcome::Complete | AlreadyExists` **and** (`sha256` absent **or**
     `--no-verify` with warning **or** `VerifyOutcome::Ok`) → `publish_one`.
   - `VerifyOutcome::Mismatch` → delete staging file, record failure, sync
     exits `EXIT_FAILURE` (the bad bytes never enter the cache).
   - `FileOutcome::Failed`/`AuthRequired` → surface as today
     (`AuthRequired` prints the token guidance).
   - Note: `AlreadyExists` files are re-verified when a hash exists
     (existing engine behavior, `download.rs` L322–364), so crashed-then-
     rerun staging files are still gated through SHA256 before publish.
10. `write_refs` (branch/tag only, R2). On full success: delete
    `.rhd-staging` remnants of published files; keep `.incomplete` staging
    files of *failed* files (resume value).
11. Print summary + snapshot path (last line). JSON mode emits
    `SyncComplete`.

### 5.3 Resume & idempotency matrix

| State on re-run | Behavior |
|---|---|
| blob present, size ok, symlink ok | skip (up_to_date), no network |
| blob present, symlink missing/dangling | relink only |
| staging `.incomplete` exists | engine re-fetches the file from scratch (its resume logic restarts, not Range-continues) |
| staging complete file exists | engine skips download, verification re-runs, publish proceeds |
| upstream branch moved | new SHA → new snapshot dir fetched; `refs/` overwritten; old snapshot retained |
| `--force` | re-download regardless; blob replaced atomically |

---

## 6. Container story (ships as docs + examples with M5)

Patterns from the dropin plan §2.7, now concrete:

- **A. shared-volume prefetch (lab default)** — k8s init container / docker
  pre-step runs `hf-cache sync <model> --for vllm --revision <sha>
  --cache-dir /hf`; vLLM mounts `:ro` with `HF_HUB_OFFLINE=1` (and both
  `HF_HOME`+`HF_HUB_CACHE` set when overriding paths). Single-writer rule:
  only the prefetch writes; readers are read-only ⇒ no lock contention even
  on NFS.
- **B. baked image** — multi-stage Dockerfile: `COPY` the binary, `RUN …
  hf-cache sync --revision <sha> --cache-dir /opt/hf` (token via BuildKit
  `--mount=type=secret`), final stage `ENV HF_HOME=/opt/hf
  HF_HUB_CACHE=/opt/hf HF_HUB_OFFLINE=1`.
- Ship `examples/docker/Dockerfile.baked`, `examples/k8s/vllm-prefetch.yaml`
  in M5; README section "Use with vLLM".

---

## 7. Pattern semantics (normative)

- One matcher, `src/patterns.rs`, Python-`fnmatch` parity: `*` matches any
  chars incl. `/`; `?` one char; `[seq]`/`[!seq]`; no `**` special-casing
  (hf `--include "*.safetensors"` crosses directories for exactly this
  reason). Case-sensitive (POSIX `fnmatch.fnmatch` normalizes case only on
  Windows).
- Patterns match the **repo-relative posix path**; anchored implicitly at
  the root (no leading-`/` handling needed — hub semantics).
- `original/**` in the ignore list is redundant-but-harmless under fnmatch
  (`original/*` would suffice); kept for readability, tests assert both.

## 8. Testing plan

| Layer | What | Where |
|---|---|---|
| Unit — layout | `repo_dir_name`, `blob_name` (LFS/non-LFS/computed-sha1 vs `git hash-object`), symlink targets incl. nested paths, refs content, no-symlink mode, `plan()` idempotency matrix (§5.3) | `src/hf_cache.rs` `#[cfg(test)]`, fixtures |
| Unit — patterns | fnmatch parity vectors (CPython docs + cross-`/` cases + preset tables) | `src/patterns.rs` |
| Unit — paths | `hf_hub_cache()` precedence incl. deprecated var warning | `src/paths.rs` |
| Integration | end-to-end `hf-cache sync` against the in-process hyper mock (`tests/cli_download.rs` harness: Range-aware, `HF_ENDPOINT`, revision fixtures): assert full tree layout, refs, symlinks, resume after kill, re-run no-op, `--dry-run`, JSON events, gated-401 path | new `tests/hf_cache_sync.rs` |
| Hub interop matrix (CI-optional, local required) | venv per `huggingface_hub` {0.34.x, 1.x, 2.x}: run `try_to_load_from_cache`, offline `snapshot_download(allow_patterns=…, local_files_only=True)`, `AutoConfig/AutoTokenizer.from_pretrained` offline, `hf cache ls` against our cache. Must pass before each release. | `tests/interop/` shell + pytest driver |
| vLLM smoke (GPU-gated, manual/CI-nightly) | `HF_HUB_OFFLINE=1 vllm serve <repo_id>` on a cache we wrote; assert zero outbound HTTP (proxy-blocked network namespace) | `tests/interop/vllm_smoke.sh` |
| Container | docker build of `examples/docker/Dockerfile.baked` in release CI (linux/amd64+arm64 builders already exist) | `.github/workflows/release.yml` additive step |

## 9. Edge cases & decisions

| # | Case | Decision |
|---|---|---|
| E1 | Repo path containing `..`, absolute, or backslash | rejected by `validate_and_sanitize_path` (already enforced for staging path; snapshot path uses the same validated segments) |
| E2 | Duplicate rfilename across recursive tree walks | dedupe in `plan()` (last wins, sizes must agree) |
| E3 | Gated repo | existing `AuthRequired` outcome + token guidance; docs note `--token`/`HF_TOKEN` |
| E4 | File present in `refs`-pinned snapshot but pruned from tree (force-pushed repo) | new-SHA fetch handles it; old snapshot untouched |
| E5 | Same content in two files (identical oid) | both symlinks → same blob; natural dedup, assert in tests |
| E6 | Symlink creation fails (Windows/registry) | fall back to copy + warn (R4); `--no-symlinks` forces |
| E7 | Cache on NFS, single writer | fine (writes are renames+creates); multi-writer NFS explicitly unsupported (docs) |
| E8 | `--revision <sha>` | resolve endpoint still called to confirm existence; no `refs/` write (R2) |
| E9 | Interrupted publish between rename and symlink | re-run relinks (R6); blob without symlink is harmless |
| E10 | `HF_HUB_DISABLE_SYMLINKS=1` in env | default `--no-symlinks` behavior (hub parity) |

## 10. Milestones & releases

| Milestone | Contents | Ships |
|---|---|---|
| M1 foundations | `hf_hub_cache()`, `CACHEDIR.TAG`, api `sha`/`oid`/`resolve_revision_sha`, `patterns.rs` | internal |
| M2 layout writer | `hf_cache.rs` core + unit/fixture tests + `hf-cache path` | internal |
| M3 the subcommand | `hf-cache sync` end-to-end (§5.2), presets, dry-run, JSON events, integration tests | **v2.11.0** |
| M4 interop hardening | `.no_exist` markers, `trees/<sha>.json` (schema-gated), hub `.locks` naming verification, hub-interop matrix wired into release checklist | v2.12.0 |
| M5 containers & docs | README "Use with vLLM", `examples/docker`, `examples/k8s`, skill + changelog updates | with v2.11.0 |
| M6 stretch | TUI destination toggle (`ui/app/downloads.rs`), cache-aware registry entries | post-v2.12 |

## 11. Open questions (resolve during M1/M4)

1. **`trees/<sha>.json` schema** — undocumented; reverse-engineer from an
   installed hub (fetch a tiny repo with `hf download`, inspect). Gate the
   writer behind a flag until byte-compatible; core drop-in must not depend
   on it (older hubs scan disk; verify hub-2.x fallback in the interop
   matrix).
2. **Exact hub `.locks` filename** (`<file>.lock` vs `<etag>.lock`) —
   read installed `_download.py`; only affects coexistence with a
   simultaneously-running hub client.
3. **Tree API `oid` completeness** — confirm every file entry carries `oid`
   on the real API (mock fixtures will; spot-check real repos incl. non-LFS
   config files). Fallback: computed sha1 (R1).
4. **Staging lock staleness policy** — 24h steal is a first guess; observe
   in CI/integration before freezing.
5. **Non-root container cache ownership** — document the UID 2000:0 /
   group-0-writable recipe; consider `--chown`-style flag only if users ask.
