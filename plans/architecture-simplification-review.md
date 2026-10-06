# Architecture Simplification & Refactor Plan — post-v2.13.2

**Status:** proposed · **Branch:** `refactor/architecture-simplification-review` · **Date:** 2026-10-06
**Provenance:** four-lane parallel fresh-context review (independent models, distinct seams), findings
spot-verified by the orchestrator against HEAD `a756371`. Review evidence: §10.

**Goal.** Reduce the cost of change for both humans and AI agents: make the architecture's real
rules *structural* (type-enforced) or *guarded* (test-enforced) instead of convention-only, close
the correctness holes that complexity is currently hiding, and make the navigation docs trustworthy
at every commit. This plan succeeds the W1–W5 readability wave (PRs #43–47) — it does not re-open
completed splits.

**Reading conventions.** Sections are numbered headings so `§N` anchors resolve unambiguously
(the failure mode documented in §3.4). All file:line coordinates are **as of `a756371`** and are
re-stated by symbol where possible — the same regime `docs/DEFERRED.md` imposes (§5 M0).
"Sign-off" items change observable behavior and are collected **before M1 starts** (§5 Gate 0),
not mid-plan.

---

## 1. Assessment summary

The W-wave landed well: facades hold (no private-submodule type escapes), `EngineState::enqueue`
is genuinely the single transaction home for all six download flows, the CLI runner layer is
shared and clean, `fmt.rs`/`patterns.rs`/`cache_layout.rs` have deliberate, tested contracts.
Three structural problems remain, all confirmed by multiple lanes independently:

1. **Registry write discipline is convention-only and already violated in three places.**
   The documented invariant — "every registry write routes through the typed ops; disk is the
   source of truth; never write from the in-memory mirror" — is bypassed by `engine/enqueue.rs`
   (`AlreadyRecorded` arm saves the *mirror* to disk), the TUI delete flow
   (`ui/app/downloads.rs` `delete_incomplete_downloads` clones the mirror → mutates → saves), and
   `cli/hf_cache/sync.rs` (`purge_staging_registry_entries` inline load-modify-save). On top of
   that, the manager and the verification worker genuinely write concurrently in-process
   (verification overlaps the next file's download), and `save_registry` is non-atomic — a crash
   mid-save truncates the registry. The race is currently *pinned as desired* by
   `registry_tests.rs` ("pinned, not fixed").

2. **The engine core is a cyclic triangle glued by a 17-field god bundle.**
   `engine ↔ download ↔ verification` import each other (`download/mod.rs` calls
   `engine::auth_status_message`; `verification.rs` takes `EngineState`; `engine/workers.rs`
   spawns both). `EngineState` exposes 11 `Arc<Mutex<..>>` + 3 senders + 3 atomics, all `pub`,
   with a 10-level lock hierarchy that (a) is enforced nowhere, (b) omits `verify_rx`/`outcome_rx`
   (drained with the same receiver pattern at `cli/run.rs`), and (c) is numerically contradicted
   by the runner's own documented descending `try_lock` drain order. No deadlock exists today —
   every site read was clean — but the contract rots silently as fields are added.

3. **The documents agents trust most are wrong at HEAD.**
   `TESTING.md` prescribes `cargo test --lib`, `--test integration`, doc-tests, benchmarks, and
   per-PR CI — none of which exist on this bin-only, tag-only-release crate. The `§8.x` anchor
   regime resolves to an unindexed plan with no `§8.x` headings, pre-W-wave file:line
   coordinates, and at least one resolved-but-still-cited item (§8.9). README's structure block
   is corrupted (wrong rows pasted); TROUBLESHOOTING references removed options fields;
   CONTRIBUTING describes the old release flow. With no CI, these docs *are* the guardrails —
   when they lie, every agent (human or AI) starts wrong.

Plus a handful of real correctness bugs the complexity was hiding (§3.5): manual verification
always verifies shard 0 of a multi-part quant; `[..16]` hash slices can panic on corrupt registry
entries; recursive tree fetch silently swallows per-directory errors (truncated trees); failed
chunk tasks are detached and never aborted while retry recreates the file beneath them.

---

## 2. Target architecture

```
src/
├── main.rs            # thin bin: dispatch to lib-level entry (see §9 — lib.rs deferred)
├── fmt.rs utils.rs patterns.rs rate_limiter.rs http_client.rs update.rs   # leaves, unchanged
├── paths/             # app-path resolution + sanitize (hub-cache parts moved out, §7 M6)
├── models/            # +auth-status contract (data only); naming note for models/{api,engine,ui}
├── api/               # unchanged facade; shared reqwest::Client threaded from bootstrap
├── registry.rs        # single-writer: with_registry() + atomic save; full typed-op set
├── config.rs          # declarative option→engine-atomics table (single edit site)
├── cache_layout/      # owns ALL hub-cache knowledge: layout math, hf_hub_cache, CACHEDIR.TAG
├── verification.rs    # owns VerificationHub + worker; imports models+registry only (no engine)
├── download/          # imports models+registry+verification-queue only (no engine)
├── engine/            # sole composite owner: constructs hubs, spawns workers
│   └── EngineState regrouped:
│       queue: QueueAccounting { state, items }        # "acquire separately" becomes one method
│       events: EventBus { status, verify, outcome }    # receiver rule stated once
│       verification: VerificationHub { …8 fields }
│       progress, complete, registry mirror
├── cli/               # unchanged; tests split per subject (§7 M6)
└── ui/                # app owns options-dialog state; render is a pure consumer
```

Key property changes: the engine triangle becomes a DAG (download and verification depend only on
models + registry), **enforced by an import-direction guard test**; three of the ten documented
lock levels become struct invariants (fields private to their bundle, accessor methods own the
ordering rule); every registry write goes through one serialized **in-process** writer with an
atomic save, with raw `load_registry`/`save_registry` demoted to module-private so a fourth
bypass site cannot compile. Cross-process lost updates (concurrent CLI + TUI) remain out of
scope — recorded in `docs/DEFERRED.md` with the optional advisory file lock as its remedy.

CI reality (verified at HEAD): no PR/push CI exists; the only CI is the tag-triggered release
workflow's `cargo test --locked` on the three native runners (Linux, macOS, Windows). Local
gates (§6 G2) are therefore the primary guard, and anything Windows-sensitive (M1's atomic
save) must hold under the release matrix too.

---

## 3. Consolidated findings register

Severity: P0 = actively hurts correctness/maintainability · P1 = high-value · P2 = polish.
Lane: S=structure, U=ui, P=pipeline, R=readability. Items marked **sign-off** change observable
behavior.

### 3.1 Registry integrity (P0)
| ID | Finding | Evidence | Lanes |
|----|---------|----------|-------|
| R1 | Mirror→disk save in TUI delete flow violates "disk is source of truth" | `ui/app/downloads.rs:344-370` | U, P |
| R2 | Mirror→disk save in enqueue `AlreadyRecorded` arm | `engine/enqueue.rs:111` | P |
| R3 | Inline load-modify-save `purge_staging_registry_entries` | `cli/hf_cache/sync.rs:601-650` | S, P |
| R4 | Live in-process lost-update race: manager (`upsert_metadata`, `mark_complete/failed`) vs verification worker (`mark_mismatch`) — serialized downloads but verification overlaps next file | `download/chunked.rs:66-82`, `download/mod.rs:279-287,431,462` vs `verification.rs:196-198`; pinned-as-desired at `registry_tests.rs:769-786` | P |
| R5 | `save_registry` non-atomic — crash truncates registry | `registry.rs:34-48` | P |

### 3.2 Engine coupling & shared state (P0/P1)
| ID | Finding | Evidence | Lanes |
|----|---------|----------|-------|
| E1 | Cyclic triangle engine↔download↔verification | `engine/mod.rs:58`, `workers.rs:5,91` vs `verification.rs:6`, `download/mod.rs:475` | S |
| E2 | `EngineState` 17 pub fields; per-consumer usage provably narrow (manager 9, verify-worker 8, CLI 5, render 6+2) | `engine/mod.rs:94-118` | S |
| E3 | Lock hierarchy omits `verify_rx`/`outcome_rx`/`verification_in_flight`; contradicted by runner's descending try_lock drain; convention-only | `AGENTS.md:192-216` vs `cli/run.rs:10-16,354,376` | S, R |
| E4 | `InvalidPolicy` axis fully derivable from `RegistryMode` (read once, 1:1 constructor pairing); `SendDiscipline` *is* earning complexity | `engine/enqueue.rs:46-49,82,334-403` | P |

### 3.3 Docs & navigation truth (P0 for agents)
| ID | Finding | Evidence | Lanes |
|----|---------|----------|-------|
| D1 | `TESTING.md` prescribes non-existent targets/CI (`--lib`, `--test integration`, doc-tests, benches, per-PR checks) | `TESTING.md:38,45,135,170,176,196` | R |
| D2 | `§8.x` anchor regime: unindexed plan, no sub-headings, stale coordinates, resolved items still cited | `plans/readability-maintainability-refactor.md:168-183` + ~10 src sites | S, R |
| D3 | README structure block corrupted (UI rows under cli/, duplicate mod.rs, `ui/app/models/` phantom) | `README.md:716,736-738` | R |
| D4 | CONTRIBUTING/TROUBLESHOOTING stale: `cargo build --dev`, old release flow, removed options fields, wrong defaults, libssl advice on rustls-only stack | `CONTRIBUTING.md:22,127-130,158-160`; `TROUBLESHOOTING.md:90-99,336-340` | R |
| D5 | `ENV_MUTEX` test convention (~45 sites, 12 modules, documented poisoning cascade) absent from all agent docs | `paths.rs:728-732` | R |
| D6 | `fmt.rs` deliberate "do-not-unify" contract absent from every module map | `fmt.rs:1-32` | R |

### 3.4 Correctness bugs (P0/P1; the plan folds fixes into milestones)
| ID | Finding | Evidence | Lanes |
|----|---------|----------|-------|
| B1 | Manual verify hardcodes `group.files[0]` — selected shard ignored; `[0]` panics on empty group | `ui/app/verification.rs:15-18` | U |
| B2 | `&item.expected_sha256[..16]` panics on short/corrupt registry hash | `verification.rs:162` | P |
| B3 | `fetch_recursive_tree` swallows subdir errors → silently truncated trees **(sign-off)** | `api/client.rs:225-231` | P |
| B4 | Failed chunk tasks detached, never aborted; retry recreates file beneath zombies **(sign-off)** | `download/chunked.rs:395-419` vs `download/mod.rs:404-417` | P |
| B5 | Per-request `reqwest::Client` (TLS handshake per directory) + silent auth-header drop to unauthenticated request **(sign-off)** | `http_client.rs:34-36,55-66` | P |
| B6 | Dead `fetch_multipart_sha256s` network call per multi-part confirm; warning self-overwrites **(sign-off)** | `ui/app/downloads.rs:221-238,249,287` | U, P |

### 3.5 UI layering & polish (P1/P2)
| ID | Finding | Evidence | Lanes |
|----|---------|----------|-------|
| U1 | Options-dialog inversion: app logic depends on `ui/render/options_popup.rs` for `OptionsFieldId`/`OPTIONS_FIELDS`/`OptionsDialogState`; text inputs sit loose on App | `options_popup.rs:20-93`, `events/mod.rs:292-388` | U |
| U2 | `snapshot()` double-clones engine state every frame just to pass borrows | `state.rs:222-229`, `app/mod.rs:94-124` | U |
| U3 | Tree navigation clones whole tree per keypress for a count | `tree.rs:32-50`, `events/mod.rs:392-429` | U |
| U4 | Vestigial single-variant `InputMode`; `App::next/previous` duplicate `advance` | `models/ui.rs:38-40`, `events/mod.rs:49-88,435-467` | U |
| U5 | Snapshot helpers + fixture setup triplicated across three render test files | `render/mod.rs:402-409`, `style_size_tests.rs:108` | U |
| U6 | `QueueState`/`download_queue` name a counter pair as if the queue; `CompleteDownloads` mis-homed in "API cache containers", keyed by bare filename (cross-model overwrite) | `models/engine.rs:106-121`, `models/cache.rs:14`, `registry.rs:221-222` | R |

### 3.6 Pipeline & CLI polish (P1/P2)
| ID | Finding | Evidence | Lanes |
|----|---------|----------|-------|
| C1 | `valid_model_id` error block triplicated | `download_cmd.rs:38-47`, `hf_cache/sync.rs:204-213`, `hf_cache/path.rs:17-23` | P |
| C2 | ~90-line inline publish gate in `run_hf_cache_sync` — extract pure fn | `hf_cache/sync.rs:468-560` | P |
| C3 | `Reporter::status_line` prefix-filters hard-code engine message strings — rewording an engine message silently changes CLI output | `cli/report.rs:286-316` | P |
| C4 | `config.rs` repeats the 14 engine atomics across 3 sites — every option is a 4-site edit; declarative table (OPTIONS_FIELDS precedent) | `config.rs:67-196` | P |
| C5 | `update_cmd` hand-rolled progress line; dead `update::check`/`CheckOutcome` kept alive | `update_cmd.rs:137-156`, `update.rs:193-216` | P, R |
| C6 | Verification worker busy-polls at 100 ms | `verification.rs:73-76` | P |
| C7 | Hub-cache dir resolution + CACHEDIR.TAG live in `paths.rs`, sole callers in cache_layout consumers | `paths.rs:195-326` | S |

### 3.7 Test organization (P1)
| ID | Finding | Evidence | Lanes |
|----|---------|----------|-------|
| T1 | `cli/tests.rs`: 1980 lines / 79 tests / 8 subjects, no module doc, exempt from the size ceiling | `cli/tests.rs` | R |
| T2 | No test inventory anywhere; TESTING.md unit index names deleted files | `TESTING.md:31-33` | R |

---

## 4. Adjudicated decisions

Two lanes disagreed on one structural question; the orchestrator adjudicates:

**D-lib: no `lib.rs`/bin split this cycle.** The structure lane proposed it (S effort,
integration-testability of internals); the readability lane rejected it with codebase-specific
evidence: the inline suites deliberately pin *non-exported* surface (`pub(in crate::cli)
tree_file_dtos` widened specifically so `cli/tests.rs` can pin it; private `advance()` pinned by
table tests), so a lib split forces re-deciding every visibility boundary to buy a `cargo test
--lib` alias that `cargo test <filter>` already provides on the bin target. **Decision:** reject
for this cycle; the discoverability problem is inventory (§7 M6), not layout. Revisit only if the
engine decoupling (M3) later creates a genuinely narrow public surface worth exposing.

**D-policy: collapse `InvalidPolicy`, keep `SendDiscipline`.** The derivable axis goes; the
discipline axis provably encodes three different orderings. Merged no-write constructors keep
their public names as delegates (zero call-site churn).

**D-signoff summary** (owner approves inside the milestone): B3 visible errors instead of silent
truncation; B4 abort semantics; B5 invalid-token becomes an error; B6 removes an observable
status warning + one HTTP round trip; R4's fix flips two pinned-as-desired tests from
last-writer-wins to all-writers-win.

---

## 5. Milestone plan

Dependency order (strictly serial PRs per G4; the earlier `M1 ∥ M2` note is withdrawn — the
`confirm_download` decomposition in M2 calls into the enqueue path M1 edits):

**Gate 0 → M0 → M1 → M2 → M3 → M4 → M5 → M6**

- M2 may be *developed* concurrently with M1 (disjoint files: `ui/app/*`, `models/ui.rs`,
  `utils.rs` vs `registry.rs` + its three call sites) but merges after it.
- **M4 must never be concurrent with M3** (both rewrite `download/mod.rs` imports and the
  engine/bootstrap plumbing; B5 threads the client through the same files M3's import surgery
  touches) — sequenced strictly after.
- M5 and M6 are reorderable relative to each other.

### Gate 0 — Sign-off collection (before M1 starts)
The five behavior changes (B3 visible tree errors, B4 chunk-abort semantics, B5 invalid-token
error, B6 dead-fetch removal, R4 all-writers-win) are decided **now**, at plan start — R4 is a
precondition of M1 (a serialized writer necessarily flips the pinned race outcome; if the owner
declines R4, M1 does not start and §8 records the registry work as declined), and the rest gate
their milestones (B6 → M2, B3/B4/B5 → M4). Outcomes recorded in `docs/DEFERRED.md` either way.

### M0 — Documentation truth floor (S; no behavior change)
The guardrails must be trustworthy before anything moves.

1. Rewrite `TESTING.md` around real targets (`cargo test`, `cargo test --test <name>`, filters);
   delete the fictional bench/doc-test/PR-CI sections; keep the insta-gate section; add the
   `ENV_MUTEX` rule + poison-recovery idiom (D1, D5).
2. Complete the lock hierarchy: add the receiver tier (`status_rx`/`verify_rx`/`outcome_rx`,
   order-free under `try_lock`), add `verification_in_flight`, and scope the ordering rule to
   blocking `.lock().await` acquisitions (E3).
3. Create `docs/DEFERRED.md` — symbol-keyed deferred-defects register with a Status column;
   rewrite ~10 src `§8.x` anchors to point at it; index the executed plan in `plans/README.md`;
   mark §8.9 resolved (D2).
4. Fix README structure block (regenerate from `src/`), CONTRIBUTING (command, release flow,
   OPTIONS_FIELDS-first checklist), TROUBLESHOOTING (real fields/defaults, SHA256SUMS, drop
   libssl) (D3, D4). Add `fmt.rs` rows to the module maps (D6).
5. **Guard tests first:** (a) `lock_hierarchy_documents_every_engine_mutex_field` — derive the
   expected field set **from the source** (`include_str!` the `EngineState` definition, extract
   `Arc<Mutex<…>>`/atomic field names by regex — not a hand-maintained list, so new fields are
   automatically checked) and match qualified tokens **only within the delimited lock-hierarchy
   section** of AGENTS.md (no whole-file word match that `state`/`items` would false-pass); must
   fail before step 2, pass after; M3 rewires it to derive from bundle definitions (§5 M3.5).
   (b) `no_bare_plan_section_anchors_in_src` — regex `§\d` must co-occur with `plans/` or
   `docs/DEFERRED.md` on the line; lists today's ~24 violations, green at the end; plus
   **bidirectional DEFERRED validation**: every `docs/DEFERRED.md#<key>` cited in `src/` must
   exist in the register, and every register entry's anchor symbol must still be greppable in
   `src/` (a resolved or orphaned entry fails the test — this catches the "resolved items still
   cited" drift class the bare-anchor rule misses). (c) `testing_md_targets_exist` — every
   `cargo test --test X` (and any `--lib`) mentioned in TESTING.md must resolve to an existing
   target (`tests/X.rs` / `src/lib.rs`), preventing D1 from recurring.

**Accept:** all three guard tests green; `INSTA_UPDATE=no cargo test` byte-stable; zero src changes
beyond comment anchors.

### M1 — Registry single-writer integrity (M) — **blocked by Gate 0 (R4)**
Fixes R1–R5, the only finding all three code lanes converged on. The invariant must be
*structural*, not procedural — the current violations exist precisely because direct saves were
merely discouraged.

1. `registry.rs`: `static REGISTRY_WRITE: std::sync::Mutex<()>` + `pub fn with_registry(f: impl
   FnOnce(&mut DownloadRegistry)) -> DownloadRegistry` — returns the **post-write snapshot** so
callers replace the engine mirror from it after release (one derivation rule, no hand-patching;
the mirror patch therefore never happens under the write lock, preserving leaf-only). Contract:
leaf-only closures (no nested ops — debug assertion); `std` mutex, no `await` inside (ops are
sync, the TOML is small); brief file IO under the lock is accepted. Route all six typed ops
   through it.
2. **Privatize the escape hatch:** `load_registry`/`save_registry` become module-private to
   `registry.rs` (bin crate — module privacy suffices; no `pub` escape). **Guard test:** scan
   `src/**` and assert no registry-path `fs::write`/`File::create` outside `registry.rs` —
   the fourth bypass site now fails CI (release matrix) and local gates instead of shipping.
3. **Atomic save, Windows-aware:** write temp file in the *same directory* (same filesystem),
   `sync_all()` on the temp file, then rename via the existing `utils` rename-with-retry (sync
twin) — Windows `rename` over an existing file can fail when a reader holds it without
   share-delete, which the retry absorbs; add a Windows-matrix-aware test (the release workflow
   runs `cargo test --locked` on Windows — the `fix(tests): Windows CI` commits exist for
   exactly this matrix).
4. Convert the three bypass sites to typed ops: `delete_incomplete_by_urls(&[String])` (loads
   disk — never the mirror — and patches the mirror from the returned snapshot),
   `purge_staging()` with the exact `contains(".rhd-staging")` predicate + TOML golden, and a
   `retain` op for the enqueue `AlreadyRecorded` arm.
5. Flip the two pinned race tests (`two_writers_*`) to assert **both** writes survive; add a
   barrier-scheduled interleaving test proving all-writers-win.
6. Notes: the process-global `REGISTRY_WRITE` also serializes registry writes across parallel
tests using distinct `ENV_DATA_DIR`s — harmless (writes are rare), accepted; path-keying rejected
   as complexity. Cross-process safety stays optional (§8).

### M2 — Quick correctness wins (S; merges after M1, may be developed alongside it)
1. B1: `verify_downloaded_file` resolves `quant_file_list_state.selected()` when focused on
   `QuantizationFiles`, bounds-guarded. Test first: selecting shard 2 queues shard 2's name+hash.
2. B2: `expected_sha256.get(..16)` fallback truncation.
3. Dead code: delete `update::check`/`CheckOutcome`; correct the `sha256_file` doc claims (or
   delete it — only its own tests call it); drop `InputMode`; delegate `App::next/previous` to
   `advance()`.
4. U2: `snapshot_in_place` + `RenderParams` borrowing `&render_cache`. U3:
   `count_visible_nodes` (+ `flatten_tree_refs`) for navigation. Equivalence test first:
   `count_visible_nodes == flatten_tree().len()` across expanded/collapsed permutations.
5. B6 (sign-off): remove the `fetch_multipart_sha256s` call + warning from `confirm_download`;
   decompose `confirm_download` into `confirm_quant_download` / `confirm_scoped_repository_download`.

**Accept:** snapshot/style suites unchanged (visual oracle), downloads characterization tests
green with only the B1/B6-intended diffs.

### M3 — Engine decoupling: DAG + state bundles (M)
Fixes E1, E2; makes the M0 lock docs structural. Order within the milestone matters.

0. **Field→bundle ownership table first:** write the explicit mapping of all 17 `EngineState`
   fields (including `verify_rx`/`outcome_rx`/`verification_in_flight`) to their target bundle —
   `EventBus` owns the *channels* (status/verify/outcome tx+rx), `VerificationHub` owns the
   *queues/counters/progress* — so no field has two plausible homes. Reviewed before any code
   moves.
1. **Behavioral pins named up front** (the shape test below pins structure only): the 8 enqueue
   characterization tests, manager drain/accounting tests, verification worker timing tests,
   the `RenderCache` defaults test, and one full e2e download+verify run through
   `tests/cli_download.rs`. **Shape test:** `EngineState::new()` asserting fresh values of every
   bundle — it changes *within* M3 by design (it is the migration checklist), which is why the
   behavioral pins above are the real gate.
2. Move the auth-status contract (`AUTH_STATUS_PREFIX`/`auth_status_message`/`parse_auth_status`)
   from `engine/mod.rs` to `models/engine.rs`; engine re-exports for one cycle; switch
   `download/mod.rs`, `cli/report.rs`, `ui/app/mod.rs` imports. The re-export shim has a named
   expiry: deleted in this same milestone's final commit.
3. Introduce `VerificationHub` in `verification.rs` (the 8 fields the worker already uses:
   queue, size, in_flight, progress, results, status_tx, verify_tx, registry_mirror);
   `verification_worker(hub)` + `verify_file(item, &hub)`; `queue_verification` becomes a hub
   method; `engine/workers.rs` constructs the hub. `verification.rs` no longer imports `engine`.
4. Regroup `EngineState`: `queue: QueueAccounting{state, items}` (with the never-nested rule as
   one method), `events: EventBus{status, verify, outcome}`, `verification: VerificationHub`,
   plus `progress`, `complete`, `registry`. Bundles keep *separate* `Arc<Mutex>` fields inside —
   no lock merging. ~60–80 mechanical renames (grep-verified clusters: ui/app 20+, engine 15,
   cli/run 5, verification 8). **Fold U6's rename in here** (one touch of the ~25 sites, instead
   of re-touching them in M5): `QueueState`→`QueueTotals`, `download_queue`→`download_queue_totals`.
5. Rewrite the AGENTS.md lock section as per-bundle invariants; **rewire the M0 guard test (a)
   to derive its field set from the bundle definitions** instead of the flat field list —
   scheduled M3 work, not an afterthought.
6. **DAG guard test:** `module_dependency_dag` — walk `src/verification.rs` + `src/download/**`
   and assert no `crate::engine` import appears (the re-export shim's expiry in step 2 makes
   this pass cleanly). This test is what makes §2's "DAG" claim type-adjacent and durable.

**Risks:** accidentally serializing separate locks (don't merge fields); `RenderCache`/`snapshot`
paths must track new field paths — `state.rs` defaults test catches drift.

### M4 — API & client robustness (M; Gate 0 sign-offs B3/B4/B5 required; never concurrent with M3)
1. B3: `fetch_recursive_tree` propagates subdir errors. Test first: `MockRepo` fail-subdir knob →
   e2e asserts surfaced `network` event + exit code.
2. B5: one `reqwest::Client` per run, threaded from the Runner bootstrap (`load_run_config`
   tail); `get_with_optional_token` becomes a thin compat wrapper; invalid header value is an
   explicit error, not a silent unauthenticated downgrade.
3. B4: `spawn_chunk_tasks`/`wait_for_chunks` → `JoinSet` + `abort_all()` on first error.
   **Deterministic pin (not a flake loop):** one chunk fails while another is parked on a
   barrier; assert the parked task is aborted and performs **no writes** after the retry
   recreates the file; the two existing retry-count tests plus the 10× e2e loop remain as
   secondary confirmation. Pinned by `timeout_then_terminal_500_attempts_exactly_max_retries_plus_one`
   and `timeout_then_success_retries_and_exits_ok`.
4. C5: unify `update_cmd` progress with Reporter shapes (H4 goldens have landed, unblocked);
   delete the dead path (done in M2 if not already).

### M5 — UI layering & policy collapse (M)
1. U1: `OptionsFieldId`/`OptionsFieldKind`/`OptionsDialogState`/`OPTIONS_FIELDS` +
   `options_directory_input`/`options_token_input` move to new `ui/app/options.rs`; render
   becomes a pure consumer. Pinned by `options_tests.rs` (16 fields) + pane predicate tests.
2. E4: delete `InvalidPolicy`; merge `AlreadyRecorded`+`StagingSweep` → `NoWrites` (constructors
   keep their names as delegates). This is a **bounded re-open of the W-wave's sealed
   `EnqueuePolicy`** — acknowledged as such, not claimed as untouched. It is compile-time-only
   *iff* the two merged variants truly share event/message shapes: pin that with a
   per-named-constructor equivalence test (same registry state + same event sequence before and
   after) rather than trusting the type change. Fields stay private; constructors remain the
   sole API.
3. U6: ~~rename `QueueState`→`QueueTotals`~~ **folded into M3 step 4** (one touch of the ~25
   sites); `CompleteDownloads` still moves here — next to `DownloadMetadata` in
   `models/engine.rs` with a doc comment on the bare-filename key + collision consequence
   (collision *fix* is a behavior change — deferred to `docs/DEFERRED.md`).
4. U5: consolidate render test helpers into `#[cfg(test)] render/test_utils.rs`; add the
   snapshot-name manifest test so a move that orphans a golden fails loudly.

### M6 — Ownership consolidation & test organization (S–M)
1. C7: `paths::hf_hub_cache` + env consts + `write_cachedir_tag` → `cache_layout` (pub-use shims
   in paths for one cycle; tests move with functions). Optional after: `cache_layout/{mod,plan,
   publish,lock,refs}.rs` tree along the existing section boundaries — only if it keeps growing.
2. T1/T2: split `cli/tests.rs` into per-subject `*_tests.rs` modules + `#[cfg(test)] cli::testutil`
   for shared fixtures (snapshot bodies byte-identical — header churn allowed; manifest test from
   M5 guards orphaning). Rewrite TESTING.md's inventory as a module → what-it-pins → how-to-run
   table.
3. C4: declarative option→atomics table in `config.rs` (single edit site per new option).
4. C1: shared `valid_model_id` helper in `cli/run.rs`. C2: extract the pure publish-gate fn.
5. C3 (optional, defer-by-default): `StatusKind` tagging for engine status lines; document the
   string contract in `docs/DEFERRED.md` if deferred. C6 (defer): verification busy-poll →
   notify, after H6-style multi-file NDJSON snapshots exist.

---

## 6. Guardrails (apply to every milestone)

- **G1 — Characterization first:** no structural move lands without the pinning test already
  green at HEAD (the W-wave's own discipline; each milestone above names its pins).
- **G2 — Local gates = CI:** `cargo test` (all targets) · `INSTA_UPDATE=no cargo test` +
  `cargo insta test --unreferenced=reject` + stray-`.snap.new` scan · `cargo clippy -- -D warnings`
  · `cargo fmt --check`. Codified in M0's TESTING.md rewrite; run per commit.
- **G3 — Docs sync is acceptance criteria:** every PR that moves a module, renames a symbol, or
  changes a contract updates AGENTS.md module map + relevant docs *in the same PR*. The two M0
  guard tests (lock-hierarchy completeness, §-anchor hygiene) plus the M5 snapshot manifest test
  make the common drift classes fail loudly.
- **G4 — One writer per PR;** milestones are sequential PRs on `refactor/architecture-*` branches
  mirroring the W-wave cadence (#43–47).
- **G5 — 800-line production ceiling** per the prior plan's metric; test files exempt *unless*
  M6's per-subject split applies (then each split file honors it). Enforce as an optional guard
test (line-count scan of `src/**/*.rs` excluding `*_tests.rs`/`#[cfg(test)]` regions), not prose.

---

## 7. Sequencing summary

```
Gate 0 sign-offs ─► M0 truth floor ─► M1 registry writer ─► M2 quick wins ─► M3 engine DAG+bundles ─► M4 API robustness ─► M5 UI layering+policy ─► M6 ownership+tests
                                              (M2 may be developed alongside M1; merges after)
```

Rough effort: M0 S · M1 M · M2 S · M3 M · M4 M · M5 M · M6 S–M → ~2–3 weeks of focused work,
shippable incrementally; every prefix of the sequence leaves the crate better than the last.

---

## 8. Explicitly deferred / rejected

| Item | Disposition |
|------|-------------|
| `lib.rs`/bin split | **Rejected this cycle** (§4 D-lib); revisit post-M3 |
| `CompleteDownloads` filename-key collision fix | Deferred to `docs/DEFERRED.md` (registry-bytes change) |
| `RegistryEntrySize::Zero` for `tui_quant` resumed GGUFs | Deferred (HUD cosmetics vs registry bytes; sign-off item if picked up) |
| `StatusKind` typed status lines (C3) | Deferred-by-default (string contract documented instead) |
| Verification busy-poll → notify (C6) | Deferred until multi-file NDJSON tail snapshots exist |
| Cross-process registry file lock | Optional M1 follow-on; without it, concurrent CLI + TUI runs can still lose updates (atomic save prevents torn files, not lost writes) — recorded in `docs/DEFERRED.md`; the single-writer claim in §2/§9 is scoped **in-process** |
| `models/{api,engine,ui}` name-collision renames | Doc note only (W3.6 precedent renames types, not worth the churn) |

---

## 9. Maintainability outcome (what "done" buys)

For **agents**: every rule an agent must obey is either type-enforced (bundle privacy, sealed
policies, module-private registry save), test-enforced (guard tests, manifest tests, goldens,
the DAG import check), or correctly documented (TESTING.md tells the truth; DEFERRED.md replaces
the §-lottery; module maps match the tree). For **humans**: the engine reads as four small
objects instead of 17 pub fields; the deadlock defense is a short per-bundle invariant list
instead of a rotting 10-level table; registry writes have one obvious right way — and one
impossible wrong way; and the hot paths (render, tree navigation, multi-part confirm) stop doing
hidden work.

---

## 10. Review evidence (provenance)

Four fresh-context read-only lanes at HEAD `a756371`, distinct seams and models; the
orchestrator independently re-verified the load-bearing P0 claims (mirror→disk saves at
`ui/app/downloads.rs` and `engine/enqueue.rs:111`; `files[0]` hardcode; `[..16]` slices; 11
engine mutexes with `verify_rx`/`outcome_rx` unlisted in AGENTS.md; TESTING.md's non-existent
targets; `plans/README.md` missing the executed plan) before incorporating them.

| Lane | Model | Seam | Full report |
|------|-------|------|-------------|
| structure | GLM-5.3 (wave-reviewer) | module graph, layering, EngineState | session artifacts `ce5df85d…` |
| ui | Gemini 3.8 Flash (gemini38-reviewer) | TUI app+render | session artifacts `1ef2b1dd…` |
| pipeline | GLM-5.3 (wave-reviewer) | CLI/engine/download/registry | session artifacts `97cb9946…` |
| readability | Qwen3.8 (reviewer, local) | docs/naming/tests/dead code | session artifacts `b51c6a72…` |

Reports live under the orchestrator session's subagent-artifacts directory; load-bearing findings
are re-stated with file:line evidence in §3 so this plan is self-contained.

## 11. Cross-model critique and disposition

A fifth, no-tools review (Claude Opus, high effort) audited this plan before commit. Accepted
and folded in: enforcement for both headline invariants (module-private `save_registry` +
fs-write scan guard; `module_dependency_dag` import check with a named re-export-shim expiry);
source-derived, section-scoped lock-doc guard; bidirectional DEFERRED key validation; Gate 0
sign-off collection (R4 blocks M1); Windows-aware atomic-save spec (same-dir temp, `sync_all`,
rename-with-retry; the tag-triggered release matrix tests Windows — verified against
`.github/workflows/release.yml`, the only workflow); `with_registry` returning the post-write
snapshot; strictly serial milestone order with the M3∥M4 overlap banned; U6 rename folded into
M3; deterministic B4 abort pin and E4 per-constructor equivalence pin (E4 acknowledged as a
bounded re-open of the sealed policy); TESTING.md target-existence guard; plan coordinates
stamped to `a756371`. Rejected/adjusted: per-PR CI was *not* found to exist (only the tag-only
release workflow — the premise stands, now stated precisely); path-keyed registry lock rejected
as complexity (global lock, documented test-serialization tradeoff).
