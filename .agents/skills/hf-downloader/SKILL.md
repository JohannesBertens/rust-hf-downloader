---
name: hf-downloader
description: Download HuggingFace models (specific GGUF quantizations, exact files, or whole repos) with SHA256 verification, and search the HuggingFace hub for model IDs. Use when asked to fetch, download, mirror, or verify HuggingFace models/weights, to resolve a vague model request into a concrete repo, or to prepare a model for serving (populate the hub cache so vLLM/transformers run offline). Requires rust-hf-downloader >= 2.3.0 (search/download); `hf-cache` (>= 2.11.0) writes hub-compatible snapshots; `update` (>= 2.10.0) upgrades the binary in place.
license: MIT
---

# HF Downloader

Non-interactive, machine-friendly HuggingFace downloads. Four subcommands:
`search` (discover a model ID), `download` (fetch it), `hf-cache` (fill the
real hub cache for offline serving), and `update` (upgrade the
rust-hf-downloader binary itself). Never launch the TUI for automation —
the CLI subcommands cover the whole flow with stable JSON contracts and
deterministic exit codes.

## Core loop: probe → select → download → decide

1. **Resolve the model ID** if the user gave a vague description:

   ```bash
   rust-hf-downloader search "qwen 2.5 gguf" --sort downloads --limit 5 --json
   # → one JSON array on stdout: [{"id": "bartowski/Qwen2.5-7B-GGUF", ...}, ...]
   ```

   Zero hits is still exit `0` with `[]` — distinguish by array length.

2. **Probe on purpose.** Invoke `download` with **no selector** and `--json`
   when the repo contents are unknown. A multi-file repo fails fast with
   exit `64` and a structured file list on the last stdout line:

   ```json
   {"type":"error","code":"ambiguous","message":"model has 9 downloadable file(s); …",
    "available":[{"filename":"Qwen2.5-7B-Q4_K_M.gguf","size_bytes":4947802324,"sha256":"…"}, …]}
   ```

3. **Re-invoke exactly once** with a selector picked from `available`:
   `--file <filename>`, `--quant <TYPE>` (derivable from filename suffixes
   like `-Q4_K_M.gguf`), or `--all`.

4. **Decide from the exit code:**

   | Code | Meaning | Skill action |
   |---|---|---|
   | 0 | files present (downloaded or already existed), verified/skipped | proceed |
   | 1 | download failed after retries, or hash mismatch | inspect the final `error` event, surface to user |
   | 2 | auth required | ask the user for a token; re-invoke with `--token` or `$HF_TOKEN`. Never guess tokens |
   | 64 | usage error / unknown file / ambiguous | pick from `error.available`, retry once; after that, ask the user |
   | 130 | interrupted | optional retry — unfinished files restart from scratch |

## Invariants

- `--json` keeps stdout **pure NDJSON events**; the `error` event is always
  the **last line** on failure. Human mode (no `--json`) sends progress to
  stderr and only the summary to stdout, so `2>/dev/null` keeps logs clean.
- **Idempotent re-runs**: existing files are skipped (`file_complete` with
  `"status":"already_exists"`); files with a known hub sha256 (LFS) are
  re-verified on the re-run — small non-LFS files have no sha and are only
  checked for existence. Re-running the same command is a cheap consistency
  check.
- **Selectors are mutually exclusive**: exactly one of `--quant` / `--file`
  / `--all`, or none (only valid for single-file repos). `--quant` is
  case-insensitive and selects **all parts** of a multipart GGUF archive
  (parts are separate files, never concatenated).
- **Token precedence**: `--token` > `$HF_TOKEN` > config file. Do not store
  secrets yourself; pass through or use the environment.
- `HF_ENDPOINT` redirects all hub traffic (mirrors like
  `https://hf-mirror.com`, or local test servers).
- Files land in `<output>/<author>/<model-name>/…` (default `~/models`) and
  are tracked in the registry the TUI shares (`~/models/hf-downloads.toml`).

## Model serving: populate the hub cache (`hf-cache`, >= 2.11.0)

When the target is an inference server (vLLM, TGI, transformers) rather
than loose files, write the **real HuggingFace hub cache** instead of the
flat download layout — the server then starts fully offline:

```bash
# Fetch only what vLLM reads, pinned to a commit (deterministic serving)
rust-hf-downloader hf-cache sync Qwen/Qwen2.5-7B-Instruct --for vllm \
  --revision <40-hex-sha>

HF_HUB_OFFLINE=1 vllm serve Qwen/Qwen2.5-7B-Instruct   # zero hub calls

# Pure path math (no network) — where the snapshot lives:
rust-hf-downloader hf-cache path Qwen/Qwen2.5-7B-Instruct
```

`hf-cache` invariants:

- Output is byte-compatible with `hf download`: `vllm serve`, transformers,
  and the `hf` CLI resolve it natively. The **last stdout line** on success
  is the snapshot path — use `$(…)` to hand it to other tools.
- Selection: positional files → `--include`/`--exclude` fnmatch globs →
  `--for vllm` preset → whole repo. For serving, always pass `--for vllm`;
  whole-repo syncs waste disk on formats vLLM never reads.
- Pin `--revision` to a commit SHA for reproducible serving (branch names
  move). Unknown revisions exit `64`, like other usage errors.
- Idempotent and atomic: re-running the same sync is a cheap no-op (exit
  `0`); files publish only after SHA256 verification. Safe in init
  containers and CronJobs.
- Containers: set BOTH `HF_HOME` and `HF_HUB_CACHE` when overriding the
  location; exactly one writer per cache volume; readers mount read-only
  with `HF_HUB_OFFLINE=1`. Ready-made recipes live in the repo's
  `examples/docker/Dockerfile.baked` and `examples/k8s/vllm-prefetch.yaml`.

## Deep reference

Read these only when needed:

- `references/events.md` — full NDJSON event schema, field tables, error codes.
- `references/cli-reference.md` — every flag for both subcommands, defaults, aliases.
- `scripts/hf-get.sh` — ready-made probe→select→download wrapper (requires
  `jq`); also serves as a regression test of the JSON contract. Run it
  relative to this skill directory.

## Keeping the tool current (>= 2.10.0)

```bash
rust-hf-downloader update --check   # exit 70 when newer exists
rust-hf-downloader update           # verify (SHA256) + swap in place
```

Use `update --check` before long download sessions; the running binary is
replaced only by an explicit `update` (never mid-session). Exit `71` means
checksum mismatch — the binary was left untouched.
