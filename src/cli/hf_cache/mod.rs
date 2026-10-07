//! `hf-cache` subcommand group (plans/hf-cache-sync.md §2, §5.2): the
//! hub-cache sync pipeline and the pure path helper.
//!
//! Split out of the single `cli/hf_cache_cmd.rs` (plan W3.5); this file is
//! the group's facade. Like `models/mod.rs` and `api/mod.rs` the
//! submodules are declared private (`mod sync;`, never `pub mod sync;`)
//! and the pieces that leave the group are re-exported here, so
//! `cli::hf_cache::X` keeps resolving for `cli/tests.rs` while the module
//! itself stays private to `cli`:
//!
//! - `selection` — the pure plans/hf-cache-sync.md §2.2 selector: [`SelectionMode`],
//!   [`SyncSelectionError`], [`select_sync_files`]
//! - `sync` — the plans/hf-cache-sync.md §5.2 sync pipeline (config/engine bootstrap through
//!   `cli::run`, cache plan, drain, publish gate, refs, run-tail) plus the
//!   symlink/ref policy helpers
//! - `path` — `hf-cache path`: snapshot-path math with a `refs/` lookup
//!   (imported privately: nothing in it leaves the group)
//!
//! Helpers used by two or more submodules stay right here; today that is
//! [`absolute_path`], needed by both the sync run-tail (the "last line: the
//! snapshot path" contract, plans/hf-cache-sync.md §2.4) and `hf-cache path`.

use std::path::{Path, PathBuf};

use super::args::{HfCacheArgs, HfCacheCommand};

mod path;
mod selection;
mod sync;

pub use selection::*;
pub use sync::*;

use path::run_hf_cache_path;

/// Dispatch the `hf-cache` subcommand group.
pub(super) async fn run_hf_cache(args: HfCacheArgs) -> i32 {
    match args.command {
        HfCacheCommand::Sync(args) => run_hf_cache_sync(args).await,
        HfCacheCommand::Path(args) => run_hf_cache_path(args).await,
    }
}

/// Absolute form of `path` without canonicalization's symlink resolution:
/// already-absolute paths pass through verbatim, relative paths anchor at
/// the current directory. plans/hf-cache-sync.md §2.4's "last line: the snapshot path" wants a
/// stable, predictable absolute path (containers mount caches elsewhere).
pub(super) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}
