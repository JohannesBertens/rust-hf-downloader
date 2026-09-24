# Queue Visualization — Design Options

> Generated 2026-09-24 · from code analysis of `src/download.rs`, `src/ui/render.rs`, `src/models.rs`, `src/verification.rs`, online graphics-design research (NN/g, Carbon Design System, Microsoft UX Guide, Colin Ware preattentive attributes, Gestalt principles, TUI conventions), and a glm-5.3-flash subagent layout analysis.

## Reference Scenario

Same scene for all three options:

- `model-Q4_K_M.gguf` downloading — 20 chunks total, 8 active lanes, 11 chunks done
- 4 SHA256 verifications running concurrently, 9 queued
- 7 files waiting in the download queue
- 2 files already verified ✓

## Pipeline (actual code behavior)

```
download_tx channel                one active file                verification queue           registry
  (queue: QueueState   ───────►  ~20 chunks, 5–100 MB,  ──────►  Vec<VerificationQueueItem>,  ──────►  Complete /
   {size, bytes}: counts only)    8 concurrent (semaphore)        4 concurrent SHA256 workers        HashMismatch
```

### Data available for rendering

| Level | Fields | Notes |
|---|---|---|
| Download | `model_id`, `filename`, `downloaded`, `total`, `speed_mbps`, `verifying` | `filename` currently not rendered; `verifying` never rendered |
| Chunk | `chunk_id`, `start`, `end`, `downloaded`, `total`, `speed_mbps`, `is_active` | `start`/`end` are dead_code; completed chunks are **removed** from the vec |
| Verification | `filename`, `verified_bytes` (atomic), `total_bytes`, `speed_mbps` | result (✓/✗) goes to status channel only |
| Queue | download: `size` + `bytes`; verification: `size` | **counts only — no item names/sizes exposed to renderer** |

### Problems these designs must fix

1. **Completed chunks vanish** — `retain(|c| c.is_active)` in `download.rs` collapses rows mid-flight: bars jump, ids reorder, box height oscillates. Violates monotonic-progress principle (NN/g, MS UX Guide).
2. **No aggregate view** — lanes used (8/8), chunks done (n/20), verifications running/done/queued, end-to-end ETA: none exist anywhere.
3. **Corner-widget stacking** — two 52-col `Clear`-based overlays (download top-right 13 rows, verification bottom-right 15 rows) collide with each other at ≤28 rows and erase panel content.
4. **Hierarchy inversion** — transient per-chunk speed occupies the glance zone; aggregate ETA/queue buried in a title string; verification (the next pipeline stage) is spatially detached.
5. **Queues are counts only** — "7 queued" gives no bytes, no next item, no tail estimate; `VerificationQueueItem.total_size` unused.

---

## Option 1 — Pipeline Activity Rail *(vertical, evolved from current design)*

Stages stacked in pipeline causality: download feeds verify. Aggregates on top (glance), fixed lane slots below (scan). Lanes never reflow — a finished chunk dims to `✓` in place.

```text
┌─ ACTIVITY ──────────────────────────────────────┐
│ ▼ DOWNLOAD   queue 7 · 38.2 GB remaining        │
│   model-Q4_K_M.gguf                             │
│   ████████████████░░░░░░░░░░░░░░░  46%  32.8MB/s│
│   lanes 8/8 · chunks 11/20 ✓ · ETA ~4m 12s      │
│   file map  ██ 11 done │▓░ active│░░ remaining  │
│   ┌0112131415161718191┬2021222324┬2526272829303─┐
│   │▓▓▓▓▓▓▓▓░▓░▓░░░░░░░│▓░░░░░░░░░│░░░░░░░░░░░░░░│
│   └────────────────────┴──────────┴─────────────┘
│   L1 ✓ 100%   L5 ▓▓▓▓▓▓▓░░ 78%   ...bar ≤8 cells
│   L2 ✓ 100%   L6 ▓▓▓▓▓▓░░░ 63%
│   L3 ✓ 100%   L7 ▓▓▓▓▓░░░░ 51%
│   L4 ✓ 100%   L8 ▓▓▓▓░░░░░ 38%
├─────────────────────────────────────────────────┤
│ ▼ VERIFY   4 running · 9 queued · 2 done        │
│   shard-00001.safetensors  ███████░░  81% ~20s  │
│   shard-00002.safetensors  █████░░░░  57% ~45s  │
│   shard-00003.safetensors  ████░░░░  44% ~1m    │
│   shard-00004.safetensors  ██░░░░░░  18% ~2m    │
│   +9 queued (11.3 GB)                           │
└─────────────────────────────────────────────────┘
```

- **Principles:** monotonic progress (file map only grows `░`→`▓`→`██`), stage color coding (cyan DL / green VF), ≤4 numbers per glance line (working-memory limit)
- **Trade-off:** needs ~20 rows and a 48-col rail; chunk map requires storing `num_chunks` + done-chunk bitmap (currently missing)
- **Best when:** downloading is the user's primary focus

---

## Option 2 — Pipeline Kanban Board *(horizontal, flow-oriented)*

Each file is a card that moves left→right through the pipeline. Spatial position = lifecycle stage — progress needs no reading at all.

```text
┌─ QUEUED 7 ───────┬─ DOWNLOADING 1 ──────────────┬─ VERIFYING 4/9 ─────┬─ DONE 2 ────┐
│ ▸ shard-5.gguf    │ model-Q4_K_M.gguf      46%   │ shard-1 ██████▓ 81% │ ✓ config    │
│   6.2 GB          │ ██████████████░░░░░░░░░░░░   │ shard-2 █████░░ 57% │ ✓ shard-0   │
│ ▸ shard-6.gguf    │                               │ shard-3 ████░░░ 44% │             │
│   6.2 GB          │ 32.8 MB/s · 8/8 lanes ~4m    │ shard-4 ██░░░░ 18%  │             │
│ ▸ shard-7.gguf    │                               │                     │             │
│ ▸ tokenizer.json  │ chunks: ██ done ▓░ act ░ rest│ ▸ +9 queued (11.3GB)│             │
│ ▸ ...+3 more      │ ▓▓▓▓▓▓▓▓▓▓▓░░░▓░░░░░░░░░░░░░ │                     │             │
│                   │ 11/20 ✓                       │                     │             │
└───────────────────┴───────────────────────────────┴─────────────────────┴─────────────┘
  next: shard-5.gguf · verified 2/17 · pipeline ETA ~18m
```

- **Principles:** Gestalt common-region (one bordered column per stage), preattentive spatial position, the whole job visible in one frame — including the tail of the queue
- **Trade-off:** needs ≥100 cols; per-chunk detail sacrificed to the one-strip file map; moving cards need animated reordering
- **Best when:** multi-file jobs dominate (whole-repo downloads) and the terminal is wide
- **Data gap:** queue column needs per-item names + sizes — `QueueState` only counts today

---

## Option 3 — Compact Matrix HUD *(dense, one line per item)*

A fixed-column table strip above the status bar. State glyphs (`DL`/`VF`/`Q `/`✓`) sort rows into stages without any boxes — 12 lines for the entire pipeline.

```text
 DL  model-Q4_K_M.gguf      ████████████░░░░░░░░░░░░░  46%  32.8MB/s  ~4m  ch 11/20 ▓▓▓▓▓▓▓░░░▓░░░░░
 VF  shard-00001.sft         ███████░░░                 81%   1.9GB/s  ~20s
 VF  shard-00002.sft         █████░░░░                  57%   2.1GB/s  ~45s
 VF  shard-00003.sft         ████░░░░                   44%   2.0GB/s  ~1m
 VF  shard-00004.sft         ██░░░░░░                   18%   1.8GB/s  ~2m
 Q   shard-5.gguf           ·                           —    6.2 GB   wait
 Q   shard-6.gguf           ·                           —    6.2 GB   wait
 Q   shard-7.gguf           ·                           —    6.2 GB   wait
 Q   +4 more                ·                           —   25.1 GB   wait
 ── verified 2 ✓ 0 ✗ · hash queue 9 · pipeline ETA ~18m ─────────────────────────────────────────────
```

- **Principles:** preattentive glyph + color in column 1, aligned numeric columns for vertical scan paths, fixed row heights (zero reflow — key invariant from layout analysis), minimal footprint (collapses to a 2-row aggregate strip on narrow terminals)
- **Trade-off:** no per-chunk lanes at all (one `▓` map cell per chunk max); speed sparklines per chunk impossible
- **Best when:** browsing/search remains the primary activity and downloads are background noise

---

## Comparison

| Criterion | 1 Rail | 2 Kanban | 3 Matrix |
|---|---|---|---|
| Glance-level answer | ★★★ | ★★★ | ★★ |
| Vertical budget | ★ | ★★ | ★★★ |
| Chunk fidelity | ★★★ | ★★ | ★ |
| Whole-queue visibility | ★ | ★★★ | ★★★ |
| Implementation cost | medium | high (needs queue items exposed) | low–medium |

## Recommendation

**Option 1** is the best evolution of the existing code — it keeps the corner-widget concept but moves it into a negotiated layout rail (no `Clear` overlay) and fixes chunk monotonicity. **Option 3** is the cheapest and safest first step.

## Shared prerequisites (data-model changes)

Both favored options need:

1. **Keep completed chunks** (or at least a done-bitmap + `num_chunks` field) in `DownloadProgress` — currently `download_chunked` retains only active chunks, so "n/20 done" and a monotonic file map are uncomputable. Touch points: `src/models.rs` (`DownloadProgress`), `src/download.rs` (~629–706 chunk lifecycle).
2. **Expose queue item names/sizes to the renderer** — download queue is `QueueState {size, bytes}` counts only; verification queue items (`VerificationQueueItem` with `filename`/`total_size`) are not surfaced. Touch points: `src/ui/app/state.rs`, `src/ui/app/downloads.rs` (`confirm_download`), `src/verification.rs` (`queue_verification`).
3. Optional: surface hash result (✓/✗) per verification item instead of status-channel-only, and render `filename` + `verifying` flag on the download widget.
