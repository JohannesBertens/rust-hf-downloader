//! HuggingFace Hub API client: model search with client-side
//! filtering/sorting, metadata fetching, file-tree construction, multipart
//! SHA256 lookups, and GGUF quantization classification.
//!
//! All base URLs route through [`api_base`], which honours `HF_ENDPOINT`.
//!
//! W3.2 split the former single-file module into three submodules; this file
//! is a facade that re-exports them wholesale, so every `crate::api::X`
//! import keeps compiling unchanged:
//!
//! - `client` — everything that talks HTTP: search (with the client-side
//!   filter/sort pass), metadata + recursive tree fetches, revision SHA
//!   resolution
//! - `quant` — the pure GGUF heuristics: quantization-group classification,
//!   mmproj handling, `looks_like_quant_type`, the multipart filename
//!   grammars
//! - `flare` — FLARE repo scanning (plan `plans/flare-models.md` §3.1):
//!   pure detection/assembly/token core + the thin `/refs`, raw-file and
//!   per-revision-tree fetchers
//! - `tree` — `FileTreeNode` construction from a flat `RepoFile` listing
//!
//! URL building stays here: [`api_base`], [`DEFAULT_REVISION`] and
//! [`resolve_url`] are the single builders every submodule (and the download
//! engine, the registry bookkeeping in both frontends, and the CLI) share.
//!
//! The submodules are private on purpose (`mod client;`, never
//! `pub mod client;`) — mirroring `models/mod.rs`, a public `api::client`
//! could be dragged in by a `use crate::api::*` glob and shadow unrelated
//! names. Tests moved to the submodule that owns what they test.

mod client;
mod flare;
mod quant;
mod tree;

pub use client::*;
// Wave-1 foundation (plans/flare-models.md §7): the re-exports gain their
// first production reader when wave-2 lanes C/D merge — drop the allow then.
#[allow(unused_imports)]
pub use flare::{
    assemble_flare_info_from, config_json_confirms_flare, fetch_flare_info, fetch_raw_file,
    fetch_repo_refs, flare_disposition, parse_flare_config_facts, parse_flare_package_tokens,
    parse_flare_publisher, FlareDisposition, FlareScanError,
};
pub use quant::*;
pub use tree::*;

#[cfg(test)]
mod flare_tests;

/// Base URL for all HuggingFace Hub requests.
///
/// Overridable via the `HF_ENDPOINT` environment variable (same convention
/// as `huggingface_hub`), which enables mirror support (e.g.
/// `HF_ENDPOINT=https://hf-mirror.com`) and hermetic integration tests
/// against a local mock server.
pub fn api_base() -> String {
    std::env::var("HF_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://huggingface.co".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The revision used when the user doesn't pick one (issue #28). A
/// "revision" is a branch name, tag, or git commit SHA in Hub terms.
pub const DEFAULT_REVISION: &str = "main";

/// Canonical `resolve` download URL for a repo file. Used by the download
/// engine, the registry bookkeeping in both frontends, and the CLI — keeping
/// one builder guarantees the URLs always match.
pub fn resolve_url(model_id: &str, filename: &str, revision: &str) -> String {
    format!(
        "{}/{}/resolve/{}/{}",
        api_base(),
        model_id,
        revision,
        filename
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_url_embeds_revision() {
        // Expectations are built from api_base() itself so the test stays
        // correct under HF_ENDPOINT overrides (mirrors, CI).
        let base = api_base();
        assert_eq!(
            resolve_url("a/b", "model.gguf", DEFAULT_REVISION),
            format!("{base}/a/b/resolve/main/model.gguf")
        );
        assert_eq!(
            resolve_url("a/b", "sub/model.gguf", "2.0bpw"),
            format!("{base}/a/b/resolve/2.0bpw/sub/model.gguf")
        );
        // commit SHA revisions use the same shape
        assert_eq!(
            resolve_url("a/b", "m.gguf", "0123456789abcdef"),
            format!("{base}/a/b/resolve/0123456789abcdef/m.gguf")
        );
    }
}
