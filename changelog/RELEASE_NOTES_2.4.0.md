# Release Notes - Version 2.4.0

**Release Date**: 2026-09-25

## Quantization detection overhaul: full-tree classification (#25)

### Problem
Users reported many repos showing **zero quantization types and zero
downloadable files**, making the tool unusable for them:

- `Ex0bit/Qwen3.5-122B-A10B-PRISM-LITE-GGUF` — every GGUF lives in a
  `Dynamic/` subdirectory → empty quant panel, dead end
- `mradermacher/Qwen3.5-27B-heretic-GGUF` — `*.mmproj-Q8_0.gguf` (multimodal
  projector) mixed into the `Q8_0` weight group
- `Sabomako/Qwen3.5-122B-A10B-heretic-GGUF` — `mxfp4_moe` multiparts and
  `mmproj-F32.gguf` silently dropped
- `stepfun-ai/Step-3.5-Flash-Int4` — safetensors-only repo: no quant view, no
  per-file download

### Root cause
`fetch_model_files` walked only the **repo root**: root-level `.gguf` files
plus quant-*named* directories. Meanwhile `fetch_model_metadata` already
fetches the complete recursive tree — the two views disagreed, and files the
root walk couldn't place were dropped **silently**.

### Changes
- **Full-tree classification**: new pure `classify_quantizations(&[RepoFile])`
  over the complete file listing. Precedence per file: mmproj → filename
  quant hint → quant-named ancestor directory → `OTHER`. The extra root-only
  API listing (and per-directory follow-ups) are gone — quant groups derive
  from the metadata fetch the frontends already perform.
- **mmproj groups**: projectors get their own `MMPROJ` / `MMPROJ-<quant>`
  groups so selecting `Q8_0` downloads weights only; `--quant mmproj` selects
  every projector in one go.
- **`mxfp4_moe` recognized** via an underscore-prefix fallback in the
  dot-split path (`model.mxfp4_moe-00001-of-00002.gguf` → `MXFP4`).
- **`OTHER` group**: unrecognized GGUFs stay visible and downloadable instead
  of vanishing (sorted last).
- **Dead-end removed**: a classification yielding no groups falls back to the
  Standard file tree with a status hint instead of empty panels.
- **Tree downloads**: `d` on the Repository Files pane queues the selected
  file or every file under the selected folder (Standard-mode, non-GGUF
  repos); whole-repo `d` on the Models list unchanged.
- **One predicate everywhere**: the three drifted copies of the quant-type
  check are unified into `looks_like_quant_type` — the drift is how MXFP went
  missing from directory heuristics. Consequence: a bare `Q`-prefixed
  directory (`QuickCheck`) is no longer treated as a quantization directory
  (it was a junk-group source).

### Compatibility
- `fetch_model_files` keeps its signature as a compat wrapper.
- CLI exit codes and NDJSON event schema unchanged (skill contract intact).
- Quant group labels: new `MMPROJ*` and `OTHER` types may appear in
  `--quant` selection output.

### Testing
- 133 tests green (112 unit + 21 integration), clippy clean
- Regression fixtures recorded from the live HF API for all four reported
  repos; mock HF server now serves directory-aware tree listings; nested
  `Dynamic/` downloads verify SHA256 end-to-end

### Files Modified
`src/api.rs`, `src/ui/app/{models,downloads,events,state}.rs`, `src/cli.rs`,
`tests/cli_download.rs`, `README.md`, `src/AGENTS.md`,
`plans/fix-issue-25-quant-detection.md`, `Cargo.toml`, `changelog/`
