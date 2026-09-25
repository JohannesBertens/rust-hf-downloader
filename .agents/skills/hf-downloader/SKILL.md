---
name: hf-downloader
description: Download HuggingFace models (specific GGUF quantizations, exact files, or whole repos) with SHA256 verification, and search the HuggingFace hub for model IDs. Use when asked to fetch, download, mirror, or verify HuggingFace models/weights, or to resolve a vague model request into a concrete repo. Requires rust-hf-downloader >= 2.3.0 (both subcommands).
license: MIT
---

# HF Downloader

Non-interactive, machine-friendly HuggingFace downloads. Two subcommands:
`search` (discover a model ID) and `download` (fetch it). Never launch the
TUI for automation — the CLI subcommands cover the whole flow with stable
JSON contracts and deterministic exit codes.

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

## Deep reference

Read these only when needed:

- `references/events.md` — full NDJSON event schema, field tables, error codes.
- `references/cli-reference.md` — every flag for both subcommands, defaults, aliases.
- `scripts/hf-get.sh` — ready-made probe→select→download wrapper (requires
  `jq`); also serves as a regression test of the JSON contract. Run it
  relative to this skill directory.
