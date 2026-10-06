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
use crate::cli::args::HfCachePathArgs;
use crate::cli::run::{
    effective_revision, invalid_model_id_message, require_valid_model_id, resolve_run_token,
};
use crate::cli::{EXIT_AUTH, EXIT_FAILURE, EXIT_OK};

pub(super) async fn run_hf_cache_path(args: HfCachePathArgs) -> i32 {
    if let Err(code) = require_valid_model_id(&args.model_id) {
        eprintln!(
            "error [usage]: {}",
            invalid_model_id_message(&args.model_id)
        );
        return code;
    }
    let revision = effective_revision(&args.revision);
    let cache_dir = crate::cache_layout::hf_hub_cache(args.cache_dir.as_deref());
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
    // precedence — no engine, no apply_options). The shared client is
    // built here too (M4/B5): a malformed token is an explicit
    // auth failure, never a silent unauthenticated lookup.
    let token = resolve_run_token(args.token.clone(), &crate::config::load_config());
    let api_client = match crate::http_client::build_client_with_token(token.as_deref(), None) {
        Ok(client) => client,
        Err(e) => {
            let code = if matches!(e, crate::http_client::ClientBuildError::InvalidToken) {
                eprintln!("error [auth_required]: {e}");
                EXIT_AUTH
            } else {
                eprintln!("error [network]: {e}");
                EXIT_FAILURE
            };
            return code;
        }
    };
    match crate::api::resolve_revision_sha(&api_client, &args.model_id, &revision).await {
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
