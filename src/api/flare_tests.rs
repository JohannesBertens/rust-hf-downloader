//! Tests for [`crate::api::flare`] — plan `plans/flare-models.md` §3.1/§4.1.
//!
//! Fixtures are INLINE `const` JSON recorded live (2026-10) from the three
//! real FLARE repos — the repo convention is code-carried fixtures, no
//! on-disk fixture dir:
//!
//! - `local-inference-lab/Qwen3.8-Flash-Next-FLARE` (`main`/`gb202`/`gb10`;
//!   packed packages on both payload branches; NO `config.json` at main)
//! - `lflare/Qwen3.8-Flash-Next-FLARE-Abliterated` (`main`/`gb202` + tags
//!   `v1`/`v2`/`v3`; `gb202` ≡ `v3` is the DIRECTORY form; `v1`/`v2` are
//!   PACKED with the SAME filename but different sha256 — the cross-revision
//!   collision fixture)
//! - `JohannesBertens/Qwen3.8-Flash-Next-FLARE-1M-TP2` (`main`/`gb202`;
//!   packed `…-yarn4-tp2.safetensors`; `model_type` + `flare`-object config
//!   markers, no `flare` tag)
//!
//! Tree fixtures carry the raw `/tree/{sha}?recursive=true` entries
//! (including `type: "directory"` rows) and are converted through the same
//! `ModelFile → RepoFile` mapping `fetch_revision_tree` applies.

use super::*;
use crate::api::flare::select_package;
use crate::models::{
    FlareConfigFacts, FlarePackage, ModelFile, ModelMetadata, ParsedPackageTokens, RefKind,
    RepoFile, RepoRefs,
};
use std::collections::HashMap;

// ---- fixture helpers ----

/// Convert a recorded recursive-tree payload into the `RepoFile` listing
/// `fetch_revision_tree` yields: directory rows dropped, `path` renamed to
/// `rfilename`, `size` widened to `Option`.
fn repo_files(recorded_tree: &str) -> Vec<RepoFile> {
    let items: Vec<ModelFile> = serde_json::from_str(recorded_tree).unwrap();
    items
        .into_iter()
        .filter(|f| f.file_type != "directory")
        .map(|f| RepoFile {
            rfilename: f.path,
            size: Some(f.size),
            oid: f.oid,
            lfs: f.lfs,
        })
        .collect()
}

fn parse_refs(recorded_refs: &str) -> RepoRefs {
    serde_json::from_str(recorded_refs).unwrap()
}

fn trees(pairs: &[(&str, &str)]) -> HashMap<String, Vec<RepoFile>> {
    pairs
        .iter()
        .map(|(sha, tree)| (sha.to_string(), repo_files(tree)))
        .collect()
}

fn metadata(tags: &[&str], sibling_paths: &[&str]) -> ModelMetadata {
    ModelMetadata {
        model_id: "a/b".to_string(),
        library_name: None,
        pipeline_tag: None,
        card_data: None,
        siblings: sibling_paths
            .iter()
            .map(|p| RepoFile {
                rfilename: p.to_string(),
                size: None,
                oid: None,
                lfs: None,
            })
            .collect(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        sha: None,
    }
}

// ---- recorded fixtures: local-inference-lab/Qwen3.8-Flash-Next-FLARE ----

const LIL_REFS: &str = r#"{"branches":[
    {"name":"main","ref":"refs/heads/main","targetCommit":"5fcda54829558faf35528c9e223e41c79c3e7feb"},
    {"name":"gb202","ref":"refs/heads/gb202","targetCommit":"431b302155d53e296aa51633a997b4b5fd92e751"},
    {"name":"gb10","ref":"refs/heads/gb10","targetCommit":"cd5b82260c6922d629b0d97d66d16bc4658ed2c2"}
],"tags":[],"converts":[]}"#;

const LIL_COMMIT_MAIN: &str = "5fcda54829558faf35528c9e223e41c79c3e7feb";
const LIL_COMMIT_GB202: &str = "431b302155d53e296aa51633a997b4b5fd92e751";
const LIL_COMMIT_GB10: &str = "cd5b82260c6922d629b0d97d66d16bc4658ed2c2";

const LIL_TREE_GB202: &str = r#"[
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"d2d3f274ec556377b13a56c240dbe6a098bee3b7","size":107011338717,
     "lfs":{"oid":"c3db5e1ac63b05efa67d6525826a8a792c0ce92b810e91ddd5ec0f42c73a175c","size":107011338717,"pointerSize":137},
     "path":"qwen3.8-flash-next-tp1-c16-mtp5.safetensors"}
]"#;

const LIL_TREE_GB10: &str = r#"[
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"cad649a2e6e0aa8eb0f061202f74c64888b98ccf","size":5898,"path":"README.md"},
    {"type":"file","oid":"99d5dfb1160c1bdc7502c15c4008506d60174c56","size":106886249924,
     "lfs":{"oid":"cf574ea402abe2f40eba0db484578cfd25093d134fdd44bf086058a973bc71c2","size":106886249924,"pointerSize":137},
     "path":"qwen3.8-flash-next-gb10-tp1-c8-i4-mtp5.safetensors"}
]"#;

/// LIL main: `publisher.json` present, `config.json` ABSENT (live 404 —
/// the fetchers and the parsers must both tolerate it).
const LIL_TREE_MAIN: &str = r#"[
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"8a091bdb6aacbfcf560bc67e8f729b341e5ac4f5","size":6054,"path":"README.md"},
    {"type":"file","oid":"c0a661e991f34d06dfaca693428c429e289ce60c","size":480,"path":"publisher.json"}
]"#;

const LIL_PUBLISHER: &str = r#"[{"name":"Local Inference Lab, Inc.","namespace":"local-inference-lab",
 "public_key":[255,40,205,49,22,58,37,174,143,52,11,7,48,227,103,28,37,66,249,163,14,192,27,219,237,234,121,58,151,247,195,125],
 "development":false}]"#;

/// sha256 over the raw `public_key` byte array (independently computed).
const LIL_FINGERPRINT: &str = "937c0143f6856d3ee8ad164f62e3d804bd825d7a8750d0accb9a5849f703a8c1";

const LIL_TAGS: &[&str] = &[
    "flare",
    "nvfp4",
    "mxfp8",
    "image-text-to-text",
    "base_model:local-inference-lab/Qwen3.8-Flash-Next-NVFP4-MXFP8-CSF-QAD",
    "license:other",
    "region:us",
];

// ---- recorded fixtures: lflare/Qwen3.8-Flash-Next-FLARE-Abliterated ----

/// Branch `gb202` and tag `v3` share `targetCommit` — the alias-dedup case.
const LFLARE_REFS: &str = r#"{"tags":[
    {"name":"v2","ref":"refs/tags/v2","targetCommit":"180c07565f568904f6f3a8b28607bd8c2d76d9d7"},
    {"name":"v3","ref":"refs/tags/v3","targetCommit":"a98704bcce2442608c3142b5c3e093ecce15caa5"},
    {"name":"v1","ref":"refs/tags/v1","targetCommit":"c158260ecaa6ffd39839638a43394ea04dd10d8f"}
],"branches":[
    {"name":"gb202","ref":"refs/heads/gb202","targetCommit":"a98704bcce2442608c3142b5c3e093ecce15caa5"},
    {"name":"main","ref":"refs/heads/main","targetCommit":"da3a8ef9e797450a81718e7eeea5a71c69acdc5f"}
],"converts":[]}"#;

const LFLARE_COMMIT_MAIN: &str = "da3a8ef9e797450a81718e7eeea5a71c69acdc5f";
const LFLARE_COMMIT_GB202: &str = "a98704bcce2442608c3142b5c3e093ecce15caa5";
const LFLARE_COMMIT_V1: &str = "c158260ecaa6ffd39839638a43394ea04dd10d8f";
const LFLARE_COMMIT_V2: &str = "180c07565f568904f6f3a8b28607bd8c2d76d9d7";

/// DIRECTORY form (representative subset of the recorded 63-entry tree):
/// root `model.safetensors` + `weights/**` + `deployments/**`, plus the
/// two `type: "directory"` rows the live tree carries (they must be
/// filtered exactly like `fetch_revision_tree` does).
const LFLARE_TREE_GB202: &str = r#"[
    {"type":"directory","oid":"1ae201157325d8d1987b19e63d06cebf3ff87f88","size":0,"path":"deployments"},
    {"type":"directory","oid":"7d4a20f0d9e29f498e9a58cf88a5a0b6a6b6a6a4","size":0,"path":"weights"},
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"3d1d3bae48fe4b72adea140c4ac7e81e4de1abff","size":1902,"path":"config.json"},
    {"type":"file","oid":"cc10a17b071dc478711f6f046a1c301acc15d25d","size":22922000,
     "lfs":{"oid":"dbf20a21e4ad13648446f0d3bddf97a46f568eaa5caf954ccb4390846cbfdc38","size":22922000,"pointerSize":133},
     "path":"model.safetensors"},
    {"type":"file","oid":"b302911cd35ac40162fdb3f1c4c5bbf5ccaa0fbb","size":482,"path":"publisher.json"},
    {"type":"file","oid":"ede36a64faa6720bb27ea9d1a1053b05055b2b23","size":25600126976,
     "lfs":{"oid":"b654abfd3b048e41a743f6ad475fa6368940e8c12bbe31fd43f53e5e5ce3aacf","size":25600126976,"pointerSize":136},
     "path":"weights/engram-0-weights.safetensors"},
    {"type":"file","oid":"6b4353d55271ed2cbf73a2bd4e456942c4971fd8","size":1511521216,
     "lfs":{"oid":"e690dade27c972d5d445bbbb1538be7ef3d675cf41980ebad4de6a245fa9bbf4","size":1511521216,"pointerSize":135},
     "path":"weights/layer-000.safetensors"},
    {"type":"file","oid":"fc832c717181eb6da1c2743b0c885e603d7c8f7d","size":4842314948,
     "lfs":{"oid":"979c096cbbe20da874d3ba582615cfa22c085cc349f75ff0dbd1a4b9aae1a19d","size":4842314948,"pointerSize":135},
     "path":"weights/misc.safetensors"},
    {"type":"file","oid":"d5c39388031078b22849fed88e39633cfee7637f","size":2409784,
     "lfs":{"oid":"ecd283c23aae2f29bc3f46f041ae98ae93ebfa82e83defec5b09927840f1adb9","size":2409784,"pointerSize":132},
     "path":"deployments/gb202-tp1/rank-0/graphs.safetensors"}
]"#;

/// PACKED, same filename as LIL `gb202`'s package — different sha256
/// (`fff19620…`): the cross-revision collision fixture, half one.
const LFLARE_TREE_V1: &str = r#"[
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"5e793c07964ae21b6124a2bd48546ec23bdb61c4","size":107011338717,
     "lfs":{"oid":"fff1962065e4b09858bf700646336243ab18faf9900dad9c4f8486345032ef3d","size":107011338717,"pointerSize":137},
     "path":"qwen3.8-flash-next-tp1-c16-mtp5.safetensors"}
]"#;

/// PACKED, same filename as `v1` — different sha256 (`35112671…`): the
/// collision fixture, half two.
const LFLARE_TREE_V2: &str = r#"[
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"3d1d3bae48fe4b72adea140c4ac7e81e4de1abff","size":1902,"path":"config.json"},
    {"type":"file","oid":"ebc5e643d895637257875d76460944763b6b6912","size":107011338717,
     "lfs":{"oid":"35112671d85c719222c40d527d88f475cc38cbae6f20d1b94d411083ce0ac7f4","size":107011338717,"pointerSize":137},
     "path":"qwen3.8-flash-next-tp1-c16-mtp5.safetensors"}
]"#;

const LFLARE_TREE_MAIN: &str = r#"[
    {"type":"directory","oid":"1ae201157325d8d1987b19e63d06cebf3ff87f88","size":0,"path":"LICENSES"},
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"b3fc4cf47d40890bc02a409b098cf080543ce0c8","size":31799,"path":"LICENSE"},
    {"type":"file","oid":"c2bdd54832fff6daf1efdfc27aaa139d752a9dac","size":2346,"path":"NOTICE"},
    {"type":"file","oid":"c18e7bd4f74ec91267760b23b70f6b74fb72b2b9","size":13906,"path":"README.md"},
    {"type":"file","oid":"3d1d3bae48fe4b72adea140c4ac7e81e4de1abff","size":1902,"path":"config.json"},
    {"type":"file","oid":"c0a661e991f34d06dfaca693428c429e289ce60c","size":480,"path":"publisher.json"}
]"#;

/// Recorded verbatim (the `"format"`/`"runtime"` markers are the load-bearing
/// part for the Probable→confirmed detection path).
const LFLARE_CONFIG: &str = r#"{
  "_what_this_is": "A download-stats anchor, not a loadable model config. This repository is a Flare runtime package and cannot be loaded by Transformers, vLLM, SGLang or llama.cpp. The Hub counts a download when a request is served for one of its query files, and with no library declared that file is config.json (see https://huggingface.co/docs/hub/models-download-stats). Without this file the repository records zero downloads no matter how many times the 107 GB package is pulled.",
  "_deliberately_absent": "No model_type, architectures or auto_map key. Those would invite tools to treat this as a Transformers checkpoint and the Hub to infer an architecture, which would be wrong. AutoConfig.from_pretrained is expected to fail here, and that is the honest outcome.",
  "format": "flare",
  "runtime": "flare",
  "package_file": "qwen3.8-flash-next-tp1-c16-mtp5.safetensors",
  "package_revision": "gb202",
  "package_size_bytes": 107011338717,
  "package_sha256": "35112671d85c719222c40d527d88f475cc38cbae6f20d1b94d411083ce0ac7f4",
  "version": "v2",
  "edit": {
    "kind": "refusal-direction ablation, applied in place on the quantised grid",
    "rank": 2,
    "layer": 42,
    "biprojected": true,
    "alpha": 1.0,
    "direction_cos": 0.3495,
    "tensors_patched": 24676,
    "tensors_not_patched": "512 MTP routed-expert down_proj planes, whose storage region is not addressable"
  },
  "parent": {
    "repo": "local-inference-lab/Qwen3.8-Flash-Next-FLARE",
    "revision": "gb202",
    "package_sha256": "c3db5e1ac63b05efa67d6525826a8a792c0ce92b810e91ddd5ec0f42c73a175c"
  },
  "weights_also_published_as": "lflare/Qwen3.8-Flash-Next-NVFP4-MXFP8-CSF-QAD-Abliterated",
  "requirements": {
    "runtime": "flare",
    "cuda_min": "13.2",
    "gpus": 1,
    "tensor_parallel_size": 1,
    "max_concurrency": 16,
    "mtp_draft_tokens": 5,
    "verify_weights_supported": false
  }
}"#;

const LFLARE_TAGS: &[&str] = &[
    "flare",
    "nvfp4",
    "mxfp8",
    "abliterated",
    "reduced-refusal",
    "image-text-to-text",
    "base_model:lflare/Qwen3.8-Flash-Next-NVFP4-MXFP8-CSF-QAD-Abliterated",
    "license:other",
    "region:us",
];

// ---- recorded fixtures: JohannesBertens/Qwen3.8-Flash-Next-FLARE-1M-TP2 ----

const JB_REFS: &str = r#"{"branches":[
    {"name":"gb202","ref":"refs/heads/gb202","targetCommit":"71961bdd6331f61f92664feaffd69a456265a085"},
    {"name":"main","ref":"refs/heads/main","targetCommit":"f6db7b969015175d4a3f68f7d363d90545ff8442"}
],"tags":[],"converts":[]}"#;

const JB_COMMIT_MAIN: &str = "f6db7b969015175d4a3f68f7d363d90545ff8442";
const JB_COMMIT_GB202: &str = "71961bdd6331f61f92664feaffd69a456265a085";

const JB_TREE_GB202: &str = r#"[
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"a1afbfd716c97a7ea856bff7160607e9fed9a2f5","size":382,"path":"LICENSE.md"},
    {"type":"file","oid":"fea5d92d7a628ec0594ceaa1968dc477546f92e1","size":8822,"path":"README.md"},
    {"type":"file","oid":"d2c622a663fca5cd86268c2df2f699fc2045bbdd","size":262,"path":"config.json"},
    {"type":"file","oid":"2f244c4a15ebb722254ddcb95b4163e8e3055326","size":467,"path":"publisher.json"},
    {"type":"file","oid":"ee3a92043cad1bf788cc0e37b422b3660b78946a","size":111434386432,
     "lfs":{"oid":"a08b6314debdeff6936723f03faf6a558b771f6ff9ea94fc25380165439188f8","size":111434386432,"pointerSize":137},
     "path":"qwen3.8-flash-next-yarn4-tp2.safetensors"}
]"#;

const JB_TREE_MAIN: &str = r#"[
    {"type":"file","oid":"a6344aac8c09253b3b630fb776ae94478aa0275b","size":1519,"path":".gitattributes"},
    {"type":"file","oid":"a1afbfd716c97a7ea856bff7160607e9fed9a2f5","size":382,"path":"LICENSE.md"},
    {"type":"file","oid":"fea5d92d7a628ec0594ceaa1968dc477546f92e1","size":8822,"path":"README.md"},
    {"type":"file","oid":"d2c622a663fca5cd86268c2df2f699fc2045bbdd","size":262,"path":"config.json"},
    {"type":"file","oid":"2f244c4a15ebb722254ddcb95b4163e8e3055326","size":467,"path":"publisher.json"}
]"#;

const JB_CONFIG: &str = r#"{
  "model_type": "flare-runtime-package",
  "architecture": "Qwen4ExpForConditionalGeneration",
  "flare": {
    "checkpoint": "nvidia/Qwen3.8-Flash-Next-FP4-Local",
    "execution_abi": 4,
    "prepared_model_contract": 2,
    "serving_context": 1048576
  }
}"#;

const JB_PUBLISHER: &str = r#"[{"name":"Johannes Bertens","namespace":"JohannesBertens",
 "public_key":[209,13,103,209,214,189,0,177,72,173,84,22,104,210,192,3,139,72,85,154,15,42,172,126,167,235,30,154,200,175,40,6],
 "development":false}]"#;

const JB_FINGERPRINT: &str = "6ae68dd76635a095255de5f669ab18ce9a1bb43aea98a7d3b21ee2ec384dedd0";

/// The JB repo carries NO `flare` tag (recorded live) — the Probable path.
const JB_TAGS: &[&str] = &["flare-runtime-package", "license:other", "region:us"];

// ---- detection ----

#[test]
fn flare_disposition_yes_on_exact_flare_tag() {
    // Both tagged repos, with their recorded tag vectors.
    for tags in [LIL_TAGS, LFLARE_TAGS] {
        assert_eq!(
            flare_disposition(&metadata(tags, &["README.md"])),
            FlareDisposition::Yes,
            "a `flare` tag must short-circuit to Yes"
        );
    }
    // Siblings are irrelevant once the tag matches.
    assert_eq!(
        flare_disposition(&metadata(&["flare"], &[])),
        FlareDisposition::Yes
    );
}

#[test]
fn flare_disposition_probable_for_jb_repo_without_flare_tag() {
    // The user's own repo: `flare-runtime-package` is NOT `flare`, but a
    // root publisher.json sits in the siblings ⇒ Probable (confirmed via
    // the config markers, pinned below).
    let jb = metadata(JB_TAGS, &["config.json", "publisher.json", "README.md"]);
    assert_eq!(flare_disposition(&jb), FlareDisposition::Probable);
    // A nested publisher.json is not the root one ⇒ No.
    assert_eq!(
        flare_disposition(&metadata(JB_TAGS, &["sub/publisher.json"])),
        FlareDisposition::No
    );
}

#[test]
fn flare_disposition_no_without_any_marker() {
    assert_eq!(
        flare_disposition(&metadata(&["gguf", "license:apache-2.0"], &["model.gguf"])),
        FlareDisposition::No
    );
}

#[test]
fn flare_config_markers_confirm_all_four_recorded_variants() {
    // format/runtime markers: the recorded lflare config.json, verbatim.
    assert!(config_json_confirms_flare(Some(LFLARE_CONFIG)));
    // model_type + top-level flare object: the recorded JB config.json.
    assert!(config_json_confirms_flare(Some(JB_CONFIG)));
    assert!(config_json_confirms_flare(Some(
        r#"{"flare": {"serving_context": 1048576}}"#
    )));
    // Negatives: absent body, unparseable body, non-object `flare`,
    // a `flare` STRING (only the object marker counts), unrelated config.
    assert!(!config_json_confirms_flare(None));
    assert!(!config_json_confirms_flare(Some("{not json")));
    assert!(!config_json_confirms_flare(Some(r#"{"flare": "yes"}"#)));
    assert!(!config_json_confirms_flare(Some(
        r#"{"model_type": "llama", "format": "safetensors"}"#
    )));
}

// ---- config facts (never errors) ----

#[test]
fn flare_config_facts_parse_from_the_recorded_jb_config() {
    assert_eq!(
        parse_flare_config_facts(Some(JB_CONFIG)),
        FlareConfigFacts {
            serving_context: Some(1048576),
            execution_abi: Some(4),
            prepared_model_contract: Some(2),
            checkpoint: Some("nvidia/Qwen3.8-Flash-Next-FP4-Local".to_string()),
        }
    );
}

#[test]
fn flare_config_facts_never_error_and_default_to_none() {
    // LIL has no config.json at main; lflare's config carries no `flare`
    // object; both yield all-None facts, as do malformed inputs.
    let empty = FlareConfigFacts::default();
    assert_eq!(parse_flare_config_facts(None), empty);
    assert_eq!(parse_flare_config_facts(Some(LFLARE_CONFIG)), empty);
    assert_eq!(parse_flare_config_facts(Some("{oops")), empty);
    assert_eq!(
        parse_flare_config_facts(Some(r#"{"flare": {"serving_context": "big"}}"#)),
        empty,
        "a wrong-typed fact is ignored, never an error"
    );
}

// ---- publisher (display-only fingerprint) ----

#[test]
fn flare_publisher_fingerprint_is_sha256_hex_of_public_key_bytes() {
    let lil = parse_flare_publisher(Some(LIL_PUBLISHER)).unwrap();
    assert_eq!(lil.namespace.as_deref(), Some("local-inference-lab"));
    assert_eq!(lil.name.as_deref(), Some("Local Inference Lab, Inc."));
    assert_eq!(lil.development, Some(false));
    assert_eq!(lil.fingerprint.as_deref(), Some(LIL_FINGERPRINT));

    let jb = parse_flare_publisher(Some(JB_PUBLISHER)).unwrap();
    assert_eq!(jb.namespace.as_deref(), Some("JohannesBertens"));
    assert_eq!(jb.fingerprint.as_deref(), Some(JB_FINGERPRINT));
    // Same publisher identity on LIL and lflare (recorded identical bytes).
    let lflare = parse_flare_publisher(Some(LIL_PUBLISHER)).unwrap();
    assert_eq!(lflare.fingerprint, lil.fingerprint);
}

#[test]
fn flare_publisher_absent_or_malformed_is_none_never_error() {
    assert!(parse_flare_publisher(None).is_none());
    assert!(parse_flare_publisher(Some("{oops")).is_none());
    assert!(parse_flare_publisher(Some("{}")).is_none()); // not an array
    assert!(parse_flare_publisher(Some("[]")).is_none()); // empty array
                                                          // A missing public_key still yields the identity, without a fingerprint.
    let p = parse_flare_publisher(Some(r#"[{"namespace":"n","name":"N"}]"#)).unwrap();
    assert_eq!(p.namespace.as_deref(), Some("n"));
    assert_eq!(p.fingerprint, None);
}

// ---- package filename token grammar (display-only) ----

#[test]
fn flare_package_tokens_pinned_on_the_three_real_names() {
    // LIL/lflare gb202 packed name.
    assert_eq!(
        parse_flare_package_tokens("qwen3.8-flash-next-tp1-c16-mtp5.safetensors"),
        ParsedPackageTokens {
            tp: Some(1),
            max_concurrency: Some(16),
            mtp_draft_tokens: Some(5),
            quant_i4: false,
            arch: None,
            variants: vec![],
        }
    );
    // LIL gb10 packed name: arch token + i4.
    assert_eq!(
        parse_flare_package_tokens("qwen3.8-flash-next-gb10-tp1-c8-i4-mtp5.safetensors"),
        ParsedPackageTokens {
            tp: Some(1),
            max_concurrency: Some(8),
            mtp_draft_tokens: Some(5),
            quant_i4: true,
            arch: Some("gb10".to_string()),
            variants: vec![],
        }
    );
    // JB gb202 packed name: free `yarn4` variant.
    assert_eq!(
        parse_flare_package_tokens("qwen3.8-flash-next-yarn4-tp2.safetensors"),
        ParsedPackageTokens {
            tp: Some(2),
            max_concurrency: None,
            mtp_draft_tokens: None,
            quant_i4: false,
            arch: None,
            variants: vec!["yarn4".to_string()],
        }
    );
}

#[test]
fn flare_package_tokens_ignores_unknown_and_edge_shapes() {
    // Model-name parts (no trailing digits / dotted) are ignored; `mtp`
    // must not be eaten by the `tp`/`c` prefixes; a bare path keeps only
    // the basename; a non-safetensors name still parses.
    let p = parse_flare_package_tokens("sub/qwen3.8-flash-next-mtp5-c16.safetensors");
    assert_eq!(p.mtp_draft_tokens, Some(5));
    assert_eq!(p.tp, None);
    assert_eq!(p.max_concurrency, Some(16));
    assert_eq!(p.variants, Vec::<String>::new());
    // `c` alone / `tp` alone (no digits) are not facts; `gb` alone is not
    // an arch; digits-then-letters is not a free variant.
    let p = parse_flare_package_tokens("c-tp-gb-5x-12x.safetensors");
    assert_eq!(p, ParsedPackageTokens::default());
    // Case-insensitive matching.
    assert_eq!(
        parse_flare_package_tokens("Qwen-TP4-I4.safetensors").tp,
        Some(4)
    );
    assert!(parse_flare_package_tokens("Qwen-TP4-I4.safetensors").quant_i4);
}

// ---- package selection (flare's choose_file rule) ----

#[test]
fn flare_selects_packed_package_from_real_trees() {
    let files = repo_files(LIL_TREE_GB202);
    match select_package(&files).unwrap() {
        FlarePackage::Packed { file } => {
            assert_eq!(
                file.rfilename,
                "qwen3.8-flash-next-tp1-c16-mtp5.safetensors"
            );
            assert_eq!(file.size, Some(107011338717));
            assert_eq!(
                file.lfs.as_ref().unwrap().oid,
                "c3db5e1ac63b05efa67d6525826a8a792c0ce92b810e91ddd5ec0f42c73a175c"
            );
        }
        other => panic!("expected Packed, got {other:?}"),
    }
}

#[test]
fn flare_selects_directory_package_with_weights_and_deployment_companions() {
    let files = repo_files(LFLARE_TREE_GB202);
    match select_package(&files).unwrap() {
        FlarePackage::Directory {
            primary,
            companions,
        } => {
            assert_eq!(primary.rfilename, "model.safetensors");
            assert_eq!(primary.size, Some(22922000));
            let names: Vec<&str> = companions.iter().map(|c| c.rfilename.as_str()).collect();
            assert_eq!(
                names,
                [
                    "weights/engram-0-weights.safetensors",
                    "weights/layer-000.safetensors",
                    "weights/misc.safetensors",
                    "deployments/gb202-tp1/rank-0/graphs.safetensors",
                ]
            );
        }
        other => panic!("expected Directory, got {other:?}"),
    }
    // The directory-form root model.safetensors must NOT degrade to Packed
    // even though it is the unique top-level .safetensors.
    assert!(matches!(
        select_package(&repo_files(LFLARE_TREE_GB202)),
        Some(FlarePackage::Directory { .. })
    ));
}

#[test]
fn flare_package_selection_rejects_non_package_shapes() {
    // No safetensors at all (LIL main).
    assert!(select_package(&repo_files(LIL_TREE_MAIN)).is_none());
    // Two candidate top-level safetensors ⇒ ambiguous ⇒ no package.
    let two = format!(
        "[{},{}]",
        &LIL_TREE_GB202[1..LIL_TREE_GB202.len() - 1],
        &LIL_TREE_GB10[1..LIL_TREE_GB10.len() - 1]
    );
    assert!(select_package(&repo_files(&two)).is_none());
    // A 64-hex stem and a *-data-NN-of-NN shard are not packages; the one
    // remaining candidate is then unique ⇒ Packed.
    let decoys = r#"[
        {"type":"file","path":"dbf20a21e4ad13648446f0d3bddf97a46f568eaa5caf954ccb4390846cbfdc38.safetensors","size":1},
        {"type":"file","path":"model-data-00001-of-00002.safetensors","size":1},
        {"type":"file","path":"qwen3.8-flash-next-tp1-c16-mtp5.safetensors","size":2}
    ]"#;
    match select_package(&repo_files(decoys)).unwrap() {
        FlarePackage::Packed { file } => {
            assert_eq!(
                file.rfilename,
                "qwen3.8-flash-next-tp1-c16-mtp5.safetensors"
            )
        }
        other => panic!("expected Packed, got {other:?}"),
    }
    // Only decoys ⇒ no package.
    let only_decoys =
        r#"[{"type":"file","path":"model-data-00001-of-00002.safetensors","size":1}]"#;
    assert!(select_package(&repo_files(only_decoys)).is_none());
    // A nested (non-top-level) safetensors is not a packed candidate.
    let nested = r#"[{"type":"file","path":"sub/model.safetensors","size":1}]"#;
    assert!(select_package(&repo_files(nested)).is_none());
}

// ---- refs serde shape ----

#[test]
fn flare_repo_refs_parses_camel_case_and_ignores_extra_fields() {
    let refs = parse_refs(LIL_REFS);
    assert_eq!(refs.branches.len(), 3);
    assert!(refs.tags.is_empty());
    // `ref` and `converts` are carried by the live API and ignored; the
    // camelCase targetCommit lands in target_commit.
    assert_eq!(refs.branches[1].name, "gb202");
    assert_eq!(
        refs.branches[1].target_commit,
        "431b302155d53e296aa51633a997b4b5fd92e751"
    );
    // Missing arrays default to empty; a missing targetCommit defaults to
    // the empty string (⇒ dropped by assembly, pinned below).
    let sparse: RepoRefs = serde_json::from_str(r#"{"branches":[{"name":"gb202"}]}"#).unwrap();
    assert_eq!(sparse.branches[0].target_commit, "");
    assert!(sparse.tags.is_empty());
}

// ---- assembly: the three real repos ----

#[test]
fn flare_assemble_lil_packed_revisions_and_no_config_json() {
    let refs = parse_refs(LIL_REFS);
    let info = assemble_flare_info_from(
        &refs,
        &trees(&[
            (LIL_COMMIT_GB202, LIL_TREE_GB202),
            (LIL_COMMIT_GB10, LIL_TREE_GB10),
            (LIL_COMMIT_MAIN, LIL_TREE_MAIN),
        ]),
        None, // LIL has no config.json at main — never an error
        Some(LIL_PUBLISHER),
    )
    .unwrap();

    // main excluded; branches keep refs order (gb202, gb10); no aliases.
    let names: Vec<&str> = info.revisions.iter().map(|r| r.revision.as_str()).collect();
    assert_eq!(names, ["gb202", "gb10"]);
    assert!(info.dropped_refs.is_empty());
    for r in &info.revisions {
        assert_eq!(r.kind, RefKind::Branch);
        assert!(r.aliases.is_empty());
        assert!(matches!(r.package, FlarePackage::Packed { .. }));
    }
    // The commit is the load-bearing pin.
    assert_eq!(info.revisions[0].target_commit, LIL_COMMIT_GB202);
    assert_eq!(info.revisions[1].target_commit, LIL_COMMIT_GB10);
    assert_eq!(info.revisions[0].parsed.tp, Some(1));
    assert_eq!(info.revisions[1].parsed.arch.as_deref(), Some("gb10"));
    // No `flare` object anywhere at main ⇒ all-None facts; publisher lands.
    assert_eq!(info.config, FlareConfigFacts::default());
    assert_eq!(
        info.publisher.as_ref().unwrap().fingerprint.as_deref(),
        Some(LIL_FINGERPRINT)
    );
}

#[test]
fn flare_assemble_lflare_dedupes_v3_alias_and_keeps_the_collision_pair() {
    let refs = parse_refs(LFLARE_REFS);
    let info = assemble_flare_info_from(
        &refs,
        &trees(&[
            (LFLARE_COMMIT_GB202, LFLARE_TREE_GB202),
            (LFLARE_COMMIT_V1, LFLARE_TREE_V1),
            (LFLARE_COMMIT_V2, LFLARE_TREE_V2),
            (LFLARE_COMMIT_MAIN, LFLARE_TREE_MAIN),
        ]),
        Some(LFLARE_CONFIG),
        Some(LIL_PUBLISHER), // lflare's publisher.json is byte-identical to LIL's
    )
    .unwrap();

    // Branches first (gb202), then tags in refs order (v2, v1 — v3 folds
    // into gb202 as an alias because it shares the targetCommit).
    let names: Vec<&str> = info.revisions.iter().map(|r| r.revision.as_str()).collect();
    assert_eq!(names, ["gb202", "v2", "v1"]);
    assert_eq!(info.revisions[0].aliases, ["v3"]);
    assert_eq!(info.revisions[0].kind, RefKind::Branch);
    assert_eq!(info.revisions[0].target_commit, LFLARE_COMMIT_GB202);
    assert_eq!(info.revisions[1].kind, RefKind::Tag);
    assert_eq!(info.revisions[1].target_commit, LFLARE_COMMIT_V2);
    assert_eq!(info.revisions[2].target_commit, LFLARE_COMMIT_V1);

    // gb202 is the DIRECTORY form; v1/v2 are PACKED under the SAME filename
    // with DIFFERENT sha256s — the cross-revision collision the commit pin
    // exists to survive.
    assert!(matches!(
        info.revisions[0].package,
        FlarePackage::Directory { .. }
    ));
    let (v1_file, v2_file) = match (&info.revisions[2].package, &info.revisions[1].package) {
        (FlarePackage::Packed { file: a }, FlarePackage::Packed { file: b }) => (a, b),
        other => panic!("expected two Packed revisions, got {other:?}"),
    };
    assert_eq!(v1_file.rfilename, v2_file.rfilename);
    assert_eq!(
        v1_file.lfs.as_ref().unwrap().oid,
        "fff1962065e4b09858bf700646336243ab18faf9900dad9c4f8486345032ef3d"
    );
    assert_eq!(
        v2_file.lfs.as_ref().unwrap().oid,
        "35112671d85c719222c40d527d88f475cc38cbae6f20d1b94d411083ce0ac7f4"
    );
    // lflare's config confirms FLARE via format/runtime markers but has no
    // `flare` facts object; the parsed tokens still ride each revision.
    assert!(config_json_confirms_flare(Some(LFLARE_CONFIG)));
    assert_eq!(info.config, FlareConfigFacts::default());
    assert_eq!(info.revisions[1].parsed.mtp_draft_tokens, Some(5));
}

#[test]
fn flare_assemble_jb_probable_repo_lands_facts_and_fingerprint() {
    let refs = parse_refs(JB_REFS);
    let info = assemble_flare_info_from(
        &refs,
        &trees(&[
            (JB_COMMIT_GB202, JB_TREE_GB202),
            (JB_COMMIT_MAIN, JB_TREE_MAIN),
        ]),
        Some(JB_CONFIG),
        Some(JB_PUBLISHER),
    )
    .unwrap();

    assert_eq!(info.revisions.len(), 1);
    let gb202 = &info.revisions[0];
    assert_eq!(gb202.revision, "gb202");
    assert_eq!(gb202.target_commit, JB_COMMIT_GB202);
    match &gb202.package {
        FlarePackage::Packed { file } => {
            assert_eq!(file.rfilename, "qwen3.8-flash-next-yarn4-tp2.safetensors");
            assert_eq!(file.size, Some(111434386432));
            assert_eq!(
                file.lfs.as_ref().unwrap().oid,
                "a08b6314debdeff6936723f03faf6a558b771f6ff9ea94fc25380165439188f8"
            );
        }
        other => panic!("expected Packed, got {other:?}"),
    }
    assert_eq!(gb202.parsed.tp, Some(2));
    assert_eq!(gb202.parsed.variants, ["yarn4"]);
    assert_eq!(gb202.parsed.arch, None);
    // The full JB fact set recorded live.
    assert_eq!(
        info.config,
        FlareConfigFacts {
            serving_context: Some(1048576),
            execution_abi: Some(4),
            prepared_model_contract: Some(2),
            checkpoint: Some("nvidia/Qwen3.8-Flash-Next-FP4-Local".to_string()),
        }
    );
    assert_eq!(
        info.publisher.as_ref().unwrap().fingerprint.as_deref(),
        Some(JB_FINGERPRINT)
    );
}

// ---- assembly: drop/skip/error rules ----

#[test]
fn flare_assemble_drops_commitless_refs_never_degrades_silently() {
    // A refs entry whose targetCommit is missing or malformed DROPS the
    // revision and names it in dropped_refs (plans/flare-models.md §3.1: trees/URLs/registry
    // pin to the commit — a name-only entry is not degradable).
    let refs: RepoRefs = serde_json::from_str(
        r#"{"branches":[
            {"name":"gb202","targetCommit":"431b302155d53e296aa51633a997b4b5fd92e751"},
            {"name":"broken","targetCommit":""},
            {"name":"malformed","targetCommit":"not-a-sha"}
        ],"tags":[{"name":"short","targetCommit":"deadbeef"}]}"#,
    )
    .unwrap();
    let info = assemble_flare_info_from(
        &refs,
        &trees(&[("431b302155d53e296aa51633a997b4b5fd92e751", LIL_TREE_GB202)]),
        None,
        None,
    )
    .unwrap();
    assert_eq!(info.revisions.len(), 1);
    assert_eq!(info.revisions[0].revision, "gb202");
    assert_eq!(info.dropped_refs, ["broken", "malformed", "short"]);
}

#[test]
fn flare_assemble_skips_packageless_revisions_but_errors_when_none_survive() {
    // gb10's tree yields no package (main-style metadata-only tree) ⇒ the
    // revision is SKIPPED, not an error; gb202 survives.
    let refs = parse_refs(LIL_REFS);
    let info = assemble_flare_info_from(
        &refs,
        &trees(&[
            (LIL_COMMIT_GB202, LIL_TREE_GB202),
            (LIL_COMMIT_GB10, LIL_TREE_MAIN),
        ]),
        None,
        None,
    )
    .unwrap();
    let names: Vec<&str> = info.revisions.iter().map(|r| r.revision.as_str()).collect();
    assert_eq!(names, ["gb202"]);

    // A payload commit with NO tree entry is skipped the same way.
    let info = assemble_flare_info_from(
        &refs,
        &trees(&[(LIL_COMMIT_GB10, LIL_TREE_GB10)]),
        None,
        None,
    )
    .unwrap();
    let names: Vec<&str> = info.revisions.iter().map(|r| r.revision.as_str()).collect();
    assert_eq!(names, ["gb10"]);

    // Nothing survives ⇒ the one assembly error (plus the all-commitless
    // end state).
    assert!(matches!(
        assemble_flare_info_from(&refs, &HashMap::new(), None, None),
        Err(FlareScanError::NoPackage)
    ));
    let only_main: RepoRefs = serde_json::from_str(
        r#"{"branches":[{"name":"main","targetCommit":"5fcda54829558faf35528c9e223e41c79c3e7feb"}]}"#,
    )
    .unwrap();
    assert!(matches!(
        assemble_flare_info_from(&only_main, &HashMap::new(), None, None),
        Err(FlareScanError::NoPackage)
    ));
}

#[test]
fn flare_assemble_alias_entry_is_never_double_counted_in_dropped_refs() {
    // The alias path: a tag sharing the canonical branch commit lands in
    // `aliases`, and a commitless tag lands in `dropped_refs` — the two
    // lists stay disjoint and complete.
    let refs: RepoRefs = serde_json::from_str(
        r#"{"branches":[{"name":"gb202","targetCommit":"a98704bcce2442608c3142b5c3e093ecce15caa5"}],
            "tags":[{"name":"v3","targetCommit":"a98704bcce2442608c3142b5c3e093ecce15caa5"},
                    {"name":"vX","targetCommit":"zzzz"}]}"#,
    )
    .unwrap();
    let info = assemble_flare_info_from(
        &refs,
        &trees(&[(
            "a98704bcce2442608c3142b5c3e093ecce15caa5",
            LFLARE_TREE_GB202,
        )]),
        None,
        None,
    )
    .unwrap();
    assert_eq!(info.revisions[0].aliases, ["v3"]);
    assert_eq!(info.dropped_refs, ["vX"]);
}
