All evidence is gathered. Here is the review verdict document.

---

# Review: `plans/fix-issue-50-resume-downloads.md`

## Overall verdict: **APPROVE-WITH-CHANGES**

The design is sound, well-matched to this codebase's constraints, and correctly anchored to the existing test harness. The root-cause analysis is accurate against the current source. One testability problem (a determinism race in two of the five pinning tests) must be resolved before implementation, plus a handful of small hardening items. None require redesign.

## Checklist verdicts

**1. Journal-based resume design — CONCERN (minor).** The validation rules (url/revision/total_size/chunk_size/etag) are the right set, and the discard-on-mismatch direction is fail-safe. The under-report-only crash-safety claim is verified sound: `record()` is a sync critical section, tasks can only be aborted at await points, and data `write_all` precedes its journal update — so a journal offset never exceeds requested bytes (the acknowledged power-loss page-cache window aside, correctly backstopped by SHA256). Two gaps: (a) the defensive rule (plan §2) bounds-checks each `chunk_offsets[i]` but never requires `chunk_offsets.len() == num_chunks` — a truncated array with a matching `chunk_size` would silently misalign chunk ids to byte ranges (corruption, not just lost progress); (b) the plan never states the journal is **persisted eagerly at download start** (all-zero offsets), which scenarios 2/4/5's journal-exists assertions depend on (see finding 1).

**2. Concurrency — OK.** `JournalWriter`'s std mutex is a leaf: no await inside, nothing else acquired under it, so it cannot join a deadlock cycle with the tokio hierarchy. The plan's claim that `tests/docs_guards.rs::lock_hierarchy_documents_every_engine_mutex_field` is unaffected is verified — the guard derives its field set only from `EngineState`/`QueueAccounting`/`EventBus`/`VerificationHub` (tests/docs_guards.rs:104–152), and the writer lives in `download/` internals. Brief blocking IO under a std mutex in async context follows the `REGISTRY_WRITE` precedent (src/registry.rs:74–100). The existing abort contract (`wait_for_chunks`, src/download/chunked.rs:336–356) makes the in-run retry handoff safe: aborted tasks never re-enter `record()`. Residual notes: pin `record()` outside the progress-lock scope, and specify poisoning policy (finding 5, finding 11).

**3. chunked.rs changes — CONCERN (minor).** Mid-chunk resume (seek to `start + resume_offset`, Range `bytes=(start+offset)-stop`, counter initialized at offset) is mechanically correct against the current worker (src/download/chunked.rs:461–478). The edge cases the plan lists are handled: all-chunks-complete journal (skip → rename, which also self-heals a prior rename failure), never-shrink `set_len` with foreign-file → mismatch, 0-length chunks impossible (`div_ceil`, probe rejects size 0). ETag-absent fallback is right for real HF. Missed: no 206/`Content-Range` verification on the resume response (finding 3), incomplete speed-pacing seeding (finding 5), and the headline byte-granular path has no planned coverage (finding 7).

**4. prepare/retry behavior changes — OK.** Keeping `.incomplete` + journal on transient retry, terminal failure, and 401 is exactly the issue's ask; 401-keep is correct (re-auth then resume; a probe-stage 401 creates no files anyway). The flipped `tests/download_failures.rs` contracts stay coherent: `timeout_then_terminal_500_attempts_exactly_max_retries_plus_one` and `timeout_then_success_retries_and_exits_ok` both fail/recover at the **probe** stage (no files ever created), so their assertions (exact attempt count, no leftover `.incomplete`) pass unchanged; the two "nothing survives" assertions flip as planned. Missed doc surface: the SIGINT message (finding 4).

**5. Out-of-scope list — OK.** Nothing critical missing. Cross-process deferral is consistent with the registry's existing story (docs/DEFERRED.md); no `If-Range` is fine since probe+journal validation is equivalent and SHA256 backstops. Verified claims: registry untouched (`mark_failed` semantics intentionally coarse — src/registry.rs:484–491); hf-cache sync claim true (staging `.incomplete` survives, src/cache_layout.rs:775–781; sync downloads route through the same `download_chunked`); TUI delete flow correctly targeted (src/ui/app/downloads.rs:310–350).

**6. Testability — CONCERN (major).** Scenarios 1, 4, 5 will pass and are deterministic (scenario 1 by its own `concurrent_threads = 2` design, tests/resume.rs:154; 4/5 only assert journal existence + restart-from-zero, both race-immune). Scenarios 2 and 3 contain a genuine scheduler race that can fail them — see finding 1. This is the repo's own incident-#37 class of bug (a nondeterministic e2e), and the plan's claim "all 5 scenarios turn green with this design" is only probabilistically true as specified.

**7. Simplicity — OK, not over-engineered.** Byte-granular offsets are justified: with `min_chunk_size` = 5 MB / `max_chunk_size` = 100 MB defaults, chunk-granular (boolean) resume would forfeit up to 100 MB per in-flight chunk — precisely the issue's pain. Per-chunk part files are correctly rejected (full-file copy + double disk on a 40 GB model). The registry genuinely cannot represent holes (single `downloaded_size: u64`), so the sidecar is the minimal correct home.

## Findings

1. **[MAJOR] Determinism race in scenarios 2 & 3; journal must be persisted eagerly.** `TestEnv` defaults `concurrent_threads = 4` (tests/common/mod.rs, config block), and scenarios 2–5 don't override it. With 4 workers, the scripted 500 (`statuses[3]`, the 4th arrival) lands **in the same in-flight batch** as three 200-response chunk requests. Whether any 200-chunk finishes its data path (open → stream 4096 B → flush → `record`) before the 500-error's much shorter path (error → 2 progress-lock awaits → `wait_for_chunks` → `abort_all`, src/download/chunked.rs:336–356) is scheduler-dependent — the 500 task is favored (no tokio-fs dispatches, no body). If it wins, the journal has **zero** completed chunks and `tests/resume.rs:260` / `tests/resume.rs:311` fail. Scenario 1 is deterministic precisely because it pins 2 workers (batch 2 starts only after batch 1 chunks complete-and-record). Fix: `env.set("concurrent_threads", "2")` in scenarios 2–5, and state in the plan (§3.1/§3.2) that the journal file is written immediately at download start with all-zero offsets — otherwise scenarios 2/4/5's `journal_path(&env).exists()` assertions (tests/resume.rs:366, 416) inherit the same zero-progress race.

2. **[MINOR] Add `chunk_offsets.len() == num_chunks` to validation** (plan §2). Per-element bounds alone let a truncated/corrupt array with a matching `chunk_size` shift chunk ids onto wrong byte ranges — silent corruption on resume, the exact failure mode the validation exists to prevent.

3. **[MINOR] Verify 206/`Content-Range` on the resume request.** The worker only does `error_for_status()` (src/download/chunked.rs:461–468). A server that ignores Range and answers 200 to `bytes=(start+offset)-stop` makes the worker write the entire body at `start+offset` — silent corruption. Pre-existing exposure for full chunks, but resume amplifies it; check status == 206 (or `Content-Range` start) at least on the resume path. Relatedly, the plan deletes the `tests/download_failures.rs` GAP note ("416 on resume not constructible") but doesn't add the now-constructible wrong-offset-resume scenario it promised; fold a 416-on-resume (or 200-ignoring-Range) case into the matrix.

4. **[MINOR] Stale user-facing message:** `src/cli/download_cmd.rs:155` — "unfinished files stay registered as incomplete and restart from scratch on the next run" becomes false after this fix. Add it to §3.5's doc/message updates.

5. **[MINOR] Seeding and record-placement underspecified.** Seed `last_downloaded_bytes` (and the pacing baseline) along with `progress_downloaded`, or the first speed interval counts all resumed bytes as fresh throughput. Also pin that `journal.record()` is called **outside** the `progress.lock().await` scope — blocking IO under the level-3 tokio mutex won't deadlock (UI uses try_lock) but stalls renders; one sentence in §3.2 prevents it.

6. **[MINOR] `record()`/`save()` error policy unspecified.** A journal write failure (disk full, transient IO) must not fail an otherwise healthy download — make it best-effort/silent-swallow (the `registry.rs` precedent) and say so in §3.1.

7. **[MINOR] Byte-granular resume has no planned coverage.** 4 KiB chunks on loopback never exceed the 1 s coalescing window, so partial offsets never persist in the e2e suite; the `seek` + Range-construction path (the issue's core mechanism) is only covered indirectly. Add a worker-level unit test (or one e2e with a large chunk + mid-chunk stall) exercising a non-zero, non-complete `resume_offset`.

8. **[NIT] Journal `revision` field is redundant** — `resolve_url` embeds the revision (src/api/mod.rs:58), so url equality already covers it. Keep only if it aids diagnostics.

9. **[NIT] Zombie-record window (accepted risk, document it):** a chunk task aborted *while inside* `record()` completes its save after `abort_all()` (sync, uninterruptible), which can overwrite a fresher attempt-2 record and permanently under-report one chunk — idempotent re-download next run, never over-report. One footnote in §3.1.

10. **[NIT] TUI display inconsistency:** the resume popup / incomplete list render registry `downloaded_size` (reset to 0 by `mark_failed`, src/registry.rs:484–491) while the journal holds real progress — users see "0 bytes" then a fast resume. Cosmetic; worth a sentence.

11. **[NIT] Std-mutex poisoning:** specify `lock().unwrap_or_else(|e| e.into_inner())` (the registry pattern, src/registry.rs:82) for `JournalWriter` so a panicking chunk task can't poison the writer for its siblings.

## Verified correct (no action)

- Root-cause table matches the source exactly: `prepare_download_paths` delete (src/download/mod.rs:252–261), transient-retry delete (:455–458), 401/terminal deletes (:467–470, 484–487), `truncate(true)` + `set_len` + byte-0 chunk starts (src/download/chunked.rs:113–121, 461–478).
- Lock-hierarchy/docs-guard non-interference claim verified (guard scope is engine bundles only).
- hf-cache sync "inherits the fix" claim verified (src/cache_layout.rs:775–781; staging downloads go through the same transport).
- The two probe-stage failure tests in `tests/download_failures.rs` stay green unchanged under the new keep-files contracts.
- Scenario 1's wire-level assertion design (completed = first two arrivals of run 1; stalls are exact-nth) is deterministic by construction.

## Summary

Approve the architecture — journal sidecar, validation set, under-report crash direction, keep-on-failure contracts, registry non-involvement — all verified against the code. Before implementation: fix the scenario 2/3 determinism race (two-worker config + eager initial journal persist), add the offsets-length validation rule, harden the resume response check, and sweep the small spec gaps (SIGINT message, seeding, error policy, mid-chunk coverage). With those incorporated the plan is ready to build.