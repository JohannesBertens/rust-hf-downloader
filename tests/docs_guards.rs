//! Documentation truth guards (M0 of `plans/architecture-simplification-review.md`).
//!
//! Pure filesystem + regex checks — they exercise no binary behavior, only
//! the documents agents navigate by. Each guard pins one drift class found
//! by the plan's review lanes:
//!
//! 1. [`lock_hierarchy_documents_every_engine_mutex_field`] — the AGENTS.md
//!    lock hierarchy documents **every** `Arc<Mutex<..>>`/atomic field of
//!    the engine's state bundles (`EngineState` + `QueueAccounting` +
//!    `EventBus` in `src/engine/mod.rs`, `VerificationHub` in
//!    `src/verification.rs`), derived from the source (not a
//!    hand-maintained list), matched only inside the delimited hierarchy
//!    section (finding E3: the table omitted `verify_rx`/`outcome_rx`/
//!    `verification_in_flight` and rotted silently as fields were added;
//!    M3 rewired the derivation from the flat field list to the bundle
//!    struct bodies).
//! 2. [`no_bare_plan_section_anchors_in_src`] — no bare `§N` plan anchor in
//!    `src/**`: a `§<digit>` on a line must co-occur with `plans/` or
//!    `docs/DEFERRED.md`, and the DEFERRED citation is validated in both
//!    directions (every cited `docs/DEFERRED.md#<key>` exists in the
//!    register; every register entry's anchor symbol is still greppable in
//!    `src/`) — finding D2: the old `§8.x` regime resolved to an unindexed
//!    plan with stale coordinates and resolved items still cited.
//! 3. [`testing_md_targets_exist`] — every `cargo test --test <name>` (and
//!    any library-target test flag) mentioned in TESTING.md resolves to a
//!    real target — finding D1: TESTING.md prescribed non-existent targets
//!    (`--test integration`, a lib target on a bin-only crate).
//! 4. [`registry_disk_writes_confined_to_registry_module`] — no
//!    `fs::write`/`File::create`/`write_all` on a registry path outside
//!    `src/registry.rs` + its own submodule — M1 of
//!    `plans/architecture-simplification-review.md`: registry writes are
//!    serialized through `registry::with_registry` (single writer,
//!    atomic save); `load_registry`/`save_registry` are module-private,
//!    so this guard catches the *hand-rolled* fourth bypass site (a raw
//!    fs write that privatization alone cannot stop).
//! 5. [`module_dependency_dag`] — `src/verification.rs` and
//!    `src/download/**` contain no `crate::engine` reference at all — M3
//!    of `plans/architecture-simplification-review.md`: the engine
//!    triangle (engine ↔ download ↔ verification) becomes a DAG, with
//!    the engine as the sole composite owner; the textual check keeps
//!    the next cycle from re-forming silently.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read a text file with CRLF normalized to LF. The release workflow runs
/// `cargo test --locked` on Windows, where a checkout without a
/// `.gitattributes` eol policy can materialize `\r\n` line endings — every
/// literal `"\n…\n"` search below must survive that.
fn read_normalized(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read_to_string {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// Escape a literal for embedding in a regex (symbols like
/// `AppOptions::default` or generic-ish tokens must not be interpreted).
fn regex_escape(literal: &str) -> String {
    literal.chars().flat_map(|c| c.escape_debug()).collect()
}

/// Every `*.rs` and `*.md` file under `src/`, as (path, contents).
fn src_files() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
        for entry in entries {
            let path = entry.expect("read_dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("rs") | Some("md")
            ) {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(&root().join("src"), &mut paths);
    paths
        .into_iter()
        .map(|path| (path.clone(), read_normalized(&path)))
        .collect()
}

// ---------------------------------------------------------------------------
// Guard (a): AGENTS.md lock hierarchy completeness
// ---------------------------------------------------------------------------

/// Derive the lock-carrying engine field names from the BUNDLE struct
/// definitions (M3 rewiring): `EngineState`, `QueueAccounting`, and
/// `EventBus` in `src/engine/mod.rs`, plus `VerificationHub` in
/// `src/verification.rs`. A field counts when its type is `Arc<Mutex<..>>`
/// (directly or through a same-file type alias such as `DownloadReceiver`)
/// or `Arc<Atomic*>`. Deriving — rather than hand-listing — means a new
/// field in ANY bundle fails this test until the hierarchy documents it.
#[test]
fn lock_hierarchy_documents_every_engine_mutex_field() {
    let engine_src = read_normalized(&root().join("src/engine/mod.rs"));
    let verification_src = read_normalized(&root().join("src/verification.rs"));

    // One level of alias resolution: `pub type X = Arc<Mutex<..>>`.
    let alias_re = Regex::new(r"(?m)^pub type (\w+) = Arc<Mutex<").unwrap();
    let mutex_aliases: Vec<&str> = alias_re
        .captures_iter(&engine_src)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();

    // The `pub <field>: <type>` lines of one named struct body (from the
    // declaration to the first column-0 closing brace). Attribute and doc
    // lines inside the body do not match the field regex and are skipped.
    fn struct_fields(src: &str, struct_name: &str) -> Vec<(String, String)> {
        let decl = format!("pub struct {struct_name}");
        let start = src
            .find(&decl)
            .unwrap_or_else(|| panic!("{struct_name} definition present"));
        let rest = &src[start..];
        let body_end = rest
            .find("\n}\n")
            .unwrap_or_else(|| panic!("{struct_name} body terminates"));
        let body = &rest[..body_end];
        let field_re = Regex::new(r"^\s*pub\s+(\w+)\s*:\s*([^,]+?),?\s*$").unwrap();
        body.lines()
            .filter_map(|line| {
                field_re.captures(line).map(|c| {
                    (
                        c.get(1).unwrap().as_str().to_string(),
                        c.get(2).unwrap().as_str().trim().to_string(),
                    )
                })
            })
            .collect()
    }

    let mut mutex_fields: Vec<String> = Vec::new();
    let mut atomic_fields: Vec<String> = Vec::new();
    for (src, bundle) in [
        (&engine_src, "EngineState"),
        (&engine_src, "QueueAccounting"),
        (&engine_src, "EventBus"),
        (&verification_src, "VerificationHub"),
    ] {
        for (name, ty) in struct_fields(src, bundle) {
            if ty.starts_with("Arc<Mutex<") || mutex_aliases.contains(&ty.as_str()) {
                mutex_fields.push(name);
            } else if ty.starts_with("Arc<Atomic") {
                atomic_fields.push(name);
            }
        }
    }

    // Sanity floors: if a bundle moves or the regex drifts, fail loudly
    // instead of passing vacuously. (Counts as of the M3 regroup: 12 mutex
    // (4 EngineState + 2 QueueAccounting + 3 EventBus + 3 VerificationHub)
    // + 2 atomic. If fields were intentionally REMOVED, lower the floor in
    // the same PR.)
    assert!(
        mutex_fields.len() >= 12,
        "derived only {mutex_fields:?} mutex fields — the bundle derivation is stale \
         (or fields were intentionally removed: lower the floor in the same PR)"
    );
    assert!(
        atomic_fields.len() >= 2,
        "derived only {atomic_fields:?} atomic fields — the bundle derivation is stale \
         (or fields were intentionally removed: lower the floor in the same PR)"
    );

    // The delimited lock-hierarchy section of the root AGENTS.md.
    let agents = read_normalized(&root().join("AGENTS.md"));
    let (begin, end) = (
        "<!-- lock-hierarchy:begin -->",
        "<!-- lock-hierarchy:end -->",
    );
    let section_start = agents
        .find(begin)
        .unwrap_or_else(|| panic!("AGENTS.md lacks the `{begin}` delimiter"));
    let section_end = agents
        .find(end)
        .unwrap_or_else(|| panic!("AGENTS.md lacks the `{end}` delimiter"));
    assert!(
        section_end > section_start,
        "AGENTS.md lock-hierarchy delimiters are out of order"
    );
    let section = &agents[section_start..section_end];

    // Backtick-quoted match *within the section only* — the bundle member
    // names include short ones (`queue`, `size`, `progress`), so a bare
    // word-boundary match would false-pass on prose fragments; requiring
    // the backticked form keeps both long and short names honest.
    for name in mutex_fields.iter().chain(atomic_fields.iter()) {
        let needle = format!("`{name}`");
        assert!(
            section.contains(&needle),
            "AGENTS.md lock-hierarchy section does not document engine field `{name}` \
             (delimited section between `{begin}` and `{end}`)"
        );
    }
}

// ---------------------------------------------------------------------------
// Guard (b): §-anchor hygiene + bidirectional DEFERRED validation
// ---------------------------------------------------------------------------

#[test]
fn no_bare_plan_section_anchors_in_src() {
    let files = src_files();
    let deferred_path = root().join("docs/DEFERRED.md");
    let deferred = read_normalized(&deferred_path);

    // (b1) A `§<digit>` on a src line must co-occur with `plans/` (a named
    // plan file) or `docs/DEFERRED.md` (the register). Bare `§N.M` anchors
    // resolve to nothing findable — the D2 drift class.
    let bare_re = Regex::new(r"§\d").unwrap();
    let mut violations: Vec<String> = Vec::new();
    for (path, contents) in &files {
        for (idx, line) in contents.lines().enumerate() {
            if bare_re.is_match(line)
                && !line.contains("plans/")
                && !line.contains("docs/DEFERRED.md")
            {
                violations.push(format!("  {}:{}: {}", path.display(), idx + 1, line.trim()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "src lines cite a bare §-anchor without naming the file — every `§N` must \
         co-occur with `plans/…` or `docs/DEFERRED.md` on the same line:\n{}",
        violations.join("\n")
    );

    // Register entries: `## <key>` headings in docs/DEFERRED.md.
    let entry_re = Regex::new(r"(?m)^## ([a-z0-9-]+)\s*$").unwrap();
    let entries: Vec<&str> = entry_re
        .captures_iter(&deferred)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    assert!(
        !entries.is_empty(),
        "docs/DEFERRED.md has no `## <key>` entries"
    );

    // (b2) Every `docs/DEFERRED.md#<key>` cited in src exists in the register.
    let cite_re = Regex::new(r"docs/DEFERRED\.md#([a-z0-9-]+)").unwrap();
    for (path, contents) in &files {
        for key in cite_re
            .captures_iter(contents)
            .map(|c| c.get(1).unwrap().as_str().to_string())
        {
            assert!(
                entries.contains(&key.as_str()),
                "{} cites docs/DEFERRED.md#{key}, but the register has no `## {key}` entry",
                path.display()
            );
        }
    }

    // (b3) Every register entry's anchor symbol is still greppable in src —
    // an entry whose symbol is gone is resolved-or-orphaned and must be
    // updated in the same PR that removed the symbol. The corpus is
    // COMMENT-STRIPPED and matched on word boundaries: a symbol that only
    // survives inside comments (e.g. the citing `docs/DEFERRED.md#…`
    // comment itself) does not keep an entry alive, and short tokens no
    // longer match longer names (`RateLimiter` vs `RateLimiterState`).
    let block_comment_re = Regex::new(r"(?s)/\*.*?\*/").unwrap();
    let corpus: String = files
        .iter()
        .map(|(_, contents)| {
            let no_blocks = block_comment_re.replace_all(contents.as_str(), "");
            no_blocks
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let symbol_re = Regex::new(r"(?m)^- \*\*Symbols?:\*\*\s+(.+)$").unwrap();
    let sym_token_re = Regex::new(r"`([^`]+)`").unwrap();
    let status_re =
        Regex::new(r"(?m)^- \*\*Status:\*\* (open|deferred|fix in flight \(M\d|resolved)").unwrap();
    let mut checked = 0usize;
    for chunk in deferred.split("\n## ").skip(1) {
        let key = chunk.lines().next().unwrap_or_default().trim();
        let symbols = symbol_re
            .captures(chunk)
            .unwrap_or_else(|| panic!("register entry `{key}` has no `- **Symbol:** …` line"))
            .get(1)
            .unwrap()
            .as_str();
        for token in sym_token_re
            .captures_iter(symbols)
            .map(|c| c.get(1).unwrap().as_str())
        {
            checked += 1;
            let re = Regex::new(&format!(r"\b{}\b", regex_escape(token)))
                .unwrap_or_else(|e| panic!("symbol {token} is not regex-safe: {e}"));
            assert!(
                re.is_match(&corpus),
                "register entry `{key}` anchors on symbol `{token}`, which no longer \
                 appears as live code in src/ (comments do not count) — resolve or \
                 re-anchor the entry"
            );
        }
        // Status vocabulary: one of the four documented states, else the
        // register drifts into free text the guards cannot reason about.
        assert!(
            status_re.is_match(chunk),
            "register entry `{key}` has no `- **Status:** <open|deferred|fix in flight (M#)|resolved>` line"
        );
    }
    assert!(
        checked >= entries.len(),
        "expected at least one checked symbol per register entry (checked {checked}, \
         entries {})",
        entries.len()
    );
}

// ---------------------------------------------------------------------------
// Guard (d): registry writes confined to the registry module (M1)
// ---------------------------------------------------------------------------

/// No direct filesystem write to the registry file outside `src/registry.rs`
/// and its own submodules (`src/registry/**`). Every registry mutation must
/// go through `registry::with_registry` — the process-global single writer
/// with the atomic temp+rename save. `load_registry`/`save_registry` are
/// module-private, so a bypass that *reuses* them cannot compile; this
/// guard catches the shape privatization cannot: a call site hand-rolling
/// `fs::write`/`File::create` (or `write_all` on a handle) against
/// `registry_path()` / the `hf-downloads.toml` file name. The registry
/// module's own tests (`registry_tests` fixtures) are exempt — they are
/// inside the module that owns the write discipline.
#[test]
fn registry_disk_writes_confined_to_registry_module() {
    let write_re =
        Regex::new(r"fs::write|fs::copy|File::create|write_all|OpenOptions|File::options").unwrap();
    let registry_path_re = Regex::new(r"registry_path|hf-downloads\.toml").unwrap();
    let mut violations: Vec<String> = Vec::new();
    for (path, contents) in src_files() {
        let normalized = path.to_string_lossy().replace('\\', "/");
        if normalized.ends_with("src/registry.rs") || normalized.contains("/registry/") {
            continue; // the module that owns the discipline (+ its fixtures)
        }
        for (idx, line) in contents.lines().enumerate() {
            if write_re.is_match(line) && registry_path_re.is_match(line) {
                violations.push(format!("  {}:{}: {}", path.display(), idx + 1, line.trim()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "registry writes live outside src/registry.rs — every mutation must go \
         through registry::with_registry (single writer, atomic save); a direct \
         fs write to the registry path is a bypass site:\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Guard (e): module dependency DAG (M3)
// ---------------------------------------------------------------------------

/// `src/verification.rs` and every file under `src/download/` must contain
/// no `crate::engine` reference — not even in comments (a doc line naming
/// the engine path in a module that must not import it is the same drift,
/// one rename away from a real import). This is the durable, test-enforced
/// side of the M3 DAG: `download` and `verification` depend only on models
/// and registry (plus the `VerificationHub` seam `verification` itself
/// owns); the engine imports them, never the reverse.
#[test]
fn module_dependency_dag() {
    let mut violations: Vec<String> = Vec::new();
    for (path, contents) in src_files() {
        let normalized = path.to_string_lossy().replace('\\', "/");
        let in_scope =
            normalized.ends_with("src/verification.rs") || normalized.contains("/src/download/");
        if !in_scope {
            continue;
        }
        for (idx, line) in contents.lines().enumerate() {
            if line.contains("crate::engine") {
                violations.push(format!("  {}:{}: {}", path.display(), idx + 1, line.trim()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "src/verification.rs and src/download/** must not reference the engine \
         module (M3 DAG: download/verification depend on models + registry + the \
         VerificationHub seam only):\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Guard (c): TESTING.md mentions only real test targets
// ---------------------------------------------------------------------------

#[test]
fn testing_md_targets_exist() {
    let testing = read_normalized(&root().join("TESTING.md"));

    // Every `cargo test --test <name>` must resolve to tests/<name>.rs.
    let test_re = Regex::new(r"--test[ =]([A-Za-z0-9_-]+)").unwrap();
    let mut seen: Vec<&str> = Vec::new();
    for name in test_re
        .captures_iter(&testing)
        .map(|c| c.get(1).unwrap().as_str())
    {
        let target = root().join(format!("tests/{name}.rs"));
        assert!(
            target.exists(),
            "TESTING.md references `cargo test --test {name}` but {} does not exist",
            target.display()
        );
        seen.push(name);
    }
    assert!(
        !seen.is_empty(),
        "TESTING.md lists no integration targets — the guard would pass vacuously"
    );

    // A library-target test flag requires src/lib.rs (this crate is bin-only).
    if testing.contains("--lib") {
        assert!(
            root().join("src/lib.rs").exists(),
            "TESTING.md mentions a library-target test flag but src/lib.rs does not exist"
        );
    }

    // Targets this crate does not have: no benches/, no doc tests on a
    // bin-only crate with no library, no examples/ — a `--bench`/`--doc`/
    // `--example` invocation in TESTING.md describes a target that cannot
    // run (finding D1's class, on the flag side). Intent: the docs must not
    // prescribe targets the crate lacks; if the crate ever gains one of
    // these targets, update this guard in the same PR.
    for flag in ["--bench", "--doc", "--example"] {
        assert!(
            !testing.contains(flag),
            "TESTING.md mentions `{flag}` but this crate has no such target \
             (no benches/, no lib doc-tests, no examples/) — if the target was \
             added, update this guard in the same PR"
        );
    }
}
