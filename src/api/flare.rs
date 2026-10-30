//! FLARE repository scanning — pure core + thin fetchers, the
//! `api/quant.rs` precedent (plan `plans/flare-models.md` §3.1).
//!
//! Pure half (exhaustively testable, no I/O): [`flare_disposition`] (tag /
//! `publisher.json` sibling detection), [`assemble_flare_info_from`]
//! (refs + per-revision trees + main config/publisher → [`FlareRepoInfo`]),
//! [`select_package`] (flare's `choose_file` package rule),
//! [`parse_flare_package_tokens`] (display-only filename grammar),
//! [`config_json_confirms_flare`] / [`parse_flare_config_facts`] /
//! [`parse_flare_publisher`] (never-erroring payload parsers).
//!
//! Fetch half (every entry point takes the run's ONE shared
//! `&reqwest::Client` and routes through `api_base()`, plan M4/B5):
//! [`fetch_repo_refs`], [`fetch_raw_file`] and the orchestrating
//! [`fetch_flare_info`] (refs → concurrent per-revision trees → raw
//! `config.json` + `publisher.json` at main, LFS-guarded by the
//! already-fetched main tree).
//!
//! The commit is the pin everywhere: trees are keyed by `target_commit`,
//! and a refs entry without a usable `targetCommit` DROPS its revision into
//! [`FlareRepoInfo::dropped_refs`] instead of degrading to a name-based
//! lookup (plans/flare-models.md §3.1 — never a silent degradation).

use std::collections::HashMap;

use crate::api::{api_base, fetch_revision_tree};
use crate::models::{
    FlareConfigFacts, FlarePackage, FlarePublisher, FlareRepoInfo, FlareRevision, ModelMetadata,
    ParsedPackageTokens, RefKind, RepoFile, RepoRefs,
};

/// Result of the cheap FLARE detection pass over already-fetched metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlareDisposition {
    /// A `flare` tag is present — the repo is FLARE, no confirmation needed.
    Yes,
    /// No `flare` tag, but a root `publisher.json` sits in the siblings.
    /// Confirm via the config markers ([`config_json_confirms_flare`]).
    Probable,
    /// Neither marker — never scanned as FLARE.
    No,
}

/// Why a FLARE scan failed.
#[derive(Debug)]
pub enum FlareScanError {
    /// A Hub API request failed (transport or HTTP status).
    Http(reqwest::Error),
    /// No payload revision yielded a package — the repo is not a usable
    /// FLARE repository (per-revision "no package" alone is a SKIP, not an
    /// error; this is the all-skipped end state).
    NoPackage,
}

impl std::fmt::Display for FlareScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlareScanError::Http(e) => write!(f, "FLARE scan request failed: {e}"),
            FlareScanError::NoPackage => {
                write!(f, "no FLARE payload revision yielded a package")
            }
        }
    }
}

impl std::error::Error for FlareScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FlareScanError::Http(e) => Some(e),
            FlareScanError::NoPackage => None,
        }
    }
}

impl From<reqwest::Error> for FlareScanError {
    fn from(e: reqwest::Error) -> Self {
        FlareScanError::Http(e)
    }
}

/// Cheap detection over fetched metadata (plans/flare-models.md §3.1): `Yes` on a `flare`
/// tag; `Probable` when a root `publisher.json` is among the siblings
/// (confirmed later via the config markers); `No` otherwise.
///
/// The tag match is exact: `flare-runtime-package` (JohannesBertens' repo)
/// is NOT a `flare` tag and lands in `Probable` via its `publisher.json`.
pub fn flare_disposition(metadata: &ModelMetadata) -> FlareDisposition {
    if metadata.tags.iter().any(|tag| tag == "flare") {
        return FlareDisposition::Yes;
    }
    if metadata
        .siblings
        .iter()
        .any(|file| file.rfilename == "publisher.json")
    {
        return FlareDisposition::Probable;
    }
    FlareDisposition::No
}

/// Parse a raw JSON body; `None` on absent input or malformed JSON — the
/// config/publisher parsers NEVER error (plans/flare-models.md §3.1).
fn parse_json(raw: Option<&str>) -> Option<serde_json::Value> {
    serde_json::from_str(raw?).ok()
}

/// Does a `config.json` body confirm a Probable FLARE detection? The four
/// live-verified markers (plans/flare-models.md §3.1 / Research facts):
/// `"format" == "flare"`, `"runtime" == "flare"`,
/// `"model_type" == "flare-runtime-package"`, or a top-level `"flare"`
/// object. Anything else (including unparseable JSON) is not a confirmation.
pub fn config_json_confirms_flare(raw: Option<&str>) -> bool {
    let Some(root) = parse_json(raw) else {
        return false;
    };
    let is_flare = |key: &str| root.get(key).and_then(|v| v.as_str()) == Some("flare");
    is_flare("format")
        || is_flare("runtime")
        || root.get("model_type").and_then(|v| v.as_str()) == Some("flare-runtime-package")
        || root.get("flare").is_some_and(|v| v.is_object())
}

/// Extract the optional config facts from the top-level `flare` object of
/// a `config.json` body. Every field is optional; a malformed or absent
/// body yields all-`None` facts, never an error (plans/flare-models.md §3.1).
pub fn parse_flare_config_facts(raw: Option<&str>) -> FlareConfigFacts {
    let facts = parse_json(raw).and_then(|root| root.get("flare").cloned());
    let number = |key: &str| {
        facts
            .as_ref()
            .and_then(|flare| flare.get(key))
            .and_then(|v| v.as_u64())
    };
    FlareConfigFacts {
        serving_context: number("serving_context"),
        execution_abi: number("execution_abi"),
        prepared_model_contract: number("prepared_model_contract"),
        checkpoint: facts
            .as_ref()
            .and_then(|flare| flare.get("checkpoint"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

/// Parse the root `publisher.json` body — an ARRAY of publisher objects;
/// the first entry is the identity. `fingerprint` is the sha256-hex over
/// the raw `public_key` byte array, a display-only claim (no signature
/// verification, plan D7). Absent/malformed input or a non-array body
/// yields `None`, never an error.
pub fn parse_flare_publisher(raw: Option<&str>) -> Option<FlarePublisher> {
    let entry = parse_json(raw)?.as_array()?.first()?.clone();
    let text = |key: &str| entry.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let public_key: Option<Vec<u8>> =
        entry
            .get("public_key")
            .and_then(|v| v.as_array())
            .map(|bytes| {
                bytes
                    .iter()
                    .filter_map(|b| u8::try_from(b.as_u64()?).ok())
                    .collect()
            });
    Some(FlarePublisher {
        namespace: text("namespace"),
        name: text("name"),
        development: entry.get("development").and_then(|v| v.as_bool()),
        fingerprint: public_key.map(|key| hex::encode(<sha2::Sha256 as sha2::Digest>::digest(key))),
    })
}

/// Is every character an ASCII digit and the slice non-empty?
fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// A 40-hex commit SHA — the shape every `targetCommit` must have to be
/// usable as a pin (plans/flare-models.md §3.1: missing/invalid ⇒ the revision is DROPPED).
fn is_commit_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Basename with the `.safetensors` extension removed.
fn package_stem(filename: &str) -> &str {
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    basename.strip_suffix(".safetensors").unwrap_or(basename)
}

/// Does the stem look like a bare content hash (a metadata-only or
/// content-addressed file, never the packed package)?
fn is_hex64_stem(stem: &str) -> bool {
    stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Does the stem end in a `-data-NN-of-NN` shard suffix (a split-weight
/// member, never the packed package)?
fn is_data_shard_stem(stem: &str) -> bool {
    let Some((head, total)) = stem.rsplit_once("-of-") else {
        return false;
    };
    if !all_digits(total) {
        return false;
    }
    head.rsplit_once("-data-")
        .is_some_and(|(_, part)| all_digits(part))
}

/// flare's `choose_file` package rule (plans/flare-models.md §3.1), over one revision's
/// complete file listing:
///
/// - Directory form (checked first — its root `model.safetensors` would
///   otherwise satisfy the packed rule): root `model.safetensors` is the
///   primary, `weights/**` + `deployments/**` are the companions.
/// - Packed form: exactly ONE top-level `.safetensors` whose stem is
///   neither a bare 64-hex hash nor a `*-data-NN-of-NN` shard.
/// - Anything else yields `None` — the caller SKIPS such a revision
///   (a revision without a package is not an error, plans/flare-models.md §3.1).
pub fn select_package(files: &[RepoFile]) -> Option<FlarePackage> {
    if let Some(primary) = files.iter().find(|f| f.rfilename == "model.safetensors") {
        let companions = files
            .iter()
            .filter(|f| {
                // weights/** + deployments/** are the payload companions;
                // the two root metadata files ride the package too (plain
                // git blobs, no LFS oid) so the served directory form is
                // complete — the CLI's tree-union and the TUI's package
                // files share ONE source here (plans/flare-models.md §3.3c
                // directory-form set).
                f.rfilename.starts_with("weights/")
                    || f.rfilename.starts_with("deployments/")
                    || f.rfilename == "config.json"
                    || f.rfilename == "publisher.json"
            })
            .cloned()
            .collect();
        return Some(FlarePackage::Directory {
            primary: primary.clone(),
            companions,
        });
    }

    let mut packed: Vec<&RepoFile> = files
        .iter()
        .filter(|f| {
            let path = &f.rfilename;
            !path.contains('/') && path.ends_with(".safetensors") && {
                let stem = package_stem(path);
                !is_hex64_stem(stem) && !is_data_shard_stem(stem)
            }
        })
        .collect();
    match (packed.len(), packed.pop()) {
        (1, Some(file)) => Some(FlarePackage::Packed { file: file.clone() }),
        _ => None,
    }
}

/// The package's primary filename — the name the token grammar parses.
fn package_filename(package: &FlarePackage) -> &str {
    match package {
        FlarePackage::Packed { file } => &file.rfilename,
        FlarePackage::Directory { primary, .. } => &primary.rfilename,
    }
}

/// Parse flare's package-filename token grammar (plans/flare-models.md §3.1): `tp<N>`,
/// `c<N>`, `mtp<N>`, `i4`, `gb<token>`, free variants (letters immediately
/// followed by digits, e.g. `yarn4`). Matching is case-insensitive; unknown
/// tokens (the model-name parts, dots, …) are ignored. DISPLAY ONLY — the
/// pinning and package selection never consult this.
pub fn parse_flare_package_tokens(filename: &str) -> ParsedPackageTokens {
    let stem = package_stem(filename);
    let mut parsed = ParsedPackageTokens::default();
    for token in stem.split(['-', '_']) {
        let token = token.to_ascii_lowercase();
        let digits = |rest: &str| rest.parse::<u32>().ok().filter(|_| all_digits(rest));
        if let Some(rest) = token.strip_prefix("mtp") {
            if let Some(n) = digits(rest) {
                parsed.mtp_draft_tokens = Some(n);
                continue;
            }
        }
        if let Some(rest) = token.strip_prefix("tp") {
            if let Some(n) = digits(rest) {
                parsed.tp = Some(n);
                continue;
            }
        }
        if let Some(rest) = token.strip_prefix('c') {
            if let Some(n) = digits(rest) {
                parsed.max_concurrency = Some(n);
                continue;
            }
        }
        if token == "i4" {
            parsed.quant_i4 = true;
            continue;
        }
        if let Some(rest) = token.strip_prefix("gb") {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()) {
                parsed.arch = Some(token);
                continue;
            }
        }
        // Free variant: `^[a-z]+[0-9]+$` (e.g. `yarn4`). Everything else —
        // model-name parts like `qwen3.8`, `flash`, `next` — is ignored.
        let Some(split) = token.find(|c: char| c.is_ascii_digit()) else {
            continue;
        };
        let (letters, tail_digits) = token.split_at(split);
        if !letters.is_empty()
            && letters.bytes().all(|b| b.is_ascii_alphabetic())
            && all_digits(tail_digits)
        {
            parsed.variants.push(token);
        }
    }
    parsed
}

/// PURE assembly core (plans/flare-models.md §3.1): refs + trees keyed by `target_commit` +
/// the raw main-branch `config.json` / `publisher.json` bodies →
/// [`FlareRepoInfo`].
///
/// Payload refs = branches other than `main`, plus every tag, in refs
/// order. Entries are deduped by `target_commit` — the FIRST name becomes
/// the canonical revision (branches are scanned first, so a branch always
/// wins the preferred name) and later names become `aliases`. A refs entry
/// whose `targetCommit` is missing or not 40-hex is DROPPED and recorded in
/// `dropped_refs` (trees/URLs/registry pin to the commit; a nameless pin
/// cannot degrade). A revision whose tree is absent or yields no package
/// is skipped, not errored; only the fully-empty result is
/// [`FlareScanError::NoPackage`].
pub fn assemble_flare_info_from(
    refs: &RepoRefs,
    trees_by_target_commit: &HashMap<String, Vec<RepoFile>>,
    main_config_json: Option<&str>,
    publisher_json: Option<&str>,
) -> Result<FlareRepoInfo, FlareScanError> {
    let mut revisions: Vec<FlareRevision> = Vec::new();
    let mut dropped_refs: Vec<String> = Vec::new();
    let mut index_by_commit: HashMap<&str, usize> = HashMap::new();

    let payload_refs = refs
        .branches
        .iter()
        .filter(|entry| entry.name != "main")
        .map(|entry| (entry, RefKind::Branch))
        .chain(refs.tags.iter().map(|entry| (entry, RefKind::Tag)));

    for (entry, kind) in payload_refs {
        if !is_commit_sha(&entry.target_commit) {
            dropped_refs.push(entry.name.clone());
            continue;
        }
        if let Some(&i) = index_by_commit.get(entry.target_commit.as_str()) {
            revisions[i].aliases.push(entry.name.clone());
            continue;
        }
        let Some(package) = trees_by_target_commit
            .get(&entry.target_commit)
            .and_then(|files| select_package(files))
        else {
            // No tree or no package: a SKIP, never an error (plans/flare-models.md §3.1).
            continue;
        };
        let parsed = parse_flare_package_tokens(package_filename(&package));
        index_by_commit.insert(&entry.target_commit, revisions.len());
        revisions.push(FlareRevision {
            revision: entry.name.clone(),
            aliases: Vec::new(),
            kind,
            target_commit: entry.target_commit.clone(),
            package,
            parsed,
        });
    }

    if revisions.is_empty() {
        return Err(FlareScanError::NoPackage);
    }
    Ok(FlareRepoInfo {
        revisions,
        config: parse_flare_config_facts(main_config_json),
        publisher: parse_flare_publisher(publisher_json),
        dropped_refs,
    })
}

/// `GET {api_base()}/api/models/{model_id}/refs` → branch/tag entries with
/// their `targetCommit` pins (camelCase, no pagination — live-verified,
/// plan Research facts).
pub async fn fetch_repo_refs(
    client: &reqwest::Client,
    model_id: &str,
) -> Result<RepoRefs, reqwest::Error> {
    let url = format!("{}/api/models/{}/refs", api_base(), model_id);
    let response = crate::http_client::get_with_optional_token(client, &url).await?;
    // Surface HTTP errors as status errors, same contract as the sibling
    // fetchers in `client.rs`.
    let response = response.error_for_status()?;
    response.json().await
}

/// `GET {api_base()}/{model_id}/raw/{revision}/{path}` — pointer-free
/// content for NON-LFS files; an LFS path would return the pointer text.
/// The CALLER guards LFS using an already-fetched tree (plans/flare-models.md §3.1) — this
/// function issues no tree request of its own.
pub async fn fetch_raw_file(
    client: &reqwest::Client,
    model_id: &str,
    revision: &str,
    path: &str,
) -> Result<String, reqwest::Error> {
    let url = format!(
        "{}/{}/raw/{}/{}",
        api_base(),
        model_id,
        revision,
        urlencoding::encode(path)
    );
    let response = crate::http_client::get_with_optional_token(client, &url).await?;
    let response = response.error_for_status()?;
    response.text().await
}

/// Thin orchestration (plans/flare-models.md §3.1): refs → concurrent per-revision trees at
/// each payload `target_commit` (+ main) → raw `config.json` +
/// `publisher.json` at main, each LFS-guarded by the already-fetched main
/// tree → [`assemble_flare_info_from`].
///
/// A missing `config.json` (the LIL repo has none at main) or a failed raw
/// fetch degrades to `None` — the facts/publisher parsers never error.
/// Tree failures propagate as [`FlareScanError::Http`].
pub async fn fetch_flare_info(
    client: &reqwest::Client,
    model_id: &str,
) -> Result<FlareRepoInfo, FlareScanError> {
    let refs = fetch_repo_refs(client, model_id).await?;

    // Commits to fetch: the deduped payload commits, plus main (its tree is
    // the LFS guard for the raw fetches below).
    let main_commit = refs
        .branches
        .iter()
        .find(|entry| entry.name == "main")
        .map(|entry| entry.target_commit.clone())
        .filter(|sha| is_commit_sha(sha));
    let mut commits: Vec<String> = Vec::new();
    {
        let mut remember = |sha: &str| {
            if is_commit_sha(sha) && !commits.iter().any(|seen| seen == sha) {
                commits.push(sha.to_string());
            }
        };
        for entry in refs.branches.iter().filter(|e| e.name != "main") {
            remember(&entry.target_commit);
        }
        for entry in &refs.tags {
            remember(&entry.target_commit);
        }
        if let Some(sha) = &main_commit {
            remember(sha);
        }
    }

    // Concurrent per-revision tree fetches (plans/flare-models.md §3.1).
    let trees = futures::future::join_all(
        commits
            .iter()
            .map(|sha| fetch_revision_tree(client, model_id, sha)),
    )
    .await;
    let mut trees_by_target_commit: HashMap<String, Vec<RepoFile>> = HashMap::new();
    for (sha, tree) in commits.iter().zip(trees) {
        trees_by_target_commit.insert(sha.clone(), tree?);
    }

    // Raw files at main — only when the main tree proves the path exists
    // AND is not LFS (a pointer file would poison the parsers).
    let mut main_config_json = None;
    let mut publisher_json = None;
    if let (Some(sha), Some(main_tree)) = (
        &main_commit,
        main_commit
            .as_ref()
            .and_then(|s| trees_by_target_commit.get(s)),
    ) {
        let plain_file = |path: &str| {
            main_tree
                .iter()
                .any(|f| f.rfilename == path && f.lfs.is_none())
        };
        if plain_file("config.json") {
            main_config_json = fetch_raw_file(client, model_id, sha, "config.json")
                .await
                .ok();
        }
        if plain_file("publisher.json") {
            publisher_json = fetch_raw_file(client, model_id, sha, "publisher.json")
                .await
                .ok();
        }
    }

    assemble_flare_info_from(
        &refs,
        &trees_by_target_commit,
        main_config_json.as_deref(),
        publisher_json.as_deref(),
    )
}

/// Does one payload revision target the detected platform? (impl-gate fix:
/// flare's OWN resolution rule is "branch name = platform string" — the
/// research-facts line plans/flare-models.md carries — with the package
/// filename's parsed arch token as a secondary signal.) The flagship repos
/// (`…-yarn4-tp2`, the LIL `tp1-c16-mtp5` gb202 package) carry NO arch
/// token, so matching on `parsed.arch` alone rejected exactly the repos
/// the feature targets. Shared by the CLI `--gpus` selection and the TUI's
/// platform star.
pub fn revision_matches_platform(
    revision_name: &str,
    kind: crate::models::RefKind,
    arch: Option<&str>,
    platform: &crate::gpu::FlarePlatform,
) -> bool {
    arch == Some(platform.as_str())
        || (kind == crate::models::RefKind::Branch && revision_name == platform.as_str())
}

/// Every payload revision targeting the platform (0, 1, or more — callers
/// error on ambiguity). See [`revision_matches_platform`].
pub fn select_platform_revisions<'a>(
    revisions: &'a [crate::models::FlareRevision],
    platform: &crate::gpu::FlarePlatform,
) -> Vec<&'a crate::models::FlareRevision> {
    revisions
        .iter()
        .filter(|r| {
            revision_matches_platform(&r.revision, r.kind, r.parsed.arch.as_deref(), platform)
        })
        .collect()
}
