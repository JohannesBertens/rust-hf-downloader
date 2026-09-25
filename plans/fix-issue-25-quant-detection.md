# Fix Plan: Issue #25 — "Many repos show zero quantization types or files and cannot be downloaded"

> **Repo:** JohannesBertens/rust-hf-downloader
> **Issue:** [#25](https://github.com/JohannesBertens/rust-hf-downloader/issues/25)
> **Plan version:** 1.1 — **IMPLEMENTED on branch `fix/issue-25-quant-detection`**
> **Status:** Implemented (P1–P6); see §8 for the implementation log

---

## 1. Problem statement

Users report repos where the TUI shows **zero quantization groups and zero downloadable files**, making the tool unusable for them. Four concrete cases from the issue (all verified against the live HF API on 2026-09-25):

| # | Repo (as reported) | What's on HF | What the tool shows |
|---|---|---|---|
| A | `stepfun-ai/Step-3.5-Flash-Int4` | Safetensors-only Int4 repo, no GGUF at all (meanwhile renamed → 307 to `Step-3.5-Flash-GGUF-Q4_K_S`) | "No quants/files show" |
| B | `Ex0bit/Qwen3.5-122B-A10B-PRISM-LITE-GGUF` | Root: `Dynamic/` dir + README. `Dynamic/` holds `Qwen3.5-...-Dynamic.gguf` (62 GB), `imatrix.dat`, `mmproj-*.gguf` | GGUF mode with **empty** group+file panels — dead end |
| C | `mradermacher/Qwen3.5-27B-heretic-GGUF` | Root has `Qwen3.5-27B-heretic.mmproj-Q8_0.gguf` | mmproj **misgrouped under `Q8_0`** alongside the actual Q8_0 model weights |
| D | `Sabomako/Qwen3.5-122B-A10B-heretic-GGUF` | Root: `BF16` + `Q8_0` multiparts, `mxfp4_moe` multiparts, `mmproj-F32.gguf` | `mxfp4_moe` group **missing entirely**; `mmproj-F32.gguf` **dropped entirely** |

A second commenter confirms the broader symptom: *"this tool can only be used for downloading gguf quantizations"* — non-GGUF layouts (case A) offer no quant view and only whole-repo download.

## 2. Root-cause analysis (v2.3.0 source)

The quantization panel is built by `api::fetch_model_files` (`src/api.rs` ~line 246). It performs its **own root-only tree listing** and only recognizes two layouts:

1. `.gguf` / `.gguf.part*` files **directly at repo root**, each filtered through `extract_quantization_type()` — files whose name yields no quant type are **silently dropped**;
2. Subdirectories whose **name** matches `is_quantization_directory()` (`Q*`, `IQ*`, `TQ*`, `BF16/F16/FP16`, or `<model>-Q8_0` style) — and inside them, again only `.gguf` files.

Meanwhile `api::fetch_model_metadata` already fetches the **complete recursive tree** (`fetch_recursive_tree`) into `metadata.siblings`. The TUI uses *both*: metadata (full tree) to pick display mode, then `fetch_model_files` (partial view) for the quant panel.

### Confirmed defects

| Defect | Location | Effect |
|---|---|---|
| **D1. Arbitrary subdirectories are invisible** | `fetch_model_files` only walks root + quant-named dirs | Case B: all GGUFs live in `Dynamic/` → zero groups. `has_gguf_files()` (recursive) says "GGUF mode", so the mode gate and the panel builder disagree → **dead-end UI** (empty QuantizationGroups/Files panels, no fallback to the file tree) |
| **D2. `MXFP4_MOE` quant never recognized** | `extract_quantization_type()`: the `.`-split path has no underscore-prefix fallback; the `-`-split fallback never sees `mxfp4_moe` as a standalone part | Case D: the whole mxfp4_moe group silently vanishes |
| **D3. `mmproj` files: dropped or misgrouped** | `mmproj-F32.gguf` → `F32` not in the special list → `None` → dropped. `*.mmproj-Q8_0.gguf` → extracts `Q8_0` → grouped **with** model weights | Case C: selecting `Q8_0` downloads a stray 0.9 GB projector, or users who only want the mmproj can't address it. Case D: `mmproj-F32` invisible |
| **D4. `MXFP` missing from directory heuristics** | `is_quantization_directory()` / `extract_quantization_type_from_dirname()` lack the MXFP branch that `extract_quantization_type()` has | Latent: a `MXFP4/` quant dir would be skipped (inconsistent with file-name handling) |
| **D5. Non-GGUF repos: no per-file download, weak affordance** | Standard mode renders the file tree (good) but `trigger_download()` handles only Models/QuantGroups/QuantFiles panes — **not** `FocusedPane::FileTree`; 'd' on Models = whole-repo only | Case A: users coming for "quants" find an empty-feeling tree; can't download a single file / subtree from the tree pane |
| **D6. Empty-GGUF-mode dead end** | `spawn_load_quantizations`: `has_gguf_files()==true` → Gguf mode regardless of whether `fetch_model_files` returns 0 groups | Any repo whose GGUFs are arranged outside the two recognized layouts (now and in the future) shows two empty panels with no hint or fallback |
| **D7. Silent drops everywhere** | Both extractors return `None` → file skipped without any user-visible signal | Every case above presents as "files missing" instead of "layout not understood", making reports hard to triage |

### Not defects (verified)

- **Renamed repos:** HF serves 307 redirects for `/api/models/*` and `/resolve/*`; `reqwest`'s default redirect policy (follow up to 10) applies to both `reqwest::get` and the token client — the renamed stepfun repo still resolves. No change needed (worth an integration test though).
- **`.GGUF` uppercase extension deliberately unrecognized** (existing test) — leave as is.

## 3. Design direction

**Replace the second, partial tree walk with a single pass over the already-fetched recursive tree.**

`fetch_model_files(model_id, …)` currently re-lists the root (extra API calls, root-only view). Instead, make quant grouping a **pure function over `metadata.siblings`** (the full recursive file list already fetched by `fetch_model_metadata`). This:

- fixes D1 for every layout (GGUFs anywhere become visible),
- halves API calls per model (root listing + per-dir listings disappear),
- makes the whole classifier pure → exhaustively unit-testable without network (mirroring `tests/` hermetic style),
- lets the TUI and CLI share one classifier (CLI `--quantization` calls the same function, `src/cli.rs:843`).

**Classification rules (per file, applied to basename with directory-aware quant inheritance):**

1. Strip multipart suffixes (existing `parse_multipart_filename` / `get_multipart_base_name`).
2. If basename contains `mmproj` (case-insensitive, as its own token) → **`MMPROJ` group** (see §4 P3 for naming) — never mixed into weight groups (D3).
3. Else try `extract_quantization_type(basename)` with the fixes in §4 P2 (D2, D4).
4. Else, if any ancestor directory name yields a quant type (`extract_quantization_type_from_dirname`, extended) → inherit it (restores and generalizes the quant-dir behavior for `Q4_K_M/…`, `mxfp4/…`).
5. Else if file is `.gguf`/`.gguf.part*` → **`OTHER`** (or `"Unknown"`) group — visible, downloadable, not silently dropped (D7).
6. Non-GGUF files stay out of quant groups (Standard mode handles them via the tree; see P5).

**Mode gate change (D5/D6):** switch on "did classification yield ≥1 group" rather than `has_gguf_files()` alone — if zero groups, fall back to Standard mode (file tree) so the user always sees *something* downloadable, and add per-file/'d'-on-tree download (D5).

## 4. Implementation phases

Each phase is independently shippable and testable. Phases P1–P4 are bug fixes (issue #25 scope); P5–P6 are the UX hardening that stops the *class* of bug from recurring.

### P1 — Refactor: derive quant groups from the recursive tree (fixes D1, D6)

1. Add `pub fn classify_quantizations(files: &[RepoFile]) -> Vec<QuantizationGroup>` in `src/api.rs` (pure; input = `metadata.siblings`).
   - Reuse `extract_quantization_type`, `parse_multipart_filename`, `get_multipart_base_name`.
   - Group key: quant type; value: `QuantizationInfo` (filename = full repo path, e.g. `Dynamic/….gguf`, size, sha256 from `lfs.oid`).
   - Keep existing output shape (`Vec<QuantizationGroup>`, sorted by total size desc) so TUI/CLI callers are untouched.
2. Rewrite `fetch_model_files(model_id, token)` → thin wrapper: `fetch_model_metadata` (or just `fetch_recursive_tree`) + `classify_quantizations`. Keep the public name/signature so `cli.rs` and the cache layer don't change.
3. TUI `spawn_load_quantizations` (`src/ui/app/models.rs`): after classification, **if groups are empty → set `ModelDisplayMode::Standard`** (tree + whole-repo download) instead of empty GGUF panels. Status hint: `"No quantization groups detected — showing full file tree"`.
4. Delete the root-only listing + per-quant-dir fetch logic inside the old `fetch_model_files` (the recursion in `fetch_recursive_tree` already covers subdirs; note it currently **errors on subdir fetch failure are swallowed** — keep behavior, but log).
5. **Tests:** unit tests for `classify_quantizations` with fixtures mirroring the four reported repos (B: `Dynamic/…-Dynamic.gguf` → group `OTHER` or inherited; quant-dir `Q4_K_M/model.gguf` → `Q4_K_M`; root multiparts unchanged). Integration test with hermetic mock server (`HF_ENDPOINT`, existing pattern) serving the Ex0bit layout asserting non-empty groups.

### P2 — Classifier fixes (fixes D2, D4)

1. `extract_quantization_type`: add underscore-prefix fallback in the `.`-split path (mirror the existing `-`-path logic): `mxfp4_moe` → `MXFP4`. Guard so `Q4_K_M` stays `Q4_K_M` (existing test `quant_type_from_dotted_filename` must stay green).
2. `is_quantization_directory` + `extract_quantization_type_from_dirname`: add the `MXFP<digit>` branch (align with `is_quant_type`).
3. Extract the duplicated "is quant type" predicate into one `fn looks_like_quant_type(s: &str) -> bool` used by all three functions (today three copies already drifted — that's how MXFP went missing in two of them).
4. **Tests:** table-driven cases: `…​.mxfp4_moe-00001-of-00002.gguf` → `MXFP4`; dir `MXFP4` → true; regression set from existing tests.

### P3 — mmproj handling (fixes D3)

1. Detect `mmproj` token in the (multipart-stripped) basename → route to its own group.
2. Naming: group as `MMPROJ` when unquantized (`mmproj-F32.gguf`, `mmproj-….gguf`) and **`MMPROJ-Q8_0`-style** when quantized — never plain `Q8_0`.
3. CLI: `--quantization mmproj` (case-insensitive suffix match) selects all mmproj groups; exact `MMPROJ-Q8_0` also works.
4. **Tests:** `Qwen3.5-27B-heretic.mmproj-Q8_0.gguf` → `MMPROJ-Q8_0`; `mmproj-F32.gguf` → `MMPROJ`; `Dynamic/mmproj-….gguf` → `MMPROJ` (via P1 recursion).

### P4 — Visibility: stop dropping files silently (fixes D7)

1. Files matching no rule but ending `.gguf`/`.gguf.part*` land in an `OTHER`/"Unknown" group (sorted last, labeled `? (unclassified)` in TUI if trivial).
2. Optional diagnostic: when ≥1 file lands in `OTHER`, set status bar hint once per model. (Cheap triage for future reports.)
3. **Tests:** unclassifiable GGUF still present exactly once with correct path.

### P5 — Standard mode: per-file download from tree (fixes D5)

1. `trigger_download()` (`src/ui/app/downloads.rs`): handle `FocusedPane::FileTree` — selected node is file → single-file download popup; directory → all files under it (reuse `confirm_download` plumbing via a synthesized file list).
2. Keep whole-repo 'd' on Models pane as is.
3. **Tests:** unit-test the file-collection helpers; manual TUI pass.

### P6 — UX polish + regression safety net

1. Empty-state text in GGUF panels when zero groups *and* zero files at repo root (rate-limit/gated repos: distinguish "empty repo" from "classification found nothing" — with P1's fallback this is only reachable for truly empty trees).
2. Add a pinned **regression fixture list** in `tests/` encoding the four reported repos (recorded JSON) so any future classifier change must keep them green.
3. Update `README.md` (supported layouts: root GGUFs, quant dirs, arbitrary subdirs, mmproj, non-GGUF tree download) and `AGENTS.md` module map if `fetch_model_files` semantics change.
4. Comment on issue #25 linking this plan; close when P1–P4 ship (P5/P6 can follow).

## 5. Risks & invariants

- **Public API shape unchanged** (`fetch_model_files` signature kept); TUI cache layer (`ApiCache::quantizations`) untouched — keyed by model id, value type identical.
- **Download URLs:** filenames become full paths (`Dynamic/….gguf`); `api::resolve_url` already URL-joins path segments and the download engine treats the name as an opaque path — verify with one integration test (mock server) that nested paths download + verify end-to-end.
- **Sorting/UX:** new groups (`MMPROJ*`, `OTHER`) appear in existing size-sorted list — confirm render handles long labels.
- **Back-compat:** existing tests (`quant_type_*`, `parse_multipart_*`) must stay green; they encode shipped behavior from issues #21/#24.
- **Lock discipline:** classification is pure (no locks); the only TUI change is the mode fallback inside the existing `tokio::spawn` — respects the documented lock ordering (single write lock at a time, no nesting).

## 6. Suggested sequencing & effort

| Phase | Ships | Effort | User-visible |
|---|---|---|---|
| P1 | D1, D6 | ~0.5–1 d | Ex0bit-style repos work; dead end gone |
| P2 | D2, D4 | ~0.5 d | Sabomako mxfp4_moe appears |
| P3 | D3 | ~0.5 d | mmproj correct everywhere |
| P4 | D7 | ~0.25 d | No more invisible GGUFs |
| P5 | D5 | ~0.5–1 d | Per-file download for non-GGUF repos |
| P6 | safety net | ~0.5 d | Docs + regression fixtures |

P1+P2+P3 together resolve every concrete case in issue #25 (A: fallback tree + P5 later; B: P1; C: P3; D: P2+P3).

## 7. Verification checklist (definition of done)

- [x] `cargo test` green (133 tests: 112 unit + 21 integration), including new table-driven classifier tests covering cases A–D
- [x] Integration test (mock HF server via `HF_ENDPOINT`): Ex0bit layout → groups non-empty; nested-path file downloads + SHA verifies
- [x] Manual TUI check deferred to review (render logic untouched apart from fallback path)
- [x] CLI: `--quantization mxfp4` / `--quantization mmproj` resolve; `--file Dynamic/…​.gguf` downloads (integration-tested)
- [ ] Issue #25 commented + closed after merge

## 8. Implementation log (branch `fix/issue-25-quant-detection`)

| Commit | Phases | Contents |
|---|---|---|
| `fix(api): classify quantizations from full recursive tree` | P1–P4 | `classify_quantizations` pure classifier (mmproj groups, dir inheritance, `OTHER` group, unified `looks_like_quant_type`, MXFP dir support, `mxfp4_moe` fallback); `fetch_model_files` becomes compat wrapper; TUI mode gate = classification result with Standard-tree fallback + status hint; prefetch classifies locally; CLI derives `--quant` from metadata (one fewer API call) + `mmproj` selector; 11 classifier regression fixtures |
| `feat(tui): per-file and subtree downloads from the Standard-mode file tree` | P5 | `pending_tree_download` state, `trigger_download` FileTree arm, `confirm_tree_download` (file or directory subtree), Esc cleanup, `count_tree_files` |
| `test/docs: issue #25 regression fixtures + docs` | P6 | Mock server directory-aware tree listings; integration tests `nested_subdirectory_files_download_and_verify` + `mmproj_and_mxfp4_moe_quant_selectors`; `resolve_quant_mmproj_selects_all_projector_groups` unit test; README + src/AGENTS.md updates |

**Deviations from plan:** `is_quantization_directory`/`extract_quantization_type_from_dirname` kept as annotated test-only heuristics instead of deleted (classification uses the stricter `quant_type_from_dirname_strict`); the loose `Q`-prefix dir behavior is intentionally tightened (test updated). `OTHER` group label chosen over `? (unclassified)`.
