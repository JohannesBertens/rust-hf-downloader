# Plan: Reflect release-pipeline outcomes in README.md

**Status:** implemented 2026-10-14 (docs-only commit on `main`; Tier 1 in
full, Tier 2 = crates.io badge + pin-example bumps — the release-workflow
badge was deliberately skipped per §4.5; §7 checklist line added to
AGENTS.md)
**Goal:** Make the README truthfully reflect what the v2.17.0 release
pipeline established — versions shipped, channels that exist, and the
now-fully-automated release path — without turning the README into a
run log. Fix the drift found along the way (the README changelog table
is 9 versions stale) and prevent the drift from recurring.

---

## 1. Analysis: what the latest pipeline run established

The latest build-pipeline run is **v2.17.0** (run `38530519469`, tag
push 2026-10-14T07:46Z) — **fully green, all 7 jobs**:

| Phase | Jobs | Outcome |
|---|---|---|
| Build matrix | 5 targets (linux x86_64/arm64, darwin arm64/x86_64, windows msvc) | all `success`; tests on the 3 native runners (490 tests, per release notes) |
| Release | `Create GitHub release` | `success` — published 07:52Z with **all 9 assets**: 5 binaries, `SHA256SUMS`, `latest.json`, `install.sh`, `install.ps1` |
| Publish | `Publish crate to crates.io` | `success` — **2.17.0 live on crates.io at 07:53:12Z** (verified via crates.io API) |

Two facts make this run more than "another green release":

1. **It is the first fully-automated release.** The v2.16.0 run had only
   6 jobs (no crates.io publish — that job landed in PR #53 *after* the
   v2.16.0 tag, following the v2.16.0 same-day manual-publish miss).
   v2.17.0 is the first tag where build → release → crates.io ran as one
   pipeline, ~7 minutes end to end. That is a durable capability worth
   stating in the README, not a one-off event.
2. **The release surface is now complete and self-verifying**: every
   install channel the README advertises (one-liner installers, prebuilt
   binaries, `cargo install`, and `rust-hf-downloader update` via
   `latest.json`) was populated by this single run.

## 2. Gap inventory (README vs reality after the run)

| Surface | State | Evidence |
|---|---|---|
| README title | ✅ already `v2.17.0` | bumped in release commit `50fdbba` |
| README changelog table | ❌ **stale at 2.10.0** — missing 2.11.0 … 2.17.0 (9 releases, 2026-09-28 → 2026-10-14) | table top row is `[2.10.0] 2026-09-28` |
| `changelog/README.md` | ❌ missing `## [2.17.0]` entry | file starts at `[2.16.0] 2026-10-13`; `RELEASE_NOTES_2.17.0.md` exists but the index wasn't updated in the release commit |
| Crates.io automation | ❌ not stated anywhere | `Using Crates.io` section is accurate but silent on the new publish job |
| Badges | — none exist (no crates.io version badge, no workflow badge) | top of README is bare |

## 3. Guiding principle

The README reflects **durable outcomes, not run logs**: which versions
exist, which install channels exist and are trustworthy, and where
per-run detail lives (GitHub Releases / Actions pages). A healthy green
run should require **at most one README edit** — one changelog-table
row. The 9-row backlog exists because that row wasn't kept up, not
because the pipeline demands more; fixing the recurrence (§7) matters
as much as fixing the table.

## 4. Changes

### Tier 1 — core (factual completeness; do these)

1. **README changelog table: add the 9 missing rows** (2.11.0 →
   2.17.0). Drafted, ready to paste, in §5. This is the single edit
   that makes the README reflect the pipeline's cumulative outcome.
2. **`changelog/README.md`: add `## [2.17.0] - 2026-10-14`** condensed
   from `RELEASE_NOTES_2.17.0.md` (small-terminal HUD borrow rows,
   fallback notice, 1-char clip fix; tests 489 → 490). This closes the
   same-commit convention miss from the release commit and is the file
   the README table links to for details.
3. **README `Using Crates.io` section: one sentence** noting the crate
   is published automatically as the final job of every release
   pipeline (since v2.17.0), so `cargo install` tracks releases with no
   manual lag. This is the durable statement of fact #1 from §1 — and
   it converts the v2.16.0 incident into a documented guarantee.

### Tier 2 — optional polish (recommend, but separable)

4. **crates.io version badge** under the title
   (`[![Crates.io](https://img.shields.io/crates/v/rust-hf-downloader.svg)](https://crates.io/crates/rust-hf-downloader)`).
   Self-updating: it renders the exact version the latest pipeline run
   published — zero-maintenance reflection of pipeline outcome.
5. **Release-workflow badge** (optional-within-the-optional): the
   Actions badge for `release.yml`. Trade-off: the repo deliberately
   runs no PR/push CI, so the badge only reflects tag runs and will
   read "success" almost always — low information, some value as a
   "releases are automated" signal. Recommendation: take the crates.io
   badge, skip this one unless the badge row looks lopsided.
6. **Bump the version-pin examples** in the one-liner section from
   `--version v2.8.0` to `--version v2.17.0` (two code blocks). Pure
   cosmetics; old pin still demonstrates pinning.

### Explicitly out of scope

- No run-by-run CI status/history in the README — per-run detail lives
  on GitHub; the README states the *mechanism*, not the log.
- No changes to `AGENTS.md` release docs beyond the one checklist line
  in §7 (that file is already current).
- No changes to installers, workflow, or code — docs-only.

## 5. Drafted changelog table rows (ready to paste)

Prepend above the `[2.10.0]` row, matching the existing column style:

```markdown
| [2.17.0] | 2026-10-14 | Small-terminal HUD: strip borrows up to 2 rows from the bottom panels instead of vanishing (full strip at 31 rows), status-bar fallback notice below 30, 1-char speed/ETA clip fix; first fully-automated release incl. crates.io publish |
| [2.16.0] | 2026-10-13 | Rate limiter: rate `0` = unlimited, not a panic (config-file path); crates.io publish automated as a release job (PR #53) |
| [2.15.0] | 2026-10-13 | Resumable downloads (#50): `.incomplete` file + resume journal survive network errors, 401s and SIGINT; re-validated mid-chunk resume across all frontends; chunk workers assert 206 + full delivery |
| [2.14.0] | 2026-10-07 | Architecture M0–M6 (PR #49): registry single-writer + atomic saves, invalid-token warn-not-fail, engine DAG (`VerificationHub`), docs truth floor; tree-fetch/chunk-abort/multipart fixes |
| [2.13.2] | 2026-10-05 | Readability wave complete (PRs #43–47): shared engine facade + sealed `EnqueuePolicy`, module splits, typed registry ops — zero behavior change, byte-pinned; Windows CI green |
| [2.13.1] | 2026-10-05 | `cli.rs` god-file split into `src/cli/`; `--help` double-default fix; plain-progress heartbeat covered by tests |
| [2.13.0] | 2026-10-05 | `--progress <MODE>` for `download`/`hf-cache sync`: `plain` tty-independent progress lines + verification heartbeat for docker/CI logs |
| [2.12.0] | 2026-10-05 | Aggregate multi-file progress line (human mode) + optional `overall` object in `--json` progress events (additive schema) |
| [2.11.0] | 2026-10-05 | `hf-cache` subcommand: `sync` writes the real HuggingFace hub cache (`blobs`/`snapshots`/`refs`, staging→publish, `--for vllm`), `path` for scripts; k8s/docker recipes |
```

(One-liners condensed from the corresponding `changelog/README.md`
sections; if any row feels too dense next to the older rows, trim the
internal-detail clauses first — keep the user-visible lead.)

## 6. Mechanics & verification

- **One docs-only commit to `main`** (no PR needed; matches how release
  docs commits land): `docs(readme): reflect v2.17.0 pipeline outcome —
  changelog table 2.11.0→2.17.0, crates.io automation note`. Docs-only
  commits trigger no CI (release.yml fires only on `v*` tags).
- Verify: `grep -c '^| \[2\.' README.md` increases by 9; table renders
  (column count matches header); `changelog/README.md` top section is
  `[2.17.0]`; sanity-run `cargo test docs_guards` (guards read
  AGENTS.md/plans, this commit shouldn't move any).
- Update this plan's `Status:` header and the `plans/README.md` index
  in the same commit (repo convention).

## 7. Preventing recurrence (the actual root cause)

The drift is procedural, not technical: the release flow's checklist
(`AGENTS.md`, "Release flow: bump Cargo.toml (+lock) → changelog
entries → merge PR → push tag") doesn't mention the README table or the
`changelog/README.md` index, so neither gets touched.

- **Now (recommended):** extend that one checklist line in `AGENTS.md`
  to read `… → changelog entries (notes file + changelog/README.md
  index + README table row) → merge PR → push tag`. One line, closes
  the loop for every future release.
- **Only if drift recurs:** a tiny generator (e.g. `just readme-changelog`
  or a `xtask`) that rebuilds the README table rows from
  `changelog/README.md` headings. Not worth the machinery while the
  manual step works.
