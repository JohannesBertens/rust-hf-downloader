//! Navigation model for the repository file tree (plan W3.4a).
//!
//! These helpers describe the tree as the user walks it — the flattened
//! visible list, the expansion toggle, the recursive file count — and are
//! consumed from both sides of the UI:
//!
//! - `ui::render` (Standard mode) draws the flattened list and the per
//!   directory "(N files)" suffix
//! - `ui::app::events` moves the cursor / toggles directories
//! - `ui::app::downloads` counts the files under a selected directory for
//!   its download label
//!
//! They live here instead of in `ui::render` because they are navigation
//! model, not drawing: the app layer must not reach into the renderer to
//! move a cursor. The module is private to `ui` (same convention as
//! `models/mod.rs` and `cli/hf_cache/mod.rs`), so `crate::ui::tree::X`
//! resolves for everything inside `ui` and nothing outside it.

use crate::models::FileTreeNode;

/// Recursively count file nodes under a tree node (the tree panel's
/// "(N files)" suffix and the directory-download status label).
pub fn count_tree_files(node: &FileTreeNode) -> usize {
    if node.is_dir {
        node.children.iter().map(count_tree_files).sum()
    } else {
        1
    }
}

/// Flatten tree into a list for rendering
pub fn flatten_tree(node: &FileTreeNode) -> Vec<FileTreeNode> {
    let mut result = Vec::new();
    flatten_tree_recursive(node, &mut result);
    result
}

fn flatten_tree_recursive(node: &FileTreeNode, result: &mut Vec<FileTreeNode>) {
    for child in &node.children {
        result.push(child.clone());
        if child.is_dir && child.expanded {
            flatten_tree_recursive(child, result);
        }
    }
}

/// Public helper for flattening tree (used by events.rs for navigation)
pub fn flatten_tree_for_navigation(node: &FileTreeNode) -> Vec<FileTreeNode> {
    flatten_tree(node)
}

/// Helper function to toggle a node's expansion state by path
pub fn toggle_node_expansion(node: &mut FileTreeNode, target_path: &str) -> bool {
    for child in &mut node.children {
        if child.path == target_path {
            if child.is_dir {
                child.expanded = !child.expanded;
            }
            return true;
        }

        if child.is_dir && toggle_node_expansion(child, target_path) {
            return true;
        }
    }
    false
}
