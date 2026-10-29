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
    // The packed form is self-contained: even when the revision tree also
    // holds root metadata files, the assembly's packed package never
    // carries them (plans/flare-models.md §3.3c scopes
    // config.json/publisher.json to the directory form).
    assert_eq!(
        flare_package_set(&package),
        vec![flare_file(
            "qwen3.8-flash-next-gb10-tp1.safetensors",
            1,
            Some("cafe")
        )]
    );
}

#[test]
fn flare_package_set_directory_adds_root_metadata_files() {
    // The assembly's companions ALREADY carry the root metadata files for
    // the directory form (single source — see select_package).
    let primary = repo_file("model.safetensors", 100, Some("prime"));
    let companions = vec![
        repo_file("weights/x-0.safetensors", 50, Some("sha-x")),
        repo_file("deployments/gb202-tp1/rank-0/plan.bin", 5, None),
        repo_file("config.json", 3, None),
        repo_file("publisher.json", 4, None),
    ];
    let package = FlarePackage::Directory {
        primary: primary.clone(),
        companions: companions.clone(),
    };
    assert_eq!(
        flare_package_set(&package),
        vec![
            flare_file("model.safetensors", 100, Some("prime")),
            flare_file("weights/x-0.safetensors", 50, Some("sha-x")),
            flare_file("deployments/gb202-tp1/rank-0/plan.bin", 5, None),
            flare_file("config.json", 3, None),
            flare_file("publisher.json", 4, None),
        ],
        "directory set = primary + weights/** + deployments/** + config.json + publisher.json"
    );

    // A revision tree WITHOUT the root metadata files (LIL has no
    // config.json) simply yields companions without them.
    let package_without = FlarePackage::Directory {
        primary,
        companions: vec![repo_file("weights/x-0.safetensors", 50, Some("sha-x"))],
    };
    assert_eq!(
        flare_package_set(&package_without)
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
use std::path::{Path, PathBuf};

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

/// A GENUINELY RELATIVE spelling of `to` as seen from `from`, GUARANTEED
/// to carry `..` segments (impl-gate r2): strip the common prefix, walk
/// up one `..` per surplus `from` component, then down into `to` — and
/// when `to` sits inside `from` (no surplus components), hop out through
/// `from`'s parent and back in by name, a no-op that still spells `..`.
/// `validate_and_sanitize_path` absolutizes this against the cwd
/// WITHOUT normalizing the dots — exactly the spelling the pre-flight's
/// identity comparisons must survive (the old regression test's
/// `strip_prefix(cwd).unwrap_or(&root)` silently degraded to an
/// absolute base whenever tmp sat outside cwd, which is why it passed
/// against the raw string comparison).
fn relative_base_with_dots(from: &Path, to: &Path) -> String {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    let mut shared = 0;
    while shared < from_components.len()
        && shared < to_components.len()
        && from_components[shared] == to_components[shared]
    {
        shared += 1;
    }
    let mut rel = PathBuf::new();
    for _ in shared..from_components.len() {
        rel.push("..");
    }
    for component in &to_components[shared..] {
        rel.push(component.as_os_str());
    }
    match from.file_name() {
        Some(name) => format!("../{}/{}", name.to_string_lossy(), rel.display()),
        // `from` is a filesystem root — no name hop possible; the common
        // strip above already produced `..` segments in that case.
        None => rel.display().to_string(),
    }
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

/// Impl-gate r1 P1 + r2 P1 regression: a RELATIVE `--output` base —
/// spelled through `..` segments — must not bypass the moved-branch
/// guard. `requested_destination` absolutizes but does NOT normalize
/// `..` in the RETURNED path, so the old raw prefix comparison missed
/// the recorded absolute row for the SAME directory and the guard
/// silently passed; the comparison now goes through `fs_identity` on
/// both sides. The base here is GENUINELY relative AND dotted — the
/// old `strip_prefix(cwd).unwrap_or(&root)` fixture fell back to an
/// ABSOLUTE base whenever tmp sat outside cwd, which is why it never
/// exercised the bypass.
#[test]
fn preflight_relative_base_still_hits_moved_branch() {
    let root = guard_root("relbase");
    let model_root = root.join("a/b");
    // The on-disk bytes from commit A exist under the gb202 dir.
    std::fs::create_dir_all(model_root.join("gb202")).unwrap();
    std::fs::write(model_root.join("gb202").join("pkg.safetensors"), b"x").unwrap();
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "gb202/pkg.safetensors",
            Some("pkg.safetensors"),
            &sha(1),
            &model_root
                .join("gb202")
                .join("pkg.safetensors")
                .to_string_lossy(),
            Some(&sha(1)),
            DownloadStatus::Complete,
        )],
    };
    // The rerun at commit B passes a base that is genuinely relative,
    // carries `..` segments, and resolves to the same root — the guard
    // must still fire (the raw dotted prefix would never string-match
    // the recorded absolute row).
    let cwd = std::env::current_dir().unwrap();
    let relative_root = relative_base_with_dots(&cwd, &root);
    assert!(
        Path::new(&relative_root).is_relative() && relative_root.contains(".."),
        "fixture must be a relative, `..`-spelled base: {relative_root:?}"
    );
    let outcome = flare_preflight(
        &registry,
        &relative_root,
        "a/b",
        "gb202",
        &sha(2),
        &[preflight_file(
            "gb202/pkg.safetensors",
            "pkg.safetensors",
            1,
        )],
    );
    assert!(
        matches!(outcome, Err(FlarePreflightError::MovedBranch { .. })),
        "relative dotted base must not bypass the moved-branch guard: {outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Impl-gate r2 P1 regression (guard iii): the same URL recorded at a
/// DIFFERENT real destination must still conflict when the requested
/// base is spelled through `..` — the identity comparison accepts the
/// SAME destination differently spelled and nothing more. Neither file
/// exists (only the revision dirs do), so neither `path_matches` branch
/// can decide: only the nearest-existing-ancestor identities can, and
/// the two real roots differ.
#[test]
fn preflight_dotted_base_destination_conflict_still_fires() {
    let root = guard_root("reldots");
    let recorded_root = guard_root("reldots-rec");
    let model_root = root.join("a/b");
    let recorded_model_root = recorded_root.join("a/b");
    std::fs::create_dir_all(model_root.join("gb202")).unwrap();
    std::fs::create_dir_all(recorded_model_root.join("gb202")).unwrap();
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "gb202/pkg.safetensors",
            Some("pkg.safetensors"),
            &sha(3),
            &recorded_model_root
                .join("gb202")
                .join("pkg.safetensors")
                .to_string_lossy(),
            Some(&sha(3)),
            DownloadStatus::Complete,
        )],
    };
    let cwd = std::env::current_dir().unwrap();
    let relative_root = relative_base_with_dots(&cwd, &root);
    let outcome = flare_preflight(
        &registry,
        &relative_root,
        "a/b",
        "gb202",
        &sha(3),
        &[preflight_file(
            "gb202/pkg.safetensors",
            "pkg.safetensors",
            1,
        )],
    );
    match outcome {
        Err(FlarePreflightError::DestinationConflict { recorded, .. }) => {
            assert!(recorded.contains("reldots-rec"), "{recorded}");
        }
        other => panic!(
            "same-url different-destination must still conflict under a \
             dotted base: {other:?}"
        ),
    }
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&recorded_root);
}

/// The identity comparison's POSITIVE case (impl-gate r2): the same URL
/// recorded at the SAME real destination, requested through a `..`-
/// spelled base, with the file deleted — full canonicalize fails on both
/// sides, so the old raw comparison raised a FALSE DestinationConflict
/// against the run's own row; the nearest-existing-ancestor identities
/// prove same-destination and the idempotent re-run proceeds.
#[test]
fn preflight_dotted_base_same_destination_is_not_a_false_conflict() {
    let root = guard_root("relsame");
    let model_root = root.join("a/b");
    std::fs::create_dir_all(model_root.join("gb202")).unwrap();
    // The recorded file is GONE (deleted after the earlier run) — only
    // its gb202 dir remains to anchor the identity comparison.
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "gb202/pkg.safetensors",
            Some("pkg.safetensors"),
            &sha(3),
            &model_root
                .join("gb202")
                .join("pkg.safetensors")
                .to_string_lossy(),
            Some(&sha(3)),
            DownloadStatus::Complete,
        )],
    };
    let cwd = std::env::current_dir().unwrap();
    let relative_root = relative_base_with_dots(&cwd, &root);
    let outcome = flare_preflight(
        &registry,
        &relative_root,
        "a/b",
        "gb202",
        &sha(3),
        &[preflight_file(
            "gb202/pkg.safetensors",
            "pkg.safetensors",
            1,
        )],
    );
    assert_eq!(
        outcome,
        Ok(()),
        "same real destination differently spelled is an idempotent re-run"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Impl-gate r3 trace (1) — A → delete-dir → B → rerun-B PASSES: A's
/// row is still on file but its FILE is gone (vanished rows never
/// vote), and B's file exists, so the EXISTENCE-BASED owner set is
/// exactly {B} = {pin} — the idempotent re-run arm. The registry Vec
/// order carries no evidence (r3 P1: `upsert_pending` preserves URLs
/// in place, so the lifecycle leaves A's row FIRST; whether the owner
/// row is first or last is irrelevant — only on-disk existence votes).
/// The two commits ship DIFFERENT artifact names in the same revision
/// dir (a re-sharded package), so each row owns a distinct path.
#[test]
fn preflight_superseded_row_does_not_block_current_pin_rerun() {
    let root = guard_root("supersede");
    let revision_dir = root.join("a/b/gb202");
    std::fs::create_dir_all(&revision_dir).unwrap();
    // B's bytes exist on disk (recreated after the delete); A's file is
    // GONE with the deleted directory — only its registry row remains.
    std::fs::write(revision_dir.join("pkg-b.safetensors"), b"x").unwrap();
    let registry = DownloadRegistry {
        downloads: vec![
            // A's remnant row, FIRST (registered before B).
            recorded(
                "a/b",
                "gb202/pkg-a.safetensors",
                Some("pkg-a.safetensors"),
                &sha(1),
                &revision_dir.join("pkg-a.safetensors").to_string_lossy(),
                Some(&sha(1)),
                DownloadStatus::Complete,
            ),
            // B's row, SECOND — its file exists.
            recorded(
                "a/b",
                "gb202/pkg-b.safetensors",
                Some("pkg-b.safetensors"),
                &sha(2),
                &revision_dir.join("pkg-b.safetensors").to_string_lossy(),
                Some(&sha(2)),
                DownloadStatus::Complete,
            ),
        ],
    };
    let outcome = flare_preflight(
        &registry,
        &root.to_string_lossy(),
        "a/b",
        "gb202",
        &sha(2),
        &[preflight_file(
            "gb202/pkg-b.safetensors",
            "pkg-b.safetensors",
            1,
        )],
    );
    assert_eq!(
        outcome,
        Ok(()),
        "owner set is {{B}} (A's file vanished) — the B re-run is idempotent"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Impl-gate r3 trace (2) — C → delete-dir → B → rerun-C ERRORS naming
/// B: C's row is on file but its file is GONE (vanished rows never
/// vote); B's file exists, so the owner set is {B} ≠ {C} — moved branch
/// naming B, the commit whose bytes are actually on disk. A historical
/// row for the CURRENT pin cannot supersede the bytes that exist.
#[test]
fn preflight_historical_current_pin_row_cannot_supersede_newer_owner() {
    let root = guard_root("lww");
    let revision_dir = root.join("a/b/gb202");
    std::fs::create_dir_all(&revision_dir).unwrap();
    // B's bytes are on disk (recreated after the delete); C's file is
    // gone. Registry Vec order [C, B] is exactly what the lifecycle
    // produces (C registered first; re-registering C preserves its row
    // in place), and is deliberately NOT what ownership follows.
    std::fs::write(revision_dir.join("pkg-b.safetensors"), b"x").unwrap();
    let registry = DownloadRegistry {
        downloads: vec![
            recorded(
                "a/b",
                "gb202/pkg-c.safetensors",
                Some("pkg-c.safetensors"),
                &sha(3),
                &revision_dir.join("pkg-c.safetensors").to_string_lossy(),
                Some(&sha(3)),
                DownloadStatus::Complete,
            ),
            recorded(
                "a/b",
                "gb202/pkg-b.safetensors",
                Some("pkg-b.safetensors"),
                &sha(2),
                &revision_dir.join("pkg-b.safetensors").to_string_lossy(),
                Some(&sha(2)),
                DownloadStatus::Complete,
            ),
        ],
    };
    match flare_preflight(
        &registry,
        &root.to_string_lossy(),
        "a/b",
        "gb202",
        &sha(3),
        &[preflight_file(
            "gb202/pkg-c.safetensors",
            "pkg-c.safetensors",
            1,
        )],
    ) {
        Err(FlarePreflightError::MovedBranch {
            recorded_commit,
            pin,
            ..
        }) => {
            // The error names B — the commit whose bytes EXIST — not C's
            // own vanished row.
            assert_eq!(recorded_commit.as_deref(), Some(sha(2).as_str()));
            assert_eq!(pin, sha(3));
        }
        other => panic!(
            "a historical row for the current pin must not supersede the \
             existing owner's bytes: {other:?}"
        ),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Impl-gate r3 trace (3), codex's round-3 abuse trace —
/// C → delete → B → delete → C, then rerun. `upsert_pending` preserves
/// already-recorded URLs in place, so the registry Vec order stays
/// [C, B] even though C's bytes were re-downloaded LAST: rerun-B must
/// ERROR naming C (C's file exists, B's is gone — C owns the directory;
/// the r2 LAST-ROW-WINS predicate crowned B, the last ROW, and let a B
/// re-run silently reuse C's bytes), and rerun-C must PASS (owner set
/// == {C} == pin — the idempotent re-run).
#[test]
fn preflight_repopulation_c_b_c_rerun_b_errors_naming_c_and_rerun_c_passes() {
    let root = guard_root("cbc");
    let revision_dir = root.join("a/b/gb202");
    std::fs::create_dir_all(&revision_dir).unwrap();
    // After the full lifecycle only C's file exists: the second delete
    // removed B's, then C's re-download re-populated its own.
    std::fs::write(revision_dir.join("pkg-c.safetensors"), b"x").unwrap();
    let registry = DownloadRegistry {
        downloads: vec![
            // C's row, FIRST — re-registering C after the second delete
            // PRESERVED it in place (upsert_pending keeps existing URLs).
            recorded(
                "a/b",
                "gb202/pkg-c.safetensors",
                Some("pkg-c.safetensors"),
                &sha(3),
                &revision_dir.join("pkg-c.safetensors").to_string_lossy(),
                Some(&sha(3)),
                DownloadStatus::Complete,
            ),
            // B's row, SECOND (appended when B ran); its file is GONE.
            recorded(
                "a/b",
                "gb202/pkg-b.safetensors",
                Some("pkg-b.safetensors"),
                &sha(2),
                &revision_dir.join("pkg-b.safetensors").to_string_lossy(),
                Some(&sha(2)),
                DownloadStatus::Complete,
            ),
        ],
    };
    // rerun-B: owner set {C} ≠ {B} — moved branch NAMING C.
    match flare_preflight(
        &registry,
        &root.to_string_lossy(),
        "a/b",
        "gb202",
        &sha(2),
        &[preflight_file(
            "gb202/pkg-b.safetensors",
            "pkg-b.safetensors",
            1,
        )],
    ) {
        Err(FlarePreflightError::MovedBranch {
            recorded_commit,
            pin,
            ..
        }) => {
            assert_eq!(recorded_commit.as_deref(), Some(sha(3).as_str()));
            assert_eq!(pin, sha(2));
        }
        other => panic!("rerun-B over C's repopulated bytes must error naming C: {other:?}"),
    }
    // rerun-C: owner set {C} == pin — the idempotent re-run passes.
    assert_eq!(
        flare_preflight(
            &registry,
            &root.to_string_lossy(),
            "a/b",
            "gb202",
            &sha(3),
            &[preflight_file(
                "gb202/pkg-c.safetensors",
                "pkg-c.safetensors",
                1,
            )],
        ),
        Ok(()),
        "rerun-C owns the directory (owner set == {{pin}}) — idempotent"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Impl-gate r3 trace (5) — same-commit rerun with the commit's OWN
/// file on disk: owner set == {pin} ⇒ passes (the {pin} arm of the
/// existence-based predicate; it equally supersedes any vanished stale
/// rows of other commits recorded under the same dir).
#[test]
fn preflight_same_commit_existing_file_rerun_is_idempotent() {
    let root = guard_root("samecommit");
    let revision_dir = root.join("a/b/gb202");
    std::fs::create_dir_all(&revision_dir).unwrap();
    std::fs::write(revision_dir.join("pkg.safetensors"), b"x").unwrap();
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "gb202/pkg.safetensors",
            Some("pkg.safetensors"),
            &sha(7),
            &revision_dir.join("pkg.safetensors").to_string_lossy(),
            Some(&sha(7)),
            DownloadStatus::Complete,
        )],
    };
    assert_eq!(
        flare_preflight(
            &registry,
            &root.to_string_lossy(),
            "a/b",
            "gb202",
            &sha(7),
            &[preflight_file(
                "gb202/pkg.safetensors",
                "pkg.safetensors",
                1
            )],
        ),
        Ok(()),
        "owner set == {{pin}} with the file on disk — idempotent re-run"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Impl-gate r3 trace (6) — AMBIGUOUS: the revision dir holds EXISTING
/// files from TWO distinct commits at once (a partial delete leaves
/// exactly this — one commit's file survived, the other's returned).
/// The owner set has >1 distinct pin, so no single owner can be named:
/// the error must say the directory holds files from multiple commits
/// and direct the user to delete the directory OR the registry entries.
#[test]
fn preflight_ambiguous_existing_files_from_two_commits_errors() {
    let root = guard_root("ambiguous");
    let revision_dir = root.join("a/b/gb202");
    std::fs::create_dir_all(&revision_dir).unwrap();
    // BOTH commits' files exist on disk (partial delete + partial
    // re-download).
    std::fs::write(revision_dir.join("pkg-a.safetensors"), b"x").unwrap();
    std::fs::write(revision_dir.join("pkg-b.safetensors"), b"x").unwrap();
    let registry = DownloadRegistry {
        downloads: vec![
            recorded(
                "a/b",
                "gb202/pkg-a.safetensors",
                Some("pkg-a.safetensors"),
                &sha(1),
                &revision_dir.join("pkg-a.safetensors").to_string_lossy(),
                Some(&sha(1)),
                DownloadStatus::Complete,
            ),
            recorded(
                "a/b",
                "gb202/pkg-b.safetensors",
                Some("pkg-b.safetensors"),
                &sha(2),
                &revision_dir.join("pkg-b.safetensors").to_string_lossy(),
                Some(&sha(2)),
                DownloadStatus::Complete,
            ),
        ],
    };
    // A B re-run hits the ambiguity even though B's own file exists:
    // A's does too, and no Vec order can say whose bytes rule.
    match flare_preflight(
        &registry,
        &root.to_string_lossy(),
        "a/b",
        "gb202",
        &sha(2),
        &[preflight_file(
            "gb202/pkg-b.safetensors",
            "pkg-b.safetensors",
            1,
        )],
    ) {
        Err(
            ref err @ FlarePreflightError::AmbiguousOwner {
                ref owners,
                ref pin,
                ..
            },
        ) => {
            assert_eq!(pin.as_str(), sha(2).as_str());
            assert!(owners.contains(&sha(1)), "owners: {owners:?}");
            assert!(owners.contains(&sha(2)), "owners: {owners:?}");
            let message = err.to_string();
            assert!(message.contains("branch moved"), "{message}");
            assert!(message.contains("multiple commits"), "{message}");
            assert!(
                message.contains("delete the directory or the registry entries"),
                "{message}"
            );
        }
        other => panic!(
            "two commits' existing files under one dir must error as \
             ambiguous, not pick an owner: {other:?}"
        ),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Impl-gate r3 P1 regression (codex's symlink trace): a base spelled
/// `<root>/missing/../alias` — where `missing` does NOT exist and
/// `alias` is an EXISTING symlink to `dirA` — must NOT bypass the
/// moved-branch guard. The single-pass helper anchored at `<root>`,
/// lexically dropped `missing/..`, and appended `alias` UNRESOLVED, so
/// the dir identity (`<root>/alias/…`) never matched the row recorded
/// at the CANONICAL path (`<root>/dirA/…`); guard (i) cannot stat the
/// dotted spelling (`missing` is absent) and the different commit means
/// a fresh URL, so ONLY guard (ii)'s identity comparison can catch the
/// collision — the fixpoint re-anchors on the lexically normalized
/// path, resolves `alias`, and fires. Mirrors the dotted-base test
/// style above (impl-gate r2).
#[cfg(unix)]
#[test]
fn preflight_missing_dotdot_then_symlink_base_still_hits_moved_branch() {
    let root = guard_root("symfix");
    // dirA holds A's bytes under the revision dir; `alias` → dirA is an
    // EXISTING symlink; NOTHING named `missing` is created.
    let dir_a = root.join("dirA");
    let file = dir_a.join("a/b/gb202/pkg.safetensors");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"x").unwrap();
    std::os::unix::fs::symlink(&dir_a, root.join("alias")).unwrap();
    // The recorded row uses the CANONICAL path (dirA/…) — what a real
    // earlier run at commit sha(1) would have registered.
    let registry = DownloadRegistry {
        downloads: vec![recorded(
            "a/b",
            "gb202/pkg.safetensors",
            Some("pkg.safetensors"),
            &sha(1),
            &file.to_string_lossy(),
            Some(&sha(1)),
            DownloadStatus::Complete,
        )],
    };
    // The rerun at commit sha(2) spells the base through
    // `missing/../alias`: equal size, so only the identity comparison
    // separates the runs.
    let base = format!("{}/missing/../alias", root.display());
    let outcome = flare_preflight(
        &registry,
        &base,
        "a/b",
        "gb202",
        &sha(2),
        &[preflight_file(
            "gb202/pkg.safetensors",
            "pkg.safetensors",
            1,
        )],
    );
    assert!(
        matches!(outcome, Err(FlarePreflightError::MovedBranch { .. })),
        "a base spelled through `missing/../alias` (existing symlink after \
         lexical `..`) must not bypass the moved-branch guard: {outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
