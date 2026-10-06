//! `cli::resolve` tests (M6/T1 split of `cli/tests.rs`): file resolution
//! (`resolve_files` over every `Selector` flavor, ambiguity/miss error
//! shapes) and the `parse_selector` usage gate over parsed `DownloadArgs`.
//! The `FileSpec: From<&RepoFile>` sibling mapping is pinned here too —
//! including its second consumer (`tree_file_dtos`) agreeing on the wire.
//! Run: `cargo test resolve`

use crate::models::{ModelMetadata, QuantizationGroup};

use super::args::{DownloadArgs, RateLimitArgs, RunOutputArgs};
use super::events::FileDto;
use super::hf_cache::tree_file_dtos;
use super::report::ProgressMode;
use super::resolve::{
    parse_selector, resolve_files, FileSpec, ResolveError, SelectionError, Selector,
};
use super::testutil::{file_spec, metadata_with};

fn download_args(quant: Option<&str>, file: &[&str], all: bool) -> DownloadArgs {
    DownloadArgs {
        model_id: "a/b".to_string(),
        quant: quant.map(String::from),
        file: file.iter().map(|f| f.to_string()).collect(),
        all,
        run_output: RunOutputArgs {
            progress: ProgressMode::Auto,
            token: None,
            no_verify: false,
            json: false,
            quiet: false,
        },
        rate_limits: RateLimitArgs {
            rate_limit: false,
            no_rate_limit: false,
            rate_limit_mbps: None,
        },
        output: None,
        revision: None,
    }
}

#[test]
fn resolve_default_single_file_repo() {
    let metadata = metadata_with(&[("only.gguf", Some(10))]);
    let files = resolve_files(&metadata, &[], &Selector::Default).unwrap();
    assert_eq!(files, vec![file_spec("only.gguf", 10)]);
}

#[test]
fn resolve_default_multi_file_repo_is_ambiguous_with_list() {
    let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
    let err = resolve_files(&metadata, &[], &Selector::Default).unwrap_err();
    match err {
        ResolveError::Ambiguous { available } => {
            assert_eq!(available.len(), 2);
            assert_eq!(available[1].filename, "b.gguf");
        }
        other => panic!("expected Ambiguous, got {:?}", other),
    }
}

#[test]
fn resolve_quant_case_insensitive_from_groups() {
    let metadata = metadata_with(&[]);
    let quants = vec![QuantizationGroup {
        quant_type: "Q4_K_M".to_string(),
        files: vec![
            crate::models::QuantizationInfo {
                quant_type: "Q4_K_M".to_string(),
                filename: "m-00001-of-00002.gguf".to_string(),
                size: 1,
                sha256: Some("dead".to_string()),
            },
            crate::models::QuantizationInfo {
                quant_type: "Q4_K_M".to_string(),
                filename: "m-00002-of-00002.gguf".to_string(),
                size: 2,
                sha256: None,
            },
        ],
        total_size: 3,
    }];
    let files = resolve_files(&metadata, &quants, &Selector::Quant("q4_k_m".to_string())).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].sha256.as_deref(), Some("dead"));
    // all parts of the quantization are selected
    assert_eq!(files[1].filename, "m-00002-of-00002.gguf");
}

#[test]
fn resolve_quant_miss_lists_available() {
    let metadata = metadata_with(&[("a.gguf", Some(10))]);
    let err = resolve_files(&metadata, &[], &Selector::Quant("Q8_0".to_string())).unwrap_err();
    assert!(matches!(err, ResolveError::NoFilesMatch { .. }));
    assert_eq!(err.code(), "no_files_match");
}

#[test]
fn file_spec_from_repo_file_pins_the_sibling_mapping() {
    // W4.9 collapsed the twin sibling-mapping loops (resolve_files'
    // `available` and hf-cache selection's `tree_file_dtos`) into one
    // `From<&RepoFile> for FileSpec`. Pin the mapped values — including
    // size:None → 0 and lfs:None → sha256:None — and that both consumers
    // agree on every non-directory fixture.
    let lfs = |oid: &str| crate::models::LfsInfo {
        oid: oid.to_string(),
        size: 123,
        pointer_size: 132,
    };
    let repo_file = |name: &str, size: Option<u64>, lfs: Option<crate::models::LfsInfo>| {
        crate::models::RepoFile {
            rfilename: name.to_string(),
            size,
            oid: None,
            lfs,
        }
    };
    let with_lfs = repo_file("model.Q4_K_M.gguf", Some(4_947), Some(lfs("cafebabe")));
    let bare = repo_file("plain.bin", None, None);
    let sized_no_lfs = repo_file("non-lfs.safetensors", Some(42), None);

    assert_eq!(
        FileSpec::from(&with_lfs),
        FileSpec {
            filename: "model.Q4_K_M.gguf".to_string(),
            size_bytes: 4_947,
            sha256: Some("cafebabe".to_string()),
        }
    );
    assert_eq!(
        FileSpec::from(&bare),
        FileSpec {
            filename: "plain.bin".to_string(),
            size_bytes: 0,
            sha256: None,
        }
    );
    assert_eq!(
        FileSpec::from(&sized_no_lfs),
        FileSpec {
            filename: "non-lfs.safetensors".to_string(),
            size_bytes: 42,
            sha256: None,
        }
    );

    // The hf-cache selection payload (tree_file_dtos) is the same mapping
    // on the wire (FileDto::from(&FileSpec::from(f))), directory markers
    // filtered.
    let metadata = ModelMetadata {
        model_id: "a/b".to_string(),
        library_name: None,
        pipeline_tag: None,
        card_data: None,
        siblings: vec![
            with_lfs,
            bare,
            sized_no_lfs,
            repo_file("subdir/", Some(1), None),
        ],
        tags: Vec::new(),
        sha: None,
    };
    assert_eq!(
        tree_file_dtos(&metadata),
        vec![
            FileDto {
                filename: "model.Q4_K_M.gguf".to_string(),
                size_bytes: 4_947,
                sha256: Some("cafebabe".to_string()),
            },
            FileDto {
                filename: "plain.bin".to_string(),
                size_bytes: 0,
                sha256: None,
            },
            FileDto {
                filename: "non-lfs.safetensors".to_string(),
                size_bytes: 42,
                sha256: None,
            },
        ]
    );
}

#[test]
fn resolve_quant_mmproj_selects_all_projector_groups() {
    // Issue #25: `--quant mmproj` spans every MMPROJ* group; exact names
    // (MMPROJ-Q8_0) still match directly and never pull weight files in.
    let metadata = metadata_with(&[
        ("model.Q8_0.gguf", Some(10)),
        ("model.mmproj-Q8_0.gguf", Some(2)),
        ("mmproj-F32.gguf", Some(3)),
    ]);
    let quants = crate::api::classify_quantizations(&metadata.siblings);

    let files = resolve_files(&metadata, &quants, &Selector::Quant("mmproj".to_string())).unwrap();
    assert_eq!(
        files,
        vec![
            file_spec("mmproj-F32.gguf", 3),
            file_spec("model.mmproj-Q8_0.gguf", 2)
        ]
    );

    let files = resolve_files(
        &metadata,
        &quants,
        &Selector::Quant("MMPROJ-Q8_0".to_string()),
    )
    .unwrap();
    assert_eq!(files, vec![file_spec("model.mmproj-Q8_0.gguf", 2)]);
}

#[test]
fn resolve_files_exact_and_missing() {
    let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
    let files =
        resolve_files(&metadata, &[], &Selector::Files(vec!["b.gguf".to_string()])).unwrap();
    assert_eq!(files, vec![file_spec("b.gguf", 20)]);

    let err = resolve_files(
        &metadata,
        &[],
        &Selector::Files(vec!["nope.gguf".to_string()]),
    )
    .unwrap_err();
    assert_eq!(err.code(), "no_files_match");
    assert_eq!(err.available().len(), 2);
}

#[test]
fn resolve_files_dedups_repeatable_selectors() {
    let metadata = metadata_with(&[("a.gguf", Some(10))]);
    let files = resolve_files(
        &metadata,
        &[],
        &Selector::Files(vec!["a.gguf".to_string(), "a.gguf".to_string()]),
    )
    .unwrap();
    assert_eq!(files.len(), 1);
}

#[test]
fn resolve_all_skips_directories_and_unsized() {
    let metadata = metadata_with(&[
        ("a.gguf", Some(10)),
        ("subdir/", None), // directory marker
        ("broken", None),  // no size → skipped
    ]);
    let files = resolve_files(&metadata, &[], &Selector::All).unwrap();
    assert_eq!(files, vec![file_spec("a.gguf", 10)]);
}

#[test]
fn selector_default_when_none_given() {
    assert_eq!(
        parse_selector(&download_args(None, &[], false)).unwrap(),
        Selector::Default
    );
}

#[test]
fn selector_conflicts_rejected() {
    assert!(parse_selector(&download_args(Some("Q4"), &[], true)).is_err());
    assert!(parse_selector(&download_args(None, &["a.gguf"], true)).is_err());
    assert!(parse_selector(&download_args(Some("Q4"), &["a.gguf"], false)).is_err());
}
