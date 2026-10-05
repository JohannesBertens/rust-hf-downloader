// Declare modules
pub mod app;
pub mod render;
// File-tree navigation model (flatten/toggle/count) shared by the renderer
// and the app layer; private to `ui` — see tree.rs docs.
mod tree;

// Re-export App
pub use app::App;
