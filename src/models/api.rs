//! HuggingFace API DTOs: model search results, repository metadata,
//! tree entries, LFS pointers, and quantization summaries.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub author: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "lastModified", default)]
    pub last_modified: Option<String>,
}

/// Extended model metadata from /api/models/{model_id}
#[derive(Debug, Clone, Deserialize)]
pub struct ModelMetadata {
    #[serde(rename = "id")]
    pub model_id: String,
    #[serde(default)]
    pub library_name: Option<String>,
    #[serde(default)]
    pub pipeline_tag: Option<String>,
    #[serde(default)]
    pub card_data: Option<ModelCardData>,
    #[serde(default)]
    pub siblings: Vec<RepoFile>, // All files in the repo
    #[serde(default)]
    pub tags: Vec<String>,
    /// Top-level commit SHA of the repo (tree tip this metadata describes).
    /// Absent on some API shapes; authoritative pinning goes through
    /// [`crate::api::resolve_revision_sha`].
    #[serde(default)]
    #[allow(dead_code)]
    // 2026-10 (R4): serde-only DTO field — parsed for schema completeness, no reader yet
    pub sha: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelCardData {
    #[serde(default)]
    pub base_model: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub language: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    // 2026-10 (R4): serde-only DTO field — parsed for schema completeness, no reader yet
    pub datasets: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoFile {
    pub rfilename: String, // API uses 'rfilename' for relative path
    #[serde(default)]
    pub size: Option<u64>,
    /// Git blob sha1 for non-LFS files, sha256 for LFS ones — the hub-cache
    /// blob name. Absent on plain siblings payloads.
    #[serde(default)]
    #[allow(dead_code)]
    // 2026-10 (R4): serde-only DTO field — parsed for schema completeness, no reader yet
    pub oid: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    // 2026-10 (R4): serde-only DTO field — parsed for schema completeness, no reader yet
    pub lfs: Option<LfsInfo>, // Reuse existing LfsInfo struct
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LfsInfo {
    pub oid: String,
    pub size: u64,
    #[serde(rename = "pointerSize")]
    pub pointer_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    #[serde(rename = "type")]
    pub file_type: String,
    pub path: String,
    #[serde(default)]
    pub size: u64,
    /// Git blob sha1 for non-LFS files, sha256 for LFS ones (tree entries).
    #[serde(default)]
    pub oid: Option<String>,
    #[serde(default)]
    pub lfs: Option<LfsInfo>,
}

#[derive(Debug, Clone)]
pub struct QuantizationInfo {
    pub quant_type: String,
    pub filename: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QuantizationGroup {
    pub quant_type: String,
    pub files: Vec<QuantizationInfo>, // All files in this quantization type
    pub total_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- serde parsing of API payload shapes ----

    #[test]
    fn model_metadata_parses_sha_sibling_oid_and_lfs() {
        let meta: ModelMetadata = serde_json::from_str(
            r#"{
                "id": "a/b",
                "sha": "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234",
                "siblings": [
                    {
                        "rfilename": "model.safetensors",
                        "size": 12345,
                        "oid": "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b",
                        "lfs": {
                            "oid": "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b",
                            "size": 12345,
                            "pointerSize": 134
                        }
                    },
                    {
                        "rfilename": "config.json",
                        "size": 100,
                        "oid": "d6a7702e2c35b4b1f9c8e3e9c2b1a0d4f7e6c5b4"
                    }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(meta.model_id, "a/b");
        assert_eq!(
            meta.sha.as_deref(),
            Some("f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234")
        );
        // LFS entry: top-level oid and lfs.oid both carry the sha256.
        let lfs_file = &meta.siblings[0];
        assert_eq!(
            lfs_file.oid.as_deref(),
            Some("6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b")
        );
        assert_eq!(
            lfs_file.lfs.as_ref().unwrap().oid,
            lfs_file.oid.clone().unwrap()
        );
        // Non-LFS entry: 40-hex git blob sha, no lfs block.
        assert_eq!(
            meta.siblings[1].oid.as_deref(),
            Some("d6a7702e2c35b4b1f9c8e3e9c2b1a0d4f7e6c5b4")
        );
        assert!(meta.siblings[1].lfs.is_none());
    }

    #[test]
    fn model_metadata_without_sha_or_oid_still_parses() {
        // Old fixtures / API shapes that predate sha and oid keep parsing:
        // every new field is #[serde(default)].
        let meta: ModelMetadata = serde_json::from_str(
            r#"{
                "id": "a/b",
                "siblings": [{"rfilename": "model.gguf", "size": 7}]
            }"#,
        )
        .unwrap();
        assert_eq!(meta.sha, None);
        assert_eq!(meta.siblings[0].rfilename, "model.gguf");
        assert_eq!(meta.siblings[0].oid, None);
        assert!(meta.siblings[0].lfs.is_none());
    }

    #[test]
    fn model_file_parses_tree_entry_oid_and_lfs() {
        let file: ModelFile = serde_json::from_str(
            r#"{
                "type": "file",
                "path": "sub/model.safetensors",
                "size": 12345,
                "oid": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "lfs": {
                    "oid": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "size": 12345,
                    "pointerSize": 134
                }
            }"#,
        )
        .unwrap();
        assert_eq!(file.file_type, "file");
        assert_eq!(file.path, "sub/model.safetensors");
        assert_eq!(
            file.oid.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert_eq!(file.lfs.as_ref().unwrap().size, 12345);
    }
}
