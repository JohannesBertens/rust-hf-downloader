//! `hf-cache path` (plans/hf-cache-sync.md §2.1): pure path math plus a `refs/` lookup — the
//! scripting helper that answers "where is this model's snapshot?" without
//! ever touching the download engine.
//!
//! Split out of `cli/hf_cache_cmd.rs` (plan W3.5); moved verbatim. Refs
//! are written by [`super::sync`] (see [`super::sync::ref_name_for_revision`]);
//! the offline branch reads them straight off disk, and only the fallback
//! resolves the revision online through the Runner's token-only bootstrap
//! ([`crate::cli::run::resolve_run_token`]).

use super::absolute_path;
use crate::cli::args::{valid_model_id, HfCachePathArgs};
use crate::cli::run::{effective_revision, resolve_run_token};
use crate::cli::{EXIT_FAILURE, EXIT_OK, EXIT_USAGE};

pub(super) async fn run_hf_cache_path(args: HfCachePathArgs) -> i32 {
    if !valid_model_id(&args.model_id) {
        eprintln!(
            "error [usage]: invalid model ID {:?} — expected \"author/model-name\"",
            args.model_id
        );
        return EXIT_USAGE;
    }
    let revision = effective_revision(&args.revision);
    let cache_dir = crate::paths::hf_hub_cache(args.cache_dir.as_deref());
    let repo_dir = cache_dir.join(crate::cache_layout::repo_dir_name(&args.model_id));

    // refs/<rev> lookup: pure path math, no network (plans/hf-cache-sync.md §2.1).
    if let Ok(sha) = std::fs::read_to_string(repo_dir.join("refs").join(&revision)) {
        let sha = sha.trim();
        if !sha.is_empty() {
            let snapshot = absolute_path(&crate::cache_layout::snapshot_dir(
                &cache_dir,
                &args.model_id,
                sha,
            ));
            if !snapshot.is_dir() {
                eprintln!(
                    "note: snapshot directory is missing from the cache; \
                     re-run the sync to repair it"
                );
            }
            println!("{}", snapshot.display());
            return EXIT_OK;
        }
    }

    // Online fallback: resolve the revision to a commit SHA (Runner
    // partial bootstrap: load config, resolve the token by the run
    // precedence — no engine, no apply_options).
    let token = resolve_run_token(args.token.clone(), &crate::config::load_config());
    match crate::api::resolve_revision_sha(&args.model_id, &revision, token.as_ref()).await {
        Ok(sha) => {
            let snapshot = absolute_path(&crate::cache_layout::snapshot_dir(
                &cache_dir,
                &args.model_id,
                &sha,
            ));
            if !snapshot.is_dir() {
                eprintln!(
                    "note: snapshot not present in the cache yet; run \
                     `rust-hf-downloader hf-cache sync {} --revision {}` to populate it",
                    args.model_id, revision
                );
            }
            println!("{}", snapshot.display());
            EXIT_OK
        }
        Err(e) => {
            eprintln!(
                "error [network]: cannot resolve revision {} of {}: {}",
                revision, args.model_id, e
            );
            eprintln!(
                "hint: run `rust-hf-downloader hf-cache sync {}` to populate the cache first",
                args.model_id
            );
            EXIT_FAILURE
        }
    }
}
