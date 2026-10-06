# Implementation Plans Index

Historical archive and active proposals for `rust-hf-downloader`.
Plans are kept after completion as design history — each carries a status
header at the top of the file.

> **Convention (keep this index current):** when you add or change a plan,
> update its `Status:` header and this index in the same commit. Superseded
> or removed-feature plans stay in the archive section, clearly marked —
> never silently delete them, never let them read as current.

## Active proposals

| Plan | Status | Summary |
|---|---|---|
| [hf-cache-sync.md](hf-cache-sync.md) | implemented on `feat/hf-cache-sync` (L1 implementation plan) | `hf-cache sync`/`path`: populate the real HuggingFace hub cache (`blobs`/`snapshots`/`refs`) via the existing engine with a staging→publish pipeline, `--for vllm` preset, hub interop matrix, container patterns |
| [hf-cache-vllm-dropin.md](hf-cache-vllm-dropin.md) | proposal (research & strategy) | How vLLM downloads models through huggingface_hub; L0/L1/L2 comparison; L2 `serve` proxy risk register; container deployment patterns. L1 detail lives in hf-cache-sync.md |
| [architecture-simplification-review.md](architecture-simplification-review.md) | proposed 2026-10 (branch `refactor/architecture-simplification-review`) | Post-W-wave architecture plan from a four-lane parallel review: registry single-writer + atomic save, engine DAG (auth-status → models, `VerificationHub`), `EngineState` bundle regrouping, docs truth floor (TESTING.md/lock hierarchy/`docs/DEFERRED.md` register), UI layering fixes, `EnqueuePolicy` collapse, hub-cache ownership + test organization. Milestones M0–M6, one PR each |

## Shipped (kept as history)

| Plan | Shipped in | Summary |
|---|---|---|
| [self-update.md](self-update.md) | v2.10.0 | `rust-hf-downloader update`: `latest.json` manifest via the rate-limit-free `releases/latest/download` CDN redirect, SHA256 verification, `self_replace` atomic swap, `RHD_UPDATE_BASE` mirror override (TUI phase 2 remains future work) |
| [add-cli.md](add-cli.md) | v2.3.0 | One-shot `download` subcommand (later joined by `search`); shared `engine.rs` extraction — the anti-drift successor to the v1 headless CLI |
| [cross-platform-paths.md](cross-platform-paths.md) | v2.6.0 | `paths.rs`: env overrides > portable mode > `dirs` defaults > temp fallback; Windows-safe sanitization |
| [fix-issue-25-quant-detection.md](fix-issue-25-quant-detection.md) | v2.4.0 | Quant detection from the full recursive file tree (subdirectory GGUFs, `MMPROJ`, `mxfp4_moe`, `OTHER`) |
| [remove-headless-cli.md](remove-headless-cli.md) | v2.0.0 | Removal of the v1 headless CLI (~1,730 lines) after its duplicated download-manager bootstrap drifted |
| [readability-maintainability-refactor.md](readability-maintainability-refactor.md) | v2.13.1–v2.13.2 (PRs #43–47) | The W1–W5 wave: submodule facades (`cli/`, `engine/`, `models/`, `api/`, `download/`, `ui/render/`, `ui/app/events/`), sealed `EnqueuePolicy`, pure render pass, typed registry ops, `OPTIONS_FIELDS`; its §8 deferred-defects list is superseded by [docs/DEFERRED.md](../docs/DEFERRED.md) (M0 of the plan above) |

## Archive — removed-feature era (v1.x headless CLI)

The headless mode these documents describe shipped in v1.x and was **removed
in v2.0.0** (see [remove-headless-cli.md](remove-headless-cli.md) and the
v2.0.0 release notes). Kept verbatim for history:

- [add-headless.md](add-headless.md) — master plan
- [implementation/add-headless-phase1.md](implementation/add-headless-phase1.md) …
  [phase5](implementation/add-headless-phase5.md) — phase-by-phase breakdown
- [phase2-completion-summary.md](phase2-completion-summary.md) …
  [phase5-completion-summary.md](phase5-completion-summary.md) — completion notes

## Archive — early architecture work

- [architecture-simplify.md](architecture-simplify.md) — the v2.0.0-era
  modularization that produced today's `src/` layout
- [fix-mutex-usage.md](fix-mutex-usage.md) — mutex-consolidation notes; the
  authoritative, current locking contract is the **lock-ordering hierarchy**
  in the root [AGENTS.md](../AGENTS.md)
