Subagent timed out after 900000ms.

Recovery summary:
- termination: timed-out
- changed tracked files: none
- session file: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/2026-10-13T05-46-02-323Z_01a133cb-e2d3-7406-bd27-ef7eaa27f17a/9b0cce06-4449-4dc0-adc3-574f23456dfb/run-0/session.jsonl
- transcript: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/subagent-artifacts/9b0cce06-4449-4dc0-adc3-574f23456dfb_reviewer_0_transcript.jsonl
- output artifact: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/subagent-artifacts/9b0cce06-4449-4dc0-adc3-574f23456dfb_reviewer_0_output.md
- metadata artifact: /home/johannes/.pi/agent/sessions/--home-johannes-projects-rust-hf-downloader--/subagent-artifacts/9b0cce06-4449-4dc0-adc3-574f23456dfb_reviewer_0_meta.json
Warning: Inspect partial changes before retrying or resuming the child.

Partial output before timeout:
All material reviewed. Here is the verdict document.

---

# Review: `plans/fix-issue-50-resume-downloads.md` — resumable downloads (issue #50)

**Overall verdict: APPROVE-WITH-CHANGES**

The plan's root-cause analysis is accurate, the sidecar-journal design is the right shape for this codebase, and the registry-untouched / lock-hierarchy-untouched claims verify against the source and the doc guards. However, the plan's §4 claim that *all five already-committed `tests/resume.rs` scenarios turn green with this design* does not hold: scenarios 2 and 3 embed a timing assumption the design cannot satisfy (Finding 1, blocker), and the journal's initial creation — load-bearing for two other pinned assertions — is never specified (Finding 2). Both are cheap to fix in the plan; neither invalidates the architecture.

## Review checklist

**1. Journal-based resume design — CONCERN (minor gaps, core sound).**
The validation set (url/revision/total_size/chunk_size/etag, per-offset bound, discard-don't-trust) and the data-before-journal under-report direction are correct, and the atomic save (same-dir temp + `sync_all` + rename, reusing the `utils::atomic_rename_with_retry` policy) matches the registry precedent (`src/registry.rs` module docs). Gaps: §3.1's `validate(&self, url, total_size, chunk_size, etag)` signature drops the `revision` argument that §2 requires (harmless in practice — `resolve_url` embeds the revision in the URL — but the spec contradicts itself, and the `chunk_offsets` vector length is never checked against `num_chunks`; a truncated vector passes the per-element bound check). The etag-absent fallback (size+url, SHA256 as backstop) is reasonable but the backstop only exists when an LFS hash is advertised (see Finding 4).

**2. Concurrency (JournalWriter, std mutex, tokio lock hierarchy) — OK.**
Verified: `JournalWriter` is download-internal state, not an `EngineState` bundle field, so `tests/docs_guards.rs::lock_hierarchy_documents_every_engine_mutex_field` (which derives its field set only from `EngineState`/`QueueAccounting`/`EventBus`/`VerificationHub`) is unaffected — the plan's claim holds. A plain `std::sync::Mutex` whose critical section is sync-only leaf IO is exactly the `REGISTRY_WRITE` terminal-leaf precedent (`AGENTS.md` "Also registry-internal…" paragraph); with no `.await` and no other lock acquired under it, no cycle with the tokio hierarchy is possible. Two hygiene requirements should be pinned in the plan (Finding 7): poison-tolerant locking and the record call site must stay outside the `progress` tokio-mutex scope. `module_dependency_dag` stays green as long as `journal.rs` doesn't reference `crate::engine` (it has no reason to).

**3. chunked.rs changes — OK with edge notes.**
Mid-chunk resume math is correct: seek to `start + resume_offset`, `Range: bytes=(start+resume_offset)-stop` matches the existing inclusive-`stop` convention (`src/download/chunked.rs:468`). The all-chunks-complete journal (skip everything, rename) and the foreign-longer-file-as-mismatch rules are both handled. 0-length chunks are unreachable, correctly: `probe_file_size` rejects `total_size == 0` before any file exists, and `div_ceil` chunking never yields an empty chunk — the plan's explicit handling of the "everything complete" journal covers the only real edge. `set_len` on resume is right (extend-only; shrink = mismatch). Missed edges: the open flags for resume (`create(true).write(true)`, no truncate — a valid journal whose `.incomplete` was manually deleted must not fail to open), 206-status assertion on resumed requests, and the speed-seeding pairing (Finding 6).

**4. prepare_download_paths / execute_download_with_retry changes — OK.**
The three deletion sites are exactly as the plan's root-cause table states (`src/download/mod.rs:252` prepare, `:455` transient retry, `:467` 401, `:484` terminal). The keep-on-transient-and-terminal flip is the issue's whole point, and the flipped contracts in `tests/download_failures.rs` remain coherent — *provided* Finding 2 is addressed (the flipped assertions at `tests/download_failures.rs:109`/`:183` require a journal to exist even when zero chunks completed, which is the normal outcome of `server_500_mid_download_fails_and_marks_registry`). The two unaffected tests verify out: `timeout_then_success_retries_and_exits_ok` fails at the probe, before Phase 2a file creation, so nothing survives attempt 1 anyway and success still cleans both files; the attempt-count test's probe-per-attempt counting is untouched. Keeping `.incomplete` on 401 is correct and mostly moot — 401 fires at the probe, pre-creation — but a mid-run token expiry now correctly leaves resumable state for a re-authed run.

**5. Out-of-scope list — OK, nothing critical missing.**
Registry-untouched verified: `mark_failed`/`upsert_metadata` semantics unchanged, no schema change, goldens safe. The hf-cache sync inheritance claim verifies against `src/cache_layout.rs:774-781` (`cleanup_staging` preserves `.incomplete` of failed runs; a journal sidecar next to it survives identically and is consumed by `journal::remove` at the in-staging rename). No `If-Range` is fine given probe+header revalidation. One risk the list doesn't acknowledge: real-Hub ETag stability across runs (Finding 5).

**6. Testability of the 5 scenarios — BLOCKER on scenarios 2/3; 1/4/5 sound.**
Scenario 1 is properly engineered: `concurrent_threads = 2` forces chunks 0/1 to complete, journal-persist (completion forces an immediate flush), and release their semaphore permits *before* the stalled requests #4/#5 even exist — deterministic by construction. Scenarios 4/5 need only "journal exists" + probe-mismatch discard, which the design gives them regardless of chunk outcomes. Scenarios 2/3 are the problem — Finding 1.

**7. Simpler alternative / over-engineering — OK, right-sized.**
Per-chunk booleans would pass these tests but can discard up to `max_chunk_size` (100 MB default) of in-flight progress per partial chunk — hours on the slow connections this issue is about; byte-granular offsets plus 1 s coalescing are justified. Part-files are rightly rejected (full-file copy + double disk). The journal-vs-registry split is correct: `downloaded_size: u64` cannot represent holes, and co-locating journal with data file keeps validation atomic.

## Findings

1. **[BLOCKER] Scenarios 2/3 require journal persistence to win a race the design loses: `wait_for_chunks`' `abort_all` vs. the sibling chunks' completion record.**
   `tests/resume.rs:208-265` (scenario 2) and `:268-345` (scenario 3) script `[200,200,200,500,200]` with TestEnv's default `concurrent_threads = 4`: the probe plus three 200 chunk requests and one 500 arrive ~simultaneously. For the tests' `!completed.is_empty()` assertions (`resume.rs:258-260`, `:309-311`) at least one 200-chunk must fully stream, `write_all`, `flush`, and journal-record before `wait_for_chunks` (`src/download/chunked.rs:403/407`) observes the 500 and aborts every sibling. It cannot, reliably: the error path is `error_for_status` → two uncontended progress-lock hops → JoinSet completion — no filesystem involvement; the chunk path includes a `tokio::fs` write (blocking-pool roundtrip), a flush, and a synchronous temp+`sync_all`+rename persist. The 500 wins by an order of magnitude, the 200-chunks are dropped mid-`write_all`, and — because the plan's records fire only "on each progress tick" (TestEnv `progress_update_interval_ms = 50`; µs-fast mock chunks never tick) or at completion — *neither the on-disk journal nor the in-memory offsets advance*. Note an error-path snapshot-persist cannot rescue this: in-memory offsets also move only inside `record()`, which never ran. This also collides with the repo's own determinism standard (`chunked.rs::abort_tests` exists precisely to avoid timing flakes; incident #37). Fix in the plan: retime the two scenarios the way scenario 1 already does — `env.set("concurrent_threads", "2")` with the failure on the 5th arrival (`statuses = [200,200,200,200,500]`), so chunks 0/1 deterministically complete and persist before the failing request exists — and amend §4's "all 5 scenarios turn green" claim accordingly (the tests are already committed, so this is a small, justified test change, not a design change).

2. **[MAJOR] The journal's initial creation/persist is never specified, and two pinned assertions depend on it.**
   §3.1 lists `load`/`save`/`remove` and §3.2 says the journal is "loaded and validated" — but nothing says a fresh download persists the journal (url/revision/total_size/chunk_size/etag, `chunk_offsets = vec![0; num_chunks]`) *before* chunks spawn. `tests/resume.rs:96-110` (`read_journal` panics if the file is missing) plus scenario 2's run — which, per Finding 1, normally ends with zero completed chunks — and the flipped `tests/download_failures.rs:109`/`:183` assertions all require the journal to exist with a full-length zero vector even when no `record()` ever fires. The plan must state the initial persist explicitly (it also gives `validate` the length anchor for Finding 3's check).

3. **[MINOR] Spec inconsistencies in `validate`.** §3.1's signature omits the `revision` argument §2 mandates (drop the rule or the argument — the URL embeds the revision either way), and the per-element `chunk_offsets[i] ≤ chunk_len(i)` check must be paired with `chunk_offsets.len() == num_chunks`, or a truncated/corrupt-short vector validates and panics later at `chunk_offsets[i]` indexing.

4. **[MINOR] The accepted power-loss window's backstop doesn't cover non-LFS files.** The plan accepts journaled-offsets-outrun-flushed-data on power loss "caught by SHA256 verification" — but verification only runs when a hash is advertised (`expected_sha256`); small/non-LFS files have none, so a power-loss resume can silently produce a corrupted final file where the old delete-and-restart behavior could not. Cheap hardening: `file.sync_all()` immediately before the *completion* record (≈25 syncs per file at default chunking — negligible) so completed chunks are power-loss safe; mid-chunk offsets keep the accepted window.

5. **[MINOR] Real-Hub ETag stability is an unacknowledged resume-efficacy risk (inference).** The plan validates etag "when both journal and probe have one." On the real Hub, `/resolve/` redirects to a CDN and reqwest observes the *redirect target's* ETag; if that varies across runs/CDN nodes (weak vs. strong, node-specific), every re-run would spuriously discard the journal and silently restart from 0 — defeating the fix for exactly the issue's audience. Cannot be verified from this repo; recommend recording the transport's already-available `expected_sha256` as an additional (preferably primary, when present) stable validator, and only comparing etags when both sides have one *and* are strong-form.

6. **[MINOR] Seed the pacing marker along with the counter.** Seeding only `progress_downloaded` (`src/download/chunked.rs:285`) while `last_downloaded_bytes` starts at 0 (`:288`) makes the first speed sample count all resumed bytes (`bytes_since_last_global = resumed_sum`), spiking displayed speed for one interval. Seed both (or subtract the seed when initializing the marker).

7. **[MINOR] Pin the JournalWriter's lock hygiene.** Two things the plan should state: (a) poison-tolerant locking (`lock().unwrap_or_else(|e| e.into_inner())`, the `REGISTRY_WRITE` pattern at `src/registry.rs:88`) so one panicked record doesn't cascade-panic every chunk; (b) `record()` must be called *outside* any `progress.lock().await` scope — keeping the std mutex a terminal leaf, and avoiding a blocking atomic-save while holding a tokio mutex other chunks need.

8. **[NIT] `ChunkContext.journal: Option<Arc<JournalWriter>>`** — it is unconditionally `Some` in the chunked flow; the `Option` buys nothing. Drop it.

9. **[NIT] Assert `206 Partial Content` (ideally with the expected `Content-Range`) on resumed chunk requests.** A range-ignoring middlebox answering `200` + full body would today corrupt the file by writing past the chunk end; resume requests are the natural place to harden this (not a regression — current code is equally exposed).

10. **[NIT] Resume-open flags and status churn.** Specify `create(true).write(true)` without `truncate` for the resume open (valid journal + missing file must not fail to open), and note the benign UX where `prepare_download_paths` announces "Resuming incomplete download" just before `download_chunked` discards a mismatched journal and restarts.

## Summary

The plan correctly identifies all three deletion/restart sites, picks the right persistence model (sidecar journal co-located with `.incomplete`, registry untouched — both claims verified against `src/registry.rs`, `tests/docs_guards.rs`, and `src/cache_layout.rs`), and sizes the solution honestly against simpler alternatives. The two must-fix items before implementation: (1) the §4 test-matrix claim is false for scenarios 2/3 — the sibling-abort vs. journal-persist race must be resolved by retiming those two tests (scenario 1's `concurrent_threads = 2` pattern) since no implementation of the specified design can make them reliably green; (2) the initial journal persist (zero-vector, full length) must be specified — scenario 2 and both flipped `download_failures` assertions depend on it. The remaining findings are hardening and spec-tightening that can ride along.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Full checklist verdict (7 items, each OK/CONCERN/BLOCKER with justification), 10 severity-tagged findings with file:line references (src/download/mod.rs:252/455/467/484, src/download/chunk