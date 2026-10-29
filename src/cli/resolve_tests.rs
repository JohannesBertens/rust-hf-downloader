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
        for_preset: None,
        gpus: None,
        include: Vec::new(),
        exclude: Vec::new(),
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
            remote_path: None,
        }
    );
    assert_eq!(
        FileSpec::from(&bare),
        FileSpec {
            filename: "plain.bin".to_string(),
            size_bytes: 0,
            sha256: None,
            remote_path: None,
        }
    );
    assert_eq!(
        FileSpec::from(&sized_no_lfs),
        FileSpec {
            filename: "non-lfs.safetensors".to_string(),
            size_bytes: 42,
            sha256: None,
            remote_path: None,
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

// --- FLARE preset selection (plans/flare-models.md §3.3b/§3.3c) ---------

use super::resolve::{
    flare_package_set, is_commit_sha, payload_revision_names, preferred_revision_name,
    select_flare_files, FlareFile,
};
use crate::models::{FlarePackage, LfsInfo, RefEntry, RepoFile, RepoRefs};

fn repo_file(path: &str, size: u64, lfs_oid: Option<&str>) -> RepoFile {
    RepoFile {
        rfilename: path.to_string(),
        size: Some(size),
        oid: None,
        lfs: lfs_oid.map(|oid| LfsInfo {
            oid: oid.to_string(),
            size,
            pointer_size: 128,
        }),
    }
}

fn flare_file(relpath: &str, size: u64, sha: Option<&str>) -> FlareFile {
    FlareFile {
        relpath: relpath.to_string(),
        size,
        sha256: sha.map(str::to_string),
    }
}

fn sha40(n: u8) -> String {
    format!("{n:040x}")
}

#[test]
fn is_commit_sha_pins_the_ladder_shape() {
    // (i) ladder step: a 40-hex value is a direct pin.
    assert!(is_commit_sha(&sha40(1)));
    assert!(is_commit_sha(&"a".repeat(40).to_uppercase()));
    // Names, short hashes, and hex-wrong lengths are NOT.
    assert!(!is_commit_sha("gb202"));
    assert!(!is_commit_sha("v1"));
    assert!(!is_commit_sha(&"a".repeat(39)));
    assert!(!is_commit_sha(&"a".repeat(41)));
    assert!(!is_commit_sha(""));
    assert!(!is_commit_sha(&"z".repeat(40)));
}

#[test]
fn flare_package_set_packed_is_the_single_file() {
    let file = repo_file("qwen3.8-flash-next-gb10-tp1.safetensors", 1, Some("cafe"));
    let package = FlarePackage::Packed { file: file.clone() };
    // The tree holds extra repo files — the packed set ignores them.
    let tree = vec![
        file,
        repo_file("README.md", 10, None),
        repo_file("publisher.json", 2, None),
    ];
    // The packed form is self-contained: even present root metadata files
    // do NOT join the set (plans/flare-models.md §3.3c scopes config.json/publisher.json to the
    // directory form).
    assert_eq!(
        flare_package_set(&package, &tree),
        vec![flare_file(
            "qwen3.8-flash-next-gb10-tp1.safetensors",
            1,
            Some("cafe")
        )]
    );
}

#[test]
fn flare_package_set_directory_adds_root_metadata_files() {
    let primary = repo_file("model.safetensors", 100, Some("prime"));
    let companions = vec![
        repo_file("weights/x-0.safetensors", 50, Some("sha-x")),
        repo_file("deployments/gb202-tp1/rank-0/plan.bin", 5, None),
    ];
    let package = FlarePackage::Directory {
        primary: primary.clone(),
        companions: companions.clone(),
    };
    let tree = vec![
        primary.clone(),
        companions[0].clone(),
        companions[1].clone(),
        repo_file("config.json", 3, None),
        repo_file("publisher.json", 4, None),
        repo_file("README.md", 10, None), // NOT part of the package
    ];
    assert_eq!(
        flare_package_set(&package, &tree),
        vec![
            flare_file("model.safetensors", 100, Some("prime")),
            flare_file("weights/x-0.safetensors", 50, Some("sha-x")),
            flare_file("deployments/gb202-tp1/rank-0/plan.bin", 5, None),
            flare_file("config.json", 3, None),
            flare_file("publisher.json", 4, None),
        ],
        "directory set = primary + weights/** + deployments/** + config.json + publisher.json"
    );

    // Missing root metadata files are simply absent (LIL has no config.json).
    // (A tree without the companions would not have produced them either —
    // wave-1's select_package derives companions from the same tree.)
    let tree_without = vec![primary.clone(), companions[0].clone()];
    let package_without = FlarePackage::Directory {
        primary: primary.clone(),
        companions: vec![companions[0].clone()],
    };
    assert_eq!(
        flare_package_set(&package_without, &tree_without)
            .iter()
            .map(|f| f.relpath.as_str())
            .collect::<Vec<_>>(),
        vec!["model.safetensors", "weights/x-0.safetensors"]
    );
}

#[test]
fn select_flare_files_prefixes_disk_and_keeps_remote_path() {
    let set = vec![
        flare_file("model.safetensors", 100, Some("prime")),
        flare_file("config.json", 3, None),
    ];
    let files = select_flare_files(&set, "gb202", &[], &[]).unwrap();
    assert_eq!(
        files,
        vec![
            FileSpec {
                filename: "gb202/model.safetensors".to_string(),
                size_bytes: 100,
                sha256: Some("prime".to_string()),
                remote_path: Some("model.safetensors".to_string()),
            },
            FileSpec {
                filename: "gb202/config.json".to_string(),
                size_bytes: 3,
                sha256: None,
                remote_path: Some("config.json".to_string()),
            },
        ]
    );
}

#[test]
fn select_flare_files_filters_within_the_package_set_on_repo_paths() {
    let set = vec![
        flare_file("model.safetensors", 100, Some("prime")),
        flare_file("weights/x-0.safetensors", 50, Some("x")),
        flare_file("weights/x-1.safetensors", 50, Some("y")),
        flare_file("deployments/gb202-tp1/rank-0/plan.bin", 5, None),
        flare_file("config.json", 3, None),
    ];
    // `*` crosses `/` (fnmatch) — weights/** keeps every weights file.
    let files = select_flare_files(&set, "v1", &["weights/*".to_string()], &[]).unwrap();
    assert_eq!(
        files
            .iter()
            .map(|f| f.remote_path.clone())
            .collect::<Vec<_>>(),
        vec![
            Some("weights/x-0.safetensors".to_string()),
            Some("weights/x-1.safetensors".to_string())
        ]
    );
    // Exclude applies after include.
    let files = select_flare_files(
        &set,
        "v1",
        &["weights/*".to_string()],
        &["*x-1*".to_string()],
    )
    .unwrap();
    assert_eq!(
        files
            .iter()
            .map(|f| f.remote_path.clone())
            .collect::<Vec<_>>(),
        vec![Some("weights/x-0.safetensors".to_string())]
    );
    // A glob matching nothing in the set ⇒ zero-files-selected error
    // listing the unfiltered set (the plans/flare-models.md §5 acceptance smoke shape).
    let err = select_flare_files(&set, "v1", &["no-such-glob".to_string()], &[]).unwrap_err();
    match &err {
        ResolveError::NoFilesMatch {
            selector,
            available,
        } => {
            assert!(selector.contains("no-such-glob"), "{selector}");
            assert_eq!(
                available.len(),
                set.len(),
                "available lists the package set"
            );
            assert_eq!(available[0].filename, "v1/model.safetensors");
            assert_eq!(
                available[0].remote_path.as_deref(),
                Some("model.safetensors")
            );
        }
        other => panic!("expected NoFilesMatch, got {other:?}"),
    }
    assert_eq!(err.code(), "no_files_match");
}

fn refs_entry(name: &str, commit: &str) -> RefEntry {
    RefEntry {
        name: name.to_string(),
        target_commit: commit.to_string(),
    }
}

#[test]
fn preferred_revision_name_branch_wins_over_tag() {
    let refs = RepoRefs {
        branches: vec![
            refs_entry("main", &sha40(0)),
            refs_entry("gb202", &sha40(1)),
        ],
        tags: vec![refs_entry("v3", &sha40(1)), refs_entry("v2", &sha40(2))],
    };
    // The branch name is preferred for the shared commit; a tag-only
    // commit falls back to the tag name; main never wins; unknown → None.
    assert_eq!(
        preferred_revision_name(&refs, &sha40(1)),
        Some("gb202".to_string())
    );
    assert_eq!(
        preferred_revision_name(&refs, &sha40(2)),
        Some("v2".to_string())
    );
    assert_eq!(preferred_revision_name(&refs, &sha40(0)), None);
    assert_eq!(preferred_revision_name(&refs, &sha40(9)), None);
}

#[test]
fn payload_revision_names_list_branches_then_tags_excluding_main() {
    let refs = RepoRefs {
        branches: vec![
            refs_entry("main", &sha40(0)),
            refs_entry("gb202", &sha40(1)),
        ],
        tags: vec![refs_entry("v1", &sha40(2)), refs_entry("v3", &sha40(1))],
    };
    assert_eq!(
        payload_revision_names(&refs),
        vec!["gb202".to_string(), "v1".to_string(), "v3".to_string()]
    );
}

// --- FLARE pre-flight guard fixtures (plans/flare-models.md §3.3d/§4.3) --

use super::resolve::{flare_preflight, FlarePreflightError, FlarePreflightFile};
use crate::models::{DownloadMetadata, DownloadRegistry, DownloadStatus};
use std::path::PathBuf;

static GUARD_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn guard_root(tag: &str) -> PathBuf {
    let n = GUARD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rhd-flare-guard-{}-{}-{}",
        tag,
        std::process::id(),
        n
    ))
}

/// The lflare same-filename pair (Research facts): the SAME package
/// filename on different commits with different shas.
fn sha(n: u8) -> String {
    format!("{n:040x}")
}

fn preflight_file(filename: &str, remote_path: &str, size: u64) -> FlarePreflightFile {
    FlarePreflightFile {
        filename: filename.to_string(),
        remote_path: Some(remote_path.to_string()),
        expected_size: size,
    }
}

/// One recorded registry row.
fn recorded(
    model_id: &str,
    filename: &str,
    remote_path: Option<&str>,
    revision: &str, // the pin the URL was built at
    local_path: &str,
    recorded_commit: Option<&str>, // the registry `revision` field
    status: DownloadStatus,
) -> DownloadMetadata {
    DownloadMetadata {
        model_id: model_id.to_string(),
        filename: filename.to_string(),
        url: crate::api::resolve_url(model_id, remote_path.unwrap_or(filename), revision),
        local_path: local_path.to_string(),
        total_size: 0,
        downloaded_size: 0,
        status,
        expected_sha256: None,
        revision: recorded_commit.map(str::to_string),
        remote_path: remote_path.map(str::to_string),
    }
}

/// plans/flare-models.md §4.3 fixture: lflare `v1` then `v2` — the same package filename on
/// different commits ⇒ NO conflict: two complete files in two dirs (the
/// revision-dir split doing its job; only same-URL/differing-destination
/// or same-dir-moved-branch collide).
#[test]
fn preflight_v1_then_v2_two_dirs_is_clean() {
    let root = guard_root("v1v2");
    let model_root = root.join("a/b");
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "v1/qwen3.8-flash-next-tp1-c16-mtp5.safetensors",
            Some("qwen3.8-flash-next-tp1-c16-mtp5.safetensors"),
            &sha(1),
            &model_root
                .join("v1")
                .join("qwen3.8-flash-next-tp1-c16-mtp5.safetensors")
                .to_string_lossy(),
            Some(&sha(1)),
            DownloadStatus::Complete,
        )],
    };
    // The v2 run: same repo relpath, DIFFERENT commit ⇒ different URL, and
    // a different revision-dir prefix ⇒ neither moved-branch (ii) nor
    // single-destination (iii) applies.
    let outcome = flare_preflight(
        &registry,
        &root.to_string_lossy(),
        "a/b",
        &model_root,
        "v2",
        &sha(2),
        &[preflight_file(
            "v2/qwen3.8-flash-next-tp1-c16-mtp5.safetensors",
            "qwen3.8-flash-next-tp1-c16-mtp5.safetensors",
            107_011_338_717,
        )],
    );
    assert_eq!(outcome, Ok(()));
    let _ = std::fs::remove_dir_all(&root);
}

/// plans/flare-models.md §4.3 fixture: moved `gb202` — commit A recorded under this run's
/// revision dir, run targets B ⇒ hard error; DELETING THE DIRECTORY clears
/// it (on-disk existence is the guard's truth; stale entries for vanished
/// files never block).
#[test]
fn preflight_moved_branch_errors_then_delete_dir_retry_succeeds() {
    let root = guard_root("moved");
    let model_root = root.join("a/b");
    let recorded_path = model_root.join("gb202").join("pkg.safetensors");
    std::fs::create_dir_all(recorded_path.parent().unwrap()).unwrap();
    std::fs::write(&recorded_path, b"commit-A bytes").unwrap();
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "gb202/pkg.safetensors",
            Some("pkg.safetensors"),
            &sha(0xA),
            &recorded_path.to_string_lossy(),
            Some(&sha(0xA)),
            DownloadStatus::Complete,
        )],
    };
    let run = |registry: &DownloadRegistry| {
        flare_preflight(
            registry,
            &root.to_string_lossy(),
            "a/b",
            &model_root,
            "gb202",
            &sha(0xB),
            &[preflight_file(
                "gb202/pkg.safetensors",
                "pkg.safetensors",
                14,
            )], // == b"commit-A bytes".len()
        )
    };
    match run(&registry) {
        Err(FlarePreflightError::MovedBranch {
            recorded_commit,
            pin,
            directory,
        }) => {
            assert_eq!(recorded_commit.as_deref(), Some(sha(0xA).as_str()));
            assert_eq!(pin, sha(0xB));
            assert!(directory.ends_with("gb202"), "{directory}");
            let message = FlarePreflightError::MovedBranch {
                recorded_commit,
                pin,
                directory,
            }
            .to_string();
            assert!(message.contains("branch moved"), "{message}");
            assert!(message.contains("remove the directory"), "{message}");
        }
        other => panic!("expected MovedBranch, got {other:?}"),
    }

    // Retry after deleting the DIRECTORY: same registry entry, no file on
    // disk ⇒ the guard clears (the documented recovery).
    std::fs::remove_dir_all(model_root.join("gb202")).unwrap();
    assert_eq!(run(&registry), Ok(()));
    let _ = std::fs::remove_dir_all(&root);
}

/// plans/flare-models.md §4.3 fixture: sha-pinned run then branch-named run of the SAME commit ⇒
/// identical URLs (`<sha>/file` ≡ `gb202/file` on the wire) under two
/// destinations ⇒ guard (iii) hard error; deleting the ENTRY clears it.
#[test]
fn preflight_sha_then_branch_same_commit_conflicts_until_entry_deleted() {
    let root = guard_root("shabranch");
    let model_root = root.join("a/b");
    // Recorded by the sha-pinned run (dir segment = the sha).
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            &format!("{}/pkg.safetensors", sha(7)),
            Some("pkg.safetensors"),
            &sha(7),
            &model_root
                .join(sha(7))
                .join("pkg.safetensors")
                .to_string_lossy(),
            Some(&sha(7)),
            DownloadStatus::Complete,
        )],
    };
    // The branch-named run: same commit pin, gb202 dir ⇒ same URL, a
    // different FULL destination.
    let run = |registry: &DownloadRegistry| {
        flare_preflight(
            registry,
            &root.to_string_lossy(),
            "a/b",
            &model_root,
            "gb202",
            &sha(7),
            &[preflight_file(
                "gb202/pkg.safetensors",
                "pkg.safetensors",
                1,
            )],
        )
    };
    match run(&registry) {
        Err(err @ FlarePreflightError::DestinationConflict { .. }) => {
            let message = err.to_string();
            assert!(message.contains("already recorded at"), "{message}");
            assert!(message.contains("delete the registry entry"), "{message}");
            assert!(message.contains("registry TOML"), "{message}");
        }
        other => panic!("expected DestinationConflict, got {other:?}"),
    }

    // Deleting the ENTRY (the documented recovery — deleting the DIRECTORY
    // would NOT clear (iii)) lets the branch run proceed.
    assert_eq!(run(&DownloadRegistry::default()), Ok(()));
    let _ = std::fs::remove_dir_all(&root);
}

/// plans/flare-models.md §4.3 fixture: ORDINARY download (no prefix, no remote_path — including
/// the legacy shape) then a FLARE run of the same URL under a prefixed
/// destination ⇒ guard (iii); delete-entry-retry succeeds. No on-disk
/// carve-out: the ENTRY, not the file, is the conflict.
#[test]
fn preflight_ordinary_then_flare_same_url_conflicts_until_entry_deleted() {
    let root = guard_root("ordinary");
    let model_root = root.join("a/b");
    // An ordinary `download --revision <sha>` row: filename = repo path,
    // no remote_path, destination directly under the model root.
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "pkg.safetensors",
            None,
            &sha(3),
            &model_root.join("pkg.safetensors").to_string_lossy(),
            Some(&sha(3)),
            DownloadStatus::Complete,
        )],
    };
    let run = |registry: &DownloadRegistry| {
        flare_preflight(
            registry,
            &root.to_string_lossy(),
            "a/b",
            &model_root,
            "gb202",
            &sha(3),
            &[preflight_file(
                "gb202/pkg.safetensors",
                "pkg.safetensors",
                1,
            )],
        )
    };
    // The ordinary entry's file need not exist: (iii) has no on-disk
    // carve-out.
    assert!(matches!(
        run(&registry),
        Err(FlarePreflightError::DestinationConflict { .. })
    ));
    // Delete the entry ⇒ clean.
    assert_eq!(run(&DownloadRegistry::default()), Ok(()));
    let _ = std::fs::remove_dir_all(&root);
}

/// Guard (i): on-disk size ≠ expected tree size ⇒ hard error naming file,
/// expected vs actual bytes, and the directory; a matching size proceeds.
#[test]
fn preflight_size_mismatch_errors_and_equal_size_proceeds() {
    let root = guard_root("size");
    let model_root = root.join("a/b");
    let on_disk = model_root.join("gb202").join("pkg.safetensors");
    std::fs::create_dir_all(on_disk.parent().unwrap()).unwrap();
    std::fs::write(&on_disk, vec![0u8; 100]).unwrap();

    let file = preflight_file("gb202/pkg.safetensors", "pkg.safetensors", 200);
    match flare_preflight(
        &DownloadRegistry::default(),
        &root.to_string_lossy(),
        "a/b",
        &model_root,
        "gb202",
        &sha(1),
        &[file],
    ) {
        Err(FlarePreflightError::SizeMismatch {
            relpath,
            expected,
            actual,
            directory,
        }) => {
            assert_eq!(relpath, "pkg.safetensors");
            assert_eq!((expected, actual), (200, 100));
            assert!(directory.ends_with("gb202"), "{directory}");
        }
        other => panic!("expected SizeMismatch, got {other:?}"),
    }

    // Equal size ⇒ the idempotent re-run proceeds (engine's existing-file
    // skip is honest).
    let file = preflight_file("gb202/pkg.safetensors", "pkg.safetensors", 100);
    assert_eq!(
        flare_preflight(
            &DownloadRegistry::default(),
            &root.to_string_lossy(),
            "a/b",
            &model_root,
            "gb202",
            &sha(1),
            &[file],
        ),
        Ok(())
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Same-commit, same-recorded-destination re-runs proceed — and a moved
/// entry under a DIFFERENT revision dir never trips (ii) (scoped to THIS
/// run's dir).
#[test]
fn preflight_same_commit_same_destination_is_idempotent() {
    let root = guard_root("idem");
    let model_root = root.join("a/b");
    let recorded_path = model_root.join("gb202").join("pkg.safetensors");
    let registry = DownloadRegistry {
        downloads: vec![
            recorded(
                "a/b",
                "gb202/pkg.safetensors",
                Some("pkg.safetensors"),
                &sha(5),
                &recorded_path.to_string_lossy(),
                Some(&sha(5)),
                DownloadStatus::Complete,
            ),
            // A v1 entry under ITS dir (same model): not this run's prefix.
            recorded(
                "a/b",
                "v1/pkg.safetensors",
                Some("pkg.safetensors"),
                &sha(4),
                &model_root
                    .join("v1")
                    .join("pkg.safetensors")
                    .to_string_lossy(),
                Some(&sha(4)),
                DownloadStatus::Complete,
            ),
            // A different model entirely.
            recorded(
                "other/model",
                "gb202/pkg.safetensors",
                Some("pkg.safetensors"),
                &sha(9),
                "/elsewhere/gb202/pkg.safetensors",
                Some(&sha(9)),
                DownloadStatus::Complete,
            ),
        ],
    };
    assert_eq!(
        flare_preflight(
            &registry,
            &root.to_string_lossy(),
            "a/b",
            &model_root,
            "gb202",
            &sha(5),
            &[preflight_file(
                "gb202/pkg.safetensors",
                "pkg.safetensors",
                1
            )],
        ),
        Ok(()),
        "same commit + same recorded destination + foreign dirs are all clean"
    );
    let _ = std::fs::remove_dir_all(&root);
}
