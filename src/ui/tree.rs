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

/// Count visible (flattened) nodes in the tree without cloning or allocating.
pub fn count_visible_nodes(node: &FileTreeNode) -> usize {
    let mut count = 0;
    count_visible_nodes_recursive(node, &mut count);
    count
}

fn count_visible_nodes_recursive(node: &FileTreeNode, count: &mut usize) {
    for child in &node.children {
        *count += 1;
        if child.is_dir && child.expanded {
            count_visible_nodes_recursive(child, count);
        }
    }
}

/// Flatten tree into a list of node references for inspection without cloning.
#[allow(dead_code)] // 2026-10 (U3): borrowed tree flattening for zero-copy inspection
pub fn flatten_tree_refs(node: &FileTreeNode) -> Vec<&FileTreeNode> {
    let mut result = Vec::new();
    flatten_tree_refs_recursive(node, &mut result);
    result
}

fn flatten_tree_refs_recursive<'a>(node: &'a FileTreeNode, result: &mut Vec<&'a FileTreeNode>) {
    for child in &node.children {
        result.push(child);
        if child.is_dir && child.expanded {
            flatten_tree_refs_recursive(child, result);
        }
    }
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

#[cfg(test)]
mod tests {
    //! G7 (test-hardening): the tree navigation model had no direct unit
    //! tests — its behavior was only observable through the render
    //! snapshots and the App cursor tests. These pin the three helpers on
    //! a nested fixture with mixed expansion state.
    use super::*;

    fn file(path: &str, size: u64) -> FileTreeNode {
        FileTreeNode {
            name: path.rsplit('/').next().unwrap().to_string(),
            path: path.to_string(),
            is_dir: false,
            size: Some(size),
            children: Vec::new(),
            expanded: false,
            depth: path.matches('/').count(),
        }
    }

    fn dir(path: &str, expanded: bool, children: Vec<FileTreeNode>) -> FileTreeNode {
        FileTreeNode {
            name: path.rsplit('/').next().unwrap().to_string(),
            path: path.to_string(),
            is_dir: true,
            size: None,
            children,
            expanded,
            depth: path.matches('/').count(),
        }
    }

    /// Nested fixture (paths double as identity):
    /// ```text
    /// root/                       5 files total
    ///   README.md                 (file)
    ///   Q4_K_M/        expanded   (dir, 3 files)
    ///     a.gguf                   (file)
    ///     shards/      collapsed  (dir, 2 files)
    ///       s1.gguf                (file)
    ///       s2.gguf                (file)
    ///   original/      collapsed  (dir, 1 file)
    ///     consolidated.safetensors (file)
    /// ```
    fn nested_tree() -> FileTreeNode {
        dir(
            "root",
            true,
            vec![
                file("root/README.md", 100),
                dir(
                    "root/Q4_K_M",
                    true,
                    vec![
                        file("root/Q4_K_M/a.gguf", 10),
                        dir(
                            "root/Q4_K_M/shards",
                            false,
                            vec![
                                file("root/Q4_K_M/shards/s1.gguf", 1),
                                file("root/Q4_K_M/shards/s2.gguf", 2),
                            ],
                        ),
                    ],
                ),
                dir(
                    "root/original",
                    false,
                    vec![file("root/original/consolidated.safetensors", 20)],
                ),
            ],
        )
    }

    /// Recursive file count on nested subtrees: the whole tree (6), a
    /// nested subtree (3 under Q4_K_M), a collapsed dir still counts its
    /// files (expansion is a VIEW concern), and a file counts as 1.
    #[test]
    fn count_tree_files_counts_nested_subtrees_recursively() {
        let root = nested_tree();
        assert_eq!(count_tree_files(&root), 5);

        let q4 = root
            .children
            .iter()
            .find(|c| c.path == "root/Q4_K_M")
            .unwrap();
        assert_eq!(count_tree_files(q4), 3);

        let shards = q4
            .children
            .iter()
            .find(|c| c.path == "root/Q4_K_M/shards")
            .unwrap();
        assert_eq!(count_tree_files(shards), 2);

        let original = root
            .children
            .iter()
            .find(|c| c.path == "root/original")
            .unwrap();
        assert_eq!(count_tree_files(original), 1, "collapsed dirs still count");

        assert_eq!(count_tree_files(&file("root/README.md", 100)), 1);
    }

    /// flatten_tree emits exactly the VISIBLE rows: children of expanded
    /// dirs only, the root itself excluded — the render list and the
    /// navigation cursor both walk this ordering.
    #[test]
    fn flatten_tree_lists_only_expanded_children_in_order() {
        let flat = flatten_tree(&nested_tree());
        let paths: Vec<&str> = flat.iter().map(|n| n.path.as_str()).collect();
        // root/Q4_K_M is expanded → itself, a.gguf AND the collapsed
        // shards/ dir row are visible (only the collapsed dir's CHILDREN
        // are hidden); original/ is collapsed → its file hidden too.
        assert_eq!(
            paths,
            vec![
                "root/README.md",
                "root/Q4_K_M",
                "root/Q4_K_M/a.gguf",
                "root/Q4_K_M/shards",
                "root/original"
            ]
        );
        // Not the root itself, ever.
        assert!(flat.iter().all(|n| n.path != "root"));
    }

    /// `flatten_tree_for_navigation` is the seam `ui::app::events` walks
    /// for cursor math; today it delegates to `flatten_tree` (identical
    /// output by construction). The pin: navigation sees EXACTLY the
    /// visible rows — if the two ever diverge (e.g. navigation keeping
    /// hidden rows selectable), this fails while the render snapshot
    /// would not.
    #[test]
    fn flatten_tree_for_navigation_matches_flatten_tree_rows() {
        let tree = nested_tree();
        let render = flatten_tree(&tree);
        let nav = flatten_tree_for_navigation(&tree);
        let render_rows: Vec<&str> = render.iter().map(|n| n.path.as_str()).collect();
        let nav_rows: Vec<&str> = nav.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(render_rows, nav_rows);
        assert_eq!(nav_rows.len(), 5);
    }

    /// Nested expansion toggles: a DEEP path flips its node (found by
    /// recursion, not top level), a second toggle restores it, file
    /// paths and unknown paths return false and change nothing.
    #[test]
    fn toggle_node_expansion_toggles_nested_dirs_only() {
        let mut tree = nested_tree();

        // Deeply nested dir under an expanded parent.
        assert!(toggle_node_expansion(&mut tree, "root/Q4_K_M/shards"));
        let shards_expanded = |t: &FileTreeNode| {
            t.children[1]
                .children
                .iter()
                .find(|c| c.path == "root/Q4_K_M/shards")
                .unwrap()
                .expanded
        };
        assert!(shards_expanded(&tree), "first toggle expands");
        // Now visible in the flatten output.
        assert!(flatten_tree(&tree)
            .iter()
            .any(|n| n.path == "root/Q4_K_M/shards/s1.gguf"));

        assert!(toggle_node_expansion(&mut tree, "root/Q4_K_M/shards"));
        assert!(!shards_expanded(&tree), "second toggle collapses");

        // A FILE path: found but not a dir → reported handled (true, the
        // historical contract) yet nothing flips.
        let before = flatten_tree(&tree);
        assert!(toggle_node_expansion(&mut tree, "root/README.md"));
        assert_eq!(flatten_tree(&tree).len(), before.len());

        // Unknown path: not found → false. (A nested FILE path also
        // reports true — same rule as README.md above.)
        assert!(!toggle_node_expansion(&mut tree, "root/nope"));
        assert!(toggle_node_expansion(
            &mut tree,
            "root/Q4_K_M/shards/s1.gguf"
        ));
    }

    #[test]
    fn count_visible_nodes_matches_flatten_tree_len_across_permutations() {
        let mut tree = nested_tree();

        // 1. Initial state: Q4_K_M expanded, shards collapsed, original collapsed
        assert_eq!(count_visible_nodes(&tree), flatten_tree(&tree).len());
        assert_eq!(flatten_tree_refs(&tree).len(), flatten_tree(&tree).len());

        // 2. Expand shards
        toggle_node_expansion(&mut tree, "root/Q4_K_M/shards");
        assert_eq!(count_visible_nodes(&tree), flatten_tree(&tree).len());
        assert_eq!(flatten_tree_refs(&tree).len(), flatten_tree(&tree).len());

        // 3. Expand original
        toggle_node_expansion(&mut tree, "root/original");
        assert_eq!(count_visible_nodes(&tree), flatten_tree(&tree).len());
        assert_eq!(flatten_tree_refs(&tree).len(), flatten_tree(&tree).len());

        // 4. Collapse Q4_K_M (which hides its expanded child shards)
        toggle_node_expansion(&mut tree, "root/Q4_K_M");
        assert_eq!(count_visible_nodes(&tree), flatten_tree(&tree).len());
        assert_eq!(flatten_tree_refs(&tree).len(), flatten_tree(&tree).len());

        // 5. Collapse original
        toggle_node_expansion(&mut tree, "root/original");
        assert_eq!(count_visible_nodes(&tree), flatten_tree(&tree).len());
        assert_eq!(flatten_tree_refs(&tree).len(), flatten_tree(&tree).len());

        // 6. Leaf file node
        let leaf = file("root/README.md", 10);
        assert_eq!(count_visible_nodes(&leaf), flatten_tree(&leaf).len());
        assert_eq!(flatten_tree_refs(&leaf).len(), flatten_tree(&leaf).len());
    }
}
