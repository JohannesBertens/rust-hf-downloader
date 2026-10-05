# Release Notes - Version 2.11.0

**Release Date**: unreleased

## New features

### `rust-hf-downloader hf-cache` — populate the real HuggingFace hub cache

A new subcommand pair that writes the **standard hub cache layout**
(`models--<org>--<name>/{refs,blobs,snapshots}`) instead of the flat
`~/models` tree, so vLLM, transformers, and `huggingface_hub` tooling find
the model — byte-identical, revision-pinned — without any network access:

```bash
# Fetch only what vLLM reads, pinned to a commit
rust-hf-downloader hf-cache sync Qwen/Qwen2.5-7B-Instruct --for vllm --revision <commit-sha>

# Serve fully offline
HF_HUB_OFFLINE=1 vllm serve Qwen/Qwen2.5-7B-Instruct

# Where does the snapshot live? (pure path math, no network)
rust-hf-downloader hf-cache path Qwen/Qwen2.5-7B-Instruct
```

How it works:

- `hf-cache sync <MODEL_ID> [FILE…]` downloads through the existing engine
  (chunked parallel downloads, SHA256 verification, rate limiting, resume)
  into a staging directory **inside** the repo folder, then publishes each
  file atomically: `rename(2)` into `blobs/<oid>`, then a **relative**
  `../../blobs/<oid>` symlink under `snapshots/<commit-sha>/` — the layout
  survives any mount point, and concurrent offline readers never observe
  partial files. Blobs are content-addressed exactly like `huggingface_hub`
  (LFS → sha256, non-LFS → git blob sha1), so `hf download` resumes from
  them and `hf cache` scans our output cleanly.
- Selection precedence mirrors `hf download`: positional files →
  `--include`/`--exclude` globs (Python-`fnmatch` semantics, `*` crosses
  `/`) → `--for vllm` preset → whole repo. The `--for vllm` preset is a
  data table (safetensors + configs + tokenizer files in; `original/**`,
  `*.bin`, `*.pt`, `*.gguf`, `*.onnx`, … out).
- Revision pinning (`--revision` branch | tag | 40-hex SHA) writes
  `refs/<branch>` like the hub does; a moved branch fetches the new commit
  into a fresh snapshot dir and keeps the old one.
- Idempotent by construction: re-running the same sync is a no-op
  network-wise (blob present + size match ⇒ skip, missing symlink ⇒
  relink). `--force` re-downloads, `--dry-run` prints the plan, and a
  verification mismatch never publishes bad bytes into the cache.
- Output parity with the `hf` CLI: the human summary's last line is the
  snapshot path; `--json` adds `SyncPlanned`, `FilePublished`, and
  `SyncComplete` NDJSON events. Exit codes stay `0` / `64` / `1`
  (usage errors include unknown revisions and empty selections).
- Cache location honors the hub precedence chain: `--cache-dir` >
  `$HF_HUB_CACHE` > `$HUGGINGFACE_HUB_CACHE` (deprecated, warns) >
  `$HF_HOME/hub` > `~/.cache/huggingface/hub`. A spec-compliant
  `CACHEDIR.TAG` is written so backup tools skip the cache.
  `--no-symlinks` (or `HF_HUB_DISABLE_SYMLINKS=1`) copies files into
  `snapshots/` instead — hub fallback mode for Windows-without-dev-mode.

### Container recipes for offline model serving

Two heavily commented examples ship under `examples/`:

- **Pattern A — shared-volume prefetch**: `examples/k8s/vllm-prefetch.yaml`
  — a Kubernetes Deployment whose init container syncs the model into a
  shared volume (emptyDir single-node; commented PVC variant for
  multi-node labs), with the `vllm/vllm-openai` container mounting it
  read-only under `HF_HUB_OFFLINE=1`.
- **Pattern B — baked image**: `examples/docker/Dockerfile.baked` — a
  multi-stage build that `COPY`s a locally built release binary, syncs the
  model at build time (gated-repo tokens via BuildKit
  `--mount=type=secret`), and ships `vllm/vllm-openai` with the cache and
  offline mode baked in.

## Internal

- `paths.rs`: `hf_hub_cache()` + `write_cachedir_tag()` — hub-parity cache
  resolution and the backup-tool marker
- `patterns.rs`: the single fnmatch-parity matcher shared by selection and
  the preset table (CPython doc vectors in its unit tests)
- `hf_cache.rs`: the layout core (repo dir naming, blob naming incl.
  computed git sha1 fallback, snapshot symlinks, refs, sync lock with
  staleness handling) — pure functions + fs ops, unit-tested against
  fixture trees; end-to-end `hf-cache sync` coverage runs against the
  in-repo mock HuggingFace server
- Documentation: README section "Use with vLLM / transformers (HF cache
  drop-in)", AGENTS.md module map entries, agent-skill `hf-cache` section
  for model-serving workflows
