//! Shared data types used across the API, engine, CLI, and UI layers.
//!
//! W3.1 split the former grab-bag into five submodules; this file is a facade
//! that re-exports them wholesale, so every `crate::models::X` import keeps
//! compiling unchanged:
//!
//! - `api` — HuggingFace API DTOs: `ModelInfo`, `ModelMetadata`,
//!   `ModelCardData`, `RepoFile`, `LfsInfo`, `ModelFile`, `QuantizationInfo`,
//!   `QuantizationGroup`
//! - `ui` — UI state enums: `PopupMode`, `FilterPreset`, `InputMode`,
//!   `SortField`, `SortDirection`, `FocusedPane`, `ModelDisplayMode`, plus
//!   `FileTreeNode` (constructed by `api`, owned by the UI that renders it)
//! - `engine` — progress, queue and verification types exchanged with the
//!   engine: `ChunkProgress`, `DownloadProgress`, `DownloadStatus`,
//!   `DownloadMetadata`, `DownloadRegistry`, `FileOutcome`, `VerifyOutcome`,
//!   `QueueState`, `QueueItemSummary`, `VerificationProgress`,
//!   `VerificationQueueItem`
//! - `cache` — cache aliases (`QuantizationCache`, `CompleteDownloads`,
//!   `MetadataCache`, `FileTreeCache`, `SearchCache`), `SearchKey`, `ApiCache`
//! - `options` — `AppOptions`, the persisted config-file schema (every serde
//!   attribute is load-bearing)
//!
//! The submodules are private on purpose (`mod api;`, never `pub mod api;`):
//! a public `models::api` would be dragged in by the `use crate::models::*`
//! globs in `ui/app/*` and shadow `crate::api` there. Impl blocks live with
//! their types; the tests moved to the submodule that owns what they test.

mod api;
mod cache;
mod engine;
mod options;
mod ui;

pub use api::*;
pub use cache::*;
pub use engine::*;
pub use options::*;
pub use ui::*;
