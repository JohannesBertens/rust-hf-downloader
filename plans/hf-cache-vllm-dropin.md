# Plan: `rust-hf-downloader` as a drop-in replacement for vLLM's HuggingFace downloads

**Status:** proposal (not yet implemented)
**Goal:** When a model is not available locally, vLLM pulls it through
`huggingface_hub` into the HF cache (`~/.cache/huggingface/hub`). Give
`rust-hf-downloader` the ability to stand in for that download path — same
cache contract, same offline semantics — so `vllm serve <repo_id>` works
unchanged on a cache our binary produced, with our parallel chunked engine,
SHA256 verification, and rate limiting doing the fetching.

---

## 0. Research summary — how vLLM actually downloads a missing model

### 0.1 The launch-time download flow (vLLM main, verified against source)

When `--model` is a repo id (not a local dir), every file arrives through
`huggingface_hub`'s cache machinery. Nothing in vLLM shells out to the `hf`
CLI — it is all Python API calls into `huggingface_hub`/`transformers`:

| Stage | vLLM code | Hub call | Notes |
|---|---|---|---|
| Revision pinning | `transformers_utils/repo_utils.py: resolve_revision()` | `HfApi.resolve_revision()` → writes `refs/<rev>` into the cache | Falls back to cached `refs/` when offline (`HF_HUB_OFFLINE`) or the Hub is down |
| Model config | `transformers_utils/config.py: HFConfigParser.parse()` | `PreTrainedConfig.get_config_dict()` (transformers → `hf_hub_download(config.json)`) | `local_files_only = HF_HUB_OFFLINE` |
| Quant config | `model_loader/weight_utils.py: get_quant_config()` | `snapshot_download(allow_patterns="*.json")` | Same offline flag; guarded by vLLM's own `filelock` |
| Format sniff | `default_loader.py: _prepare_weights()` → `list_filtered_repo_files()` | `HfApi.list_repo_files()` (cached + retried) | Checks for `consolidated*.safetensors` (Mistral format); returns `[]` in offline mode |
| **Weights** | `weight_utils.py: download_weights_from_hf()` | `snapshot_download(allow_patterns, ignore_patterns, revision, local_files_only)` | `auto` format → `["*.safetensors", "*.bin"]` + `*.pt` fallback; `safetensors` → `["*.safetensors"]`; `mistral` → `consolidated*.safetensors`; `pt` → `["*.pt"]`. Under a `filelock` keyed by model name |
| Weight-index refinement | same, before snapshot | `HfFileSystem.ls()` + `hf_hub_download(model.safetensors.index.json)` | If an index exists, downloads it, then restricts `allow_patterns` to the exact shard list from `weight_map` (avoids e.g. `original/` dupes) |
| Tokenizer / processor | `transformers_utils/tokenizer.py` → `AutoTokenizer.from_pretrained` | per-file `hf_hub_download` via transformers | Optional files probed one by one; misses recorded in `.no_exist/` |
| Offline path resolution | `repo_utils.py: get_model_path()` | `snapshot_download(ignore_patterns="*", local_files_only=True)` | Resolves repo id → local snapshot dir using only the cache |

Key facts confirmed in source:

- **`local_files_only` is wired to `HF_HUB_OFFLINE`** everywhere
  (`huggingface_hub.constants.HF_HUB_OFFLINE`), so with
  `HF_HUB_OFFLINE=1` vLLM makes zero HTTP calls and operates purely off the
  cache (vLLM PRs #3125, #4374 introduced this; air-gap guidance in
  issue #39039 and discuss.vllm.ai relies on it).
- vLLM passes its `--download-dir` as `cache_dir` to `snapshot_download` —
  i.e. vLLM's "download dir" **is an HF hub cache**, not a flat folder.
- vLLM force-enables `HF_XET_HIGH_PERFORMANCE` at import
  (`weight_utils.enable_xet_high_performance()`), so hf_xet rides along
  whenever `huggingface_hub ≥ 0.32` is installed (it is, by default).

### 0.2 The cache contract we must reproduce

Layout written by `huggingface_hub` (docs: *Understand caching* / *Hub Local
Cache*), rooted at `HF_HUB_CACHE` (default `$HF_HOME/hub` =
`~/.cache/huggingface/hub`):

```
<CACHE_DIR>/
├── CACHEDIR.TAG                       # cache-dir tagging standard (backup tools)
├── .locks/models--<org>--<repo>/      # per-file lock files during downloads
└── models--<org>--<repo>/
    ├── refs/<branch-or-tag>           # plain file containing the commit SHA
    ├── blobs/<hash>                   # content-addressed file store
    ├── snapshots/<commit-sha>/<path>  # symlink → ../../blobs/<hash>
    ├── trees/<commit-sha>.json        # (hub ≥ 2.x) per-commit file list cache
    └── .no_exist/<commit-sha>/<path>  # empty markers for probed-but-absent files
```

Rules that matter to us:

1. **Blob naming = the file's etag.** LFS files → SHA-256 of content
   (64 hex, == the `lfs.oid` the tree API gives us — *exactly what our
   verification worker already checks*). Non-LFS (git) files → git blob
   SHA-1 (40 hex; the tree API's `oid` field). Blob names are opaque to
   readers (loads follow symlinks), but matching them keeps
   `hf cache` tooling, later hub downloads, and dedup consistent.
2. **`refs/` is the offline anchor.** `try_to_load_from_cache` and offline
   `snapshot_download` resolve a branch/tag → SHA via `refs/<rev>`; the
   SHA then selects `snapshots/<sha>/`. Without `refs/main`, offline
   loads by repo id fail.
3. **Snapshots use relative symlinks** (`../../blobs/<hash>`), including for
   nested repo paths (`snapshots/<sha>/text_encoder/model.safetensors`).
   On filesystems without symlink support (Windows without dev-mode,
   `HF_HUB_DISABLE_SYMLINKS=1`) files are stored directly in `snapshots/`.
4. **`.no_exist`** markers make transformers' optional-file probing
   (tokenizer variants, index files, chat templates) cheap and
   offline-safe. Absent markers are tolerated (probes just miss).
5. **`trees/` (hub ≥ 2.x)** caches the commit's full file list
   (path/size/hash) and enables offline completeness checking
   (`IncompleteSnapshotError` if expected files are missing). Older
   clients ignore it; newer clients fall back to disk scan when absent
   (verify — see risks).
6. Downloads in progress use `blobs/<hash>.incomplete` + `.locks/` files;
   a finished cache needs neither (only writers care).
7. New hub versions dedupe xet files across repos into a top-level
   `blobs/` with a marker file — we never touch that (we don't speak
   xet; see §0.4).

### 0.3 Environment variables (the interop surface)

| Var | Meaning | Our handling |
|---|---|---|
| `HF_HOME` | root (default `~/.cache/huggingface`) | read for cache resolution |
| `HF_HUB_CACHE` | hub cache dir (default `$HF_HOME/hub`); `HUGGINGFACE_HUB_CACHE` deprecated | **honor precedence: flag > `HF_HUB_CACHE` > `HUGGINGFACE_HUB_CACHE` > `HF_HOME/hub` > default** — new `paths::hf_hub_cache()` |
| `HF_HUB_OFFLINE` | zero HTTP; cache-only reads | documented; never required by us (we *are* the writer) |
| `HF_ENDPOINT` | mirror/proxy base URL | already honored by `api.rs` — free interop with mirrors, and the hook for Phase 3 |
| `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN` | auth (HF_TOKEN canonical) | already honored (`merge_token`) |
| `HF_HUB_DISABLE_XET`, `HF_HUB_DISABLE_SYMLINKS` | client toggles | documented for users; symlink-less mode supported by our writer |

### 0.4 Xet (the modern HF download path) and why it doesn't block us

`huggingface_hub ≥ 0.32` bundles `hf_xet` (Rust) and switches xet-backed
files to the CAS protocol against `cas-server.xethub.hf.co`, bypassing
`resolve/` URLs entirely (hub issue #4475). This is triggered **per file by
server-advertised xet metadata**, not forced client-side
(`HF_HUB_DISABLE_XET=1` opts out). Consequences:

- **Cache-writer strategy:** irrelevant — we fetch via plain HTTP
  `resolve/` + Range (what `download.rs` already does) and never write
  xet structures.
- **Proxy strategy (Phase 3):** we must not advertise xet metadata
  (no `X-Xet-Hash`-style headers); clients then stay on the HTTP path.
  Document `HF_HUB_DISABLE_XET=1` as the belt-and-braces recommendation.

### 0.5 `hf download` CLI parity (UX baseline)

`hf download <repo_id> [files…]` with `--revision`, `--include/--exclude`
(fnmatch), `--repo-type`, `--local-dir`, `--cache-dir`, `--dry-run`,
`--token`, `--quiet`; prints the snapshot path as the last line. Our
subcommand should feel equivalent for the model-repo case (repo types beyond
models are out of scope).

### 0.6 Alternatives considered and rejected

| Approach | Why rejected |
|---|---|
| Replace the `hf` CLI binary on PATH | vLLM never invokes the CLI; it calls Python APIs |
| Ship a fake `hf_transfer` wheel / monkeypatch hub | Packaging hack; only accelerates the byte path, doesn't own cache layout; not our binary |
| Point vLLM at our flat download dir (`vllm serve ~/models/foo`) | Works **today**, zero code — but changes the user-facing invocation, loses revision pinning/refs and shared-cache semantics. Kept as "Level 0" documented mode |
| Full xet client implementation | Large effort; only needed to mimic hub's newest transfer path; HTTP `resolve/` remains fully supported server-side |

---

## 1. Integration strategies

| Level | Mechanism | vLLM invocation changes? | Effort | Verdict |
|---|---|---|---|---|
| **L0 — local dir** | `rust-hf-downloader download --all <repo>` then `vllm serve <dir>` | yes (`--model <path>`) | none (exists) | document only |
| **L1 — HF-cache writer** | new `hf-cache` subcommand populates the real HF cache (blobs/snapshots/refs); vLLM/transformers find everything and (optionally with `HF_HUB_OFFLINE=1`) touch no network | **no** — `vllm serve <repo_id>` as-is | medium | ✅ **Phase 1–2, the drop-in** |
| **L2 — local `HF_ENDPOINT` proxy** | `rust-hf-downloader serve` daemon: implements the small hub API surface (`/api/models/...`, `/resolve/...`) backed by our cache+engine; `HF_ENDPOINT=http://127.0.0.1:PORT vllm serve …` | one env var | high | Phase 3, optional |

L1 is the recommended core: it is a *true* drop-in for the download step
(the thing our engine is good at), requires zero changes at vLLM runtime,
and its output is indistinguishable from `hf download`'s. L2 adds
launch-time on-demand fetching and shared serving for arbitrary hf clients,
at the cost of maintaining a header-exact HTTP façade.

---

## 2. Phase 1 — `hf-cache` subcommand (the drop-in)

> **This level now has its own full implementation plan:**
> [hf-cache-sync.md](hf-cache-sync.md) (CLI contract, layout spec, module
> design, pipeline, tests, milestones). The summary below is kept for
> context; where the two differ, hf-cache-sync.md wins.

### 2.1 UX

```
rust-hf-downloader hf-cache sync <MODEL_ID> [files…]
    --revision REV            # branch/tag/SHA (default main) — reuse existing parser
    --for vllm                # preset allow/ignore patterns (see 2.4)
    --include/--exclude PAT   # fnmatch patterns, hf-compatible
    --cache-dir DIR           # default: paths::hf_hub_cache() (see 2.2)
    --no-symlinks             # snapshots get real files (hub's fallback mode)
    --token/--json/--quiet/--rate-limit… --no-verify   # existing flags
    --dry-run                 # list what would download (hf parity)
```

Success prints the snapshot path on the last line (hf CLI parity), e.g.
`~/.cache/huggingface/hub/models--Qwen--Qwen2.5-7B-Instruct/snapshots/1c9…`.

### 2.2 New module `src/hf_cache.rs` (+ `paths.rs` addition)

- `paths::hf_hub_cache()`: `--cache-dir` flag > `$HF_HUB_CACHE` >
  `$HUGGINGFACE_HUB_CACHE` (deprecated, warn) > `$HF_HOME/hub` >
  platform default via existing `paths.rs` machinery. Also write
  `CACHEDIR.TAG` at the cache root (spec-compliant, 43-byte file) if absent.
- Layout writer (pure functions, unit-tested against fixtures):
  - `blobs/<lfs.oid or git-blob-sha1>` — name from tree API `oid`
    (LFS files: sha256 == `lfs.oid`, which the tree/metadata APIs already
    give us and the verification worker already checks; non-LFS: 40-hex
    git blob sha from the tree listing, **or computed locally** as
    `sha1("blob <len>\0" ++ content)` — small files, cheap, removes API
    dependency).
  - `snapshots/<commit-sha>/<path>` → relative symlink
    `../../blobs/<hash>` (create nested parents; `--no-symlinks` copies
    instead).
  - `refs/<branch-or-tag>` containing the commit SHA (only when the
    requested revision was a ref, never for raw SHA requests — matches hub).
  - Download temp: `blobs/<hash>.incomplete`, atomic rename on completion;
    acquire `.locks/models--<org>--<repo>/<filename>.lock` (filelock
    compatible: exclusive create/hold) so concurrent `hf`/vLLM writers
    don't interleave with us.
  - Idempotent re-runs: blob exists + size matches (and sha256 for LFS,
    which we verify anyway) → skip; symlink missing/incorrect → relink.
    This gives `hf-cache sync` "update" semantics for free.

### 2.3 API layer additions (`api.rs`, `models.rs`)

- Extend `ModelMetadata` with the top-level `sha` (commit hash) from
  `/api/models/<id>` — needed to resolve `revision → commit SHA` when the
  tree fetch used a branch name. (Alternatively read `sha` from the
  `?recursive=true` tree response metadata.)
- Extend the tree parser (`RepoFile`) to capture `oid` per file entry
  (git blob sha for non-LFS; sha256 for LFS) — current struct keeps only
  `rfilename/size/lfs`. `fetch_file_tree` already paginates the tree
  endpoint; ensure `expand[]=lfs` (or default payload) includes LFS oids.

### 2.4 `--for vllm` preset

Allow: `*.safetensors *.json *.txt *.model *.jinja` (config, tokenizer,
vocab/merges, sentencepiece, chat templates; `*` in fnmatch matches `/`, so
subfolder weights like `text_encoder/*.safetensors` are covered).
Ignore: `original/** *.bin *.pt *.gguf *.onnx onnx/** *.msgpack *.h5 *.ot
*.tflite *.md .gitattributes`.
Rationale: mirrors vLLM's own patterns (`auto`: safetensors→bin→pt order)
minus what it never reads; keeps `model.safetensors.index.json` (matched by
`*.json`) so offline `filter_duplicate_safetensors_files` works.

### 2.5 Engine reuse

Bootstrap exactly like `cli::run_download` does today: build
`EngineState`, `register_pending` the selected files, run
`spawn_manager`/`spawn_verification_worker`, drain to
`verification_idle`. Only the *sink* differs: instead of
`<output>/<model>/<file>`, the manager writes to
`blobs/<hash>.incomplete` and the sync step publishes blobs/symlinks/refs
after verification passes. Verification source of truth (`lfs.oid`) doubles
as the blob name — a mismatch fails the file before it ever enters the
cache, which is a stronger guarantee than hub's own etag-trust.

### 2.7 Containerized vLLM (Docker/Kubernetes) — deployment patterns

The official `vllm/vllm-openai` convention (docs.vllm.ai/deployment/docker):
bind-mount the HF cache at `/root/.cache/huggingface` (root) or
`/home/vllm/.cache/huggingface` (non-root `vllm` user, UID 2000:0 — volume
must be group-0 writable), pass `HF_TOKEN` as env, and when overriding
locations set **both `HF_HOME` and `HF_HUB_CACHE`** (community-verified:
`HF_HOME` alone silently fails in some vLLM code paths). vLLM's own CI
shares one HF cache across containers via volumes (vLLM PR #4874) — the
shared-cache pattern is blessed upstream.

| Pattern | How | Best for | Caveats |
|---|---|---|---|
| **A. Shared volume + L1 prefetch** (recommended lab default) | init container / k8s Job / host one-shot runs `hf-cache sync <model> --for vllm --revision <pin>` into a named volume or PVC; vLLM pods mount it (ideally `:ro`) with `HF_HUB_OFFLINE=1` | many pods serving the same pinned model; NFS/CephFS labs | prefetch must be the **only** writer (never N pods downloading into one PVC — `.locks` + NFS `fcntl` locking is the classic stall/corruption source); uid/gid must match container user; read-only + offline avoids all lock writes anyway |
| **B. Baked image** | multi-stage Dockerfile: copy the static `rust-hf-downloader` binary, `RUN … hf-cache sync --revision <sha>` into `/opt/hf-cache` (token via BuildKit `--mount=type=secret`); final stage sets `HF_HOME=HF_HUB_CACHE=/opt/hf-cache` + `HF_HUB_OFFLINE=1` | immutable, reproducible, air-gapped, scale-from-zero autoscaling | multi-GB layers → registry cost; model update = rebuild; gated repos need the build secret |
| **C. L2 proxy sidecar/service** | `rust-hf-downloader serve` as a deployment/service; vLLM containers get `HF_ENDPOINT=http://hf-cache:8080` | ephemeral stateless pods, heterogeneous/on-demand models, no pre-provisioning | each pod still materializes its **own** local hub cache (hub clients always write blobs); proxy must answer HEADs instantly (see Phase 3 risks) or pods crash-loop at bootstrap (cf. production-stack issue #310: storage slow to start ⇒ vLLM pod crash) |

README section "Use with vLLM / transformers":
`rust-hf-downloader hf-cache sync Qwen/Qwen2.5-7B-Instruct --for vllm`
then `HF_HUB_OFFLINE=1 vllm serve Qwen/Qwen2.5-7B-Instruct` (offline is
optional — online runs just no-op on the warm cache). Mention L0
(`vllm serve <dir>`) as the no-cache variant. Update the bundled
`hf-downloader` skill and `changelog/`.

---

## 3. Phase 2 — interop hardening

1. **`trees/<sha>.json` writer** (hub ≥ 2.x): reverse-engineer the exact
   JSON schema from an installed `huggingface_hub` (e.g. `hf download` a
   tiny repo and inspect), then write it from our tree listing. Gate
   behind a flag until schema-verified; benefit = offline completeness
   errors and zero-network re-syncs on new hub versions.
2. **`.no_exist` markers**: after sync, probe the common optional set
   (`model.safetensors.index.json`, `tokenizer.model`,
   `chat_template.jinja`, `preprocessor_config.json`,
   `generation_config.json`, `special_tokens_map.json`) against the tree
   listing and write markers for absent ones. Polish, not required.
3. **`hf cache ls` / `scan_cache_dir` interop test**: our-written cache
   must scan cleanly (sizes, refs, revisions) with the real tool.
4. **Windows**: default to hub's no-symlink mode when symlink creation
   fails; CI target already exists (`x86_64-pc-windows-msvc`).
5. **TUI**: allow the TUI download flow to target the HF cache
   (new destination toggle in `ui/app/downloads.rs`) — optional, after CLI
   proves out.

## 4. Phase 3 (optional) — `serve`: local `HF_ENDPOINT` proxy

Endpooint surface (proven viable by hf-mirror.com operating exactly this
way; hub follows cross-host redirects for `resolve/` per hub PR #4739):

- `GET /api/models/<id>/revision/<rev>` → `{sha}` (cached / upstream
  passthrough)
- `GET /api/models/<id>/tree/<rev>?recursive=true` (Link-header pagination)
- `GET /api/models/<id>` (siblings) — passthrough
- `HEAD|GET /<repo>/resolve/<rev>/<path>`:
  - cached → `200` with `Content-Length`, quoted `ETag`,
    `X-Linked-Etag`/`X-Linked-Size` (LFS), `X-Repo-Commit`; Range → `206`
  - not cached → trigger engine fetch; serve streaming or `308` to the
    upstream CDN URL while fetching (auth forwarded for gated repos)
  - **never** advertise xet metadata → clients stay on HTTP; docs recommend
    `HF_HUB_DISABLE_XET=1`
- Value: on-demand fetch during `vllm serve` with zero pre-step, shared by
  every hf client on the machine (`HF_ENDPOINT=http://127.0.0.1:8080`).
- Cost: header-exact emulation across hub versions (httpx-era clients,
  redirect rules, lock semantics). Defer until L1 ships and there's demand.

**L2 risk register** (why this is Phase 3, not Phase 1):

| Risk | Failure mode | Mitigation |
|---|---|---|
| Protocol fidelity (ETag quoting, `X-Linked-Etag`/`X-Linked-Size`/`X-Repo-Commit`, LFS redirect vs direct-serve, `Link` pagination on tree API) | hub client mis-maps errors (`EntryNotFound` vs `RepositoryNotFound` vs `LocalEntryNotFound`) → vLLM silently falls into offline/stale-cache paths | golden-client CI: run real `huggingface_hub` 0.34/1.x/2.x against the proxy in the test matrix; record-and-replay real hub traffic as fixtures |
| Client version drift (requests→httpx move, redirect rules changed in hub PRs #2721/#4648/#4739, retry/backoff semantics) | works with pinned version, breaks on image rebuild picking up newer hub | same matrix; pin per-version behavior behind feature detection where possible |
| Xet bypass | upstream xet metadata passes through → `hf_xet` fetches from `cas-server.xethub.hf.co` directly, bypassing the proxy (hub issue #4475) and breaking air-gaps | sanitize/strip xet headers on every response; document `HF_HUB_DISABLE_XET=1` |
| HEAD latency (`HF_HUB_ETAG_TIMEOUT` default 10s) | proxy triggering a synchronous upstream fetch on a HEAD makes vLLM time out and take fallback paths | answer metadata calls instantly from cache/upstream-API; only GET/Range may block/stream |
| Auth/token custody | gated repos break on redirect; proxy becomes a shared token escrow for anything on the network | forward `Authorization` upstream only; bind to the internal interface; optional per-token allowlist; never log tokens |
| Concurrency (TP ranks × pods requesting the same cold file) | duplicate upstream fetches, interleaved partial writes, corrupt `.incomplete` blobs | single-flight per blob; serve `308` upstream while fetching; reuse existing `.locks` convention |
| Range/206 correctness | resume math off-by-one corrupts client-side partial files | property tests against `hf download` resume; fuzz Content-Range |
| Orchestrator lifecycle | proxy down at pod start ⇒ vLLM treats Hub as unreachable (crash or stale-cache fallback); restart loops | k8s readiness gate / compose `depends_on: condition: service_healthy`; pods can still set `HF_HUB_OFFLINE` with a stale shared cache as last resort |

---

## 5. Testing & validation matrix

| Check | How |
|---|---|
| Layout fixtures | unit tests: symlink targets, refs content, nested paths, no-symlink mode, idempotent re-sync |
| Blob naming | LFS: matches `lfs.oid`; non-LFS: matches `git hash-object` output |
| Hub-version interop | matrix: `huggingface_hub` 0.34.x / 1.x / 2.x in a venv; `try_to_load_from_cache`, offline `snapshot_download`, `hf cache ls` against our cache; confirm disk-scan fallback when `trees/` absent (risk item) |
| vLLM offline smoke | `HF_HUB_OFFLINE=1 python -c "from vllm import LLM; LLM(model='<id>', load_format='auto')"` in a CI-optional job (GPU-gated; otherwise `transformers`-only smoke: `AutoConfig/AutoTokenizer.from_pretrained` offline) |
| Hermetic integration tests | reuse the existing local HTTP test harness (`HF_ENDPOINT` + tree/resolve stubs, as in `tests/`) end-to-end into a temp cache dir |
| Auth/gated | token precedence tests via existing `merge_token` |

## 6. Risks & open questions

- **`trees/` schema and offline fallback**: exact JSON shape undocumented;
  hub ≥ 2.x may require `trees/` for offline completeness. Mitigation:
  verify against installed hub before writing; core Phase 1 ships without
  it and must be proven against the disk-scan fallback (test matrix).
- **Pattern drift in vLLM**: `default_loader` patterns evolve
  (e.g. new formats). `--for vllm` preset is data, not code — keep it in
  one table, easy to update; `--include/--exclude` always available as an
  escape hatch.
- **Gated repos**: `resolve/` needs the token on redirects; engine already
  sends auth — ensure CDN redirect headers/cookies survive (test with a
  gated test repo if available; documented limitation otherwise).
- **Concurrent writers**: we hold hub-compatible `.locks`; vLLM's own
  filelock (model-name-keyed, temp dir) is coarser but compatible —
  worst case is redundant work, not corruption.
- **`hf download` behavior parity edge cases**: PR refs (`refs/pr/N`),
  datasets/spaces repo types — explicitly out of scope for v1 of the
  subcommand; document.

## 7. Work breakdown

| # | Item | Modules |
|---|---|---|
| 1 | `paths::hf_hub_cache()` + `CACHEDIR.TAG` | `paths.rs` |
| 2 | Tree parser `oid` + metadata `sha`; revision→SHA resolution | `api.rs`, `models.rs` |
| 3 | Layout writer (blobs/snapshots/refs/symlinks/locks/incomplete) | new `hf_cache.rs` |
| 4 | `hf-cache sync` subcommand + patterns + `--for vllm` + dry-run | `cli.rs`, `main.rs` |
| 5 | Engine sink adaptation (cache-layout targets, publish-after-verify) | `engine.rs`, `download.rs`, `verification.rs` |
| 6 | Tests (fixtures, hub-venv matrix, hermetic e2e, offline smoke) | `tests/` |
| 7 | Docs + skill + changelog + version bump + release | `README.md`, `.agents/skills/…`, `changelog/` |
| 8 | Phase 2 items (trees/.no_exist/interop/TUI) and Phase 3 (serve) | follow-ups |
| 9 | Container docs: Dockerfile examples for patterns A/B, compose/k8s snippets, `HF_HOME`+`HF_HUB_CACHE` gotcha | `README.md`, `examples/` |
