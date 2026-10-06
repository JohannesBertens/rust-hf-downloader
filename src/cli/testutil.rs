//! Shared fixtures for the per-subject CLI test modules (M6/T1: the
//! former 1986-line `cli/tests.rs` grab-bag split by subject; this file
//! holds what three or more subjects — or TESTING.md's conventions —
//! reference). Test-only: declared `#[cfg(test)]` from `cli/mod.rs`.
//!
//! - [`file_spec`] / [`metadata_with`] — resolve/selection fixtures
//! - `snap!` — the pretty-JSON insta macro behind every CLI event
//!   snapshot (`src/cli/snapshots/`)
//! - [`SharedStderr`] — in-memory stderr sink for Reporter tests
//! - [`VarGuard`] — restore-one-env-var-on-drop; every test that mutates
//!   ambient env also takes `paths::ENV_MUTEX` (the crate-wide
//!   convention, see TESTING.md)

use crate::models::ModelMetadata;

use super::resolve::FileSpec;

/// Minimal `FileSpec` fixture: filename + size, no sha256.
pub(super) fn file_spec(filename: &str, size: u64) -> FileSpec {
    FileSpec {
        filename: filename.to_string(),
        size_bytes: size,
        sha256: None,
    }
}

/// `ModelMetadata` for model `a/b` with the given sibling files
/// `(name, size)`; no LFS info, no tags.
pub(super) fn metadata_with(files: &[(&str, Option<u64>)]) -> ModelMetadata {
    ModelMetadata {
        model_id: "a/b".to_string(),
        library_name: None,
        pipeline_tag: None,
        card_data: None,
        siblings: files
            .iter()
            .map(|(name, size)| crate::models::RepoFile {
                rfilename: name.to_string(),
                size: *size,
                oid: None,
                lfs: None,
            })
            .collect(),
        tags: Vec::new(),
        sha: None,
    }
}

/// Serialize `event` as pretty JSON and pin it as an insta snapshot.
/// A macro (not a fn) so the snapshot file name derives from the CALLING
/// test module (`rust_hf_downloader__cli__<test-module>__<name>.snap` in
/// `src/cli/snapshots/`), not from this file.
macro_rules! snap {
    ($event:expr, $name:expr $(,)?) => {{
        let json = serde_json::to_string_pretty($event).unwrap();
        insta::assert_snapshot!($name, json);
    }};
}
pub(crate) use snap;

/// Shareable in-memory stderr sink for `Reporter::new_with_stderr`.
#[derive(Clone, Default)]
pub(super) struct SharedStderr(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl SharedStderr {
    /// Drain and return everything written so far.
    pub(super) fn take(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().drain(..).collect()).unwrap()
    }
}

impl std::io::Write for SharedStderr {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Restore one env var on drop (matrix cells mutate the ambient env; tests
/// that do so share `paths::ENV_MUTEX`).
pub(super) struct VarGuard {
    key: &'static str,
    saved: Option<std::ffi::OsString>,
}

impl VarGuard {
    pub(super) fn set(key: &'static str, value: Option<&str>) -> Self {
        let saved = std::env::var_os(key);
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        Self { key, saved }
    }
}

impl Drop for VarGuard {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}
