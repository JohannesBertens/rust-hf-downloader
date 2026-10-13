Subagent timed out after 900000ms.

Recovery summary:
- termination: timed-out
- changed tracked files: none
- session file: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/2026-10-13T05-46-02-323Z_01a133cb-e2d3-7406-bd27-ef7eaa27f17a/d9adb9c7-2bf9-4e8b-899b-b2415d154743/run-0/session.jsonl
- transcript: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/subagent-artifacts/d9adb9c7-2bf9-4e8b-899b-b2415d154743_reviewer_0_transcript.jsonl
- output artifact: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/subagent-artifacts/d9adb9c7-2bf9-4e8b-899b-b2415d154743_reviewer_0_output.md
- metadata artifact: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/subagent-artifacts/d9adb9c7-2bf9-4e8b-899b-b2415d154743_reviewer_0_meta.json
Warning: Inspect partial changes before retrying or resuming the child.

Partial output before timeout:
I have all the evidence I need. Compiling the review now.

---

# Review: fix for issue #50 — resumable downloads (`/home/johannes/projects/rust-hf-downloader/.review/fix-glm-5.3-flash.md`)

## Overall verdict: **APPROVE-WITH-CHANGES**

The design is sound and matches the amended plan: fail-safe journal validation (all-or-nothing trust), data-before-journal crash ordering, eager journal persistence, correct mid-chunk resume math, and a strong test suite (5 e2e scenarios on wire-level `RangeLog` truth + pure validator matrix + a deterministic in-process worker test for the byte-granular path). I found **no corruption or wrong-bytes path**. One shipped contract violation (journal `record()` executed while holding the tokio progress mutex — the exact thing the diff pins in three places as forbidden) and a handful of stale-doc/hardening items keep this from a clean APPROVE.

## Checklist answers

**1. Correctness — OK (with one hardening note).**
Validation (`journal.rs:151-198`) gates every resume: url/total_size/chunk_size equality, offsets length == `div_ceil(total,chunk)`, per-offset bounds against `chunk_len`, on-disk length ≥ highest claimed byte, sha256/etag when both sides present. I could not construct a bypass that resumes into wrong bytes: the fresh arm deletes both files (`chunked.rs:131-138`); the foreign-file case (`len > total_size`, `chunked.rs:108-109`) bypasses the journal entirely and truncates; `validate` returns false on missing data file, so journal-alone is never progress. Under-report direction holds: ticks record after `write_all` completes (`chunked.rs:716`), completion records after `flush`+`sync_all` (`chunked.rs:741-743`). The monotonic guard (`journal.rs:243-249`) is correct defense; note (inference) a true post-`abort_all` zombie record is nearly impossible since `record` is synchronous and aborts land at await points — the real residual is a dead writer's dirty in-memory state being silently dropped, which is under-report (safe). Known accepted gaps (plan-documented): page-cache window on power loss for mid-chunk offsets (sha256 backstops LFS) and no `If-Range`.

**2. Concurrency — CONCERN.**
Terminal-leaf discipline is honored everywhere **except the one hot call site**: the progress tick. `chunked.rs:698` acquires `progress.lock()` and the guard lives to the end of the `if elapsed >= interval_secs` block — so `journal.record(...)` at `chunked.rs:716` runs **while the progress mutex is held**, directly contradicting the comment at `chunked.rs:712-715`, journal.rs module docs, root AGENTS.md, and plan §3.1. No deadlock (order is always progress→journal; nothing takes journal→progress; no await under the journal mutex), but a sync write+fsync+rename executes under the render lock up to once per active chunk per 200 ms interval — the exact stall the doc says must not happen. The completion site (`chunked.rs:743`) is clean. `wait_for_chunks` interaction is safe: aborted tasks can't run more code after their next await, writers don't outlive their attempt, and the retry builds a fresh writer from the persisted (under-reported) journal.

**3. chunked.rs changes — OK.**
Resume math is right: `bytes={start+resume_offset}-{stop}` (`chunked.rs:615`) with `resume_offset < chunk_total` guaranteed (completed chunks skipped at `chunked.rs:430-434`, `> chunk_total` rejected by validation), seek at `start+resume_offset` (`:643`), counter seeded at `resume_offset` (`:634`). The 206 assert (`:627-633`) fires after `error_for_status` so 416/5xx keep their richer errors; short-read detection (`:726-736`) correctly catches the advertised_size_mismatch straddle case (mock clamps `end` at `tests/common/mod.rs:345` and still answers 206). Progress seeding is complete and consistent: `downloaded`/`chunk_completed` (`:171-191`), `progress_downloaded` + `last_downloaded_bytes` both seeded with `resumed_bytes` (`:426-433`), chunk entries start at `resume_offset` (`:455-463`). Two one-sided gaps noted below (findings 3-4): no Content-Range echo check on the chunk response, and no over-delivery cap.

**4. mod.rs changes — OK.**
`prepare_download_paths` (`mod.rs:262-279`) covers all three cells: keep both (journal present), delete `.incomplete` without journal, delete orphan journal without `.incomplete`. A "lying" journal cannot corrupt a later run because `download_chunked` re-validates against the fresh probe before trusting anything; the only unvalidated dimension is claimed-byte *content*, which is exactly the plan-accepted no-`If-Range` risk (sha256 backstop for LFS). Retry/terminal/401 arms keep both files (`mod.rs:461-515`); the rename-failure terminal path self-heals next run (all-complete journal → zero chunks spawned → rename retried). Registry semantics untouched.

**5. Failure/cleanup paths — OK.**
Success: journal removed after the rename (`chunked.rs:264-270`); a crash between rename and remove leaves an orphan cleaned by `mod.rs:277-278` (and, in staging flows that skip `prepare`, by `download_chunked`'s fresh arm itself). TUI delete removes the journal only after the `.incomplete` delete succeeded (`downloads.rs:330-336`); an Err leaves an orphan that the next run's prepare cleans. The eager-journal guarantee is met where tests pin it: flush before spawn (`chunked.rs:168`), and probe-stage failures create neither file (consistent with `timeout_then_success`'s `!exists` assertion, `tests/download_failures.rs:315-316`).

**6. Tests — OK.**
The suite pins the contract at the right layer: validator matrix isolates each rule (`journal_tests`, including etag-absent fallback and truncated/out-of-range offsets), writer coalescing/monotonicity/poison tolerance are unit-pinned, and `resume_worker_tests` deterministically pin the byte-granular mechanism (exact wire Range `bytes=1536-2047`, untouched neighbor bytes, journal `[0,1024,0]`, clamped-end short-serve failure) that the 4 KiB e2e chunks can't expose. E2E assertions are wire-level (re-requested ranges from `RangeLog`), flipped `download_failures` assertions match the new contract, and the `concurrent_threads=2` + arrival-counted-script determinism fix from the plan review is applied in scenarios 2-5. Residual load sensitivity (noted, not a blocker): scenario 1's `attempt1.len() >= 2` assumes requests #2/#3 complete under a 1 s timeout; extreme CI load could complete zero chunks and fail the assert loudly (safe failure mode).

**7. Silent contract drift — CONCERN (docs only, no code drift).**
All code-level old contracts were updated (flipped assertions, coverage table, SIGINT message, TUI test fixture). Two doc sites still teach the deleted-file contract: `src/AGENTS