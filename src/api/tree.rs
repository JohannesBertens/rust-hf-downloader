//! File-tree construction: turns a flat `RepoFile` listing into the
//! `FileTreeNode` the Standard-mode UI navigates (dirs-first sort + rolled-up
//! directory sizes).
//!
//! Moved verbatim from `src/api.rs` (plan W3.2b).

use crate::models::{FileTreeNode, RepoFile};

/// Build tree structure from flat file list
pub fn build_file_tree(files: Vec<RepoFile>) -> FileTreeNode {
    let mut root = FileTreeNode {
        name: String::new(),
        path: String::new(),
        is_dir: true,
        size: None,
        children: Vec::new(),
        expanded: true, // Root is always expanded
        depth: 0,
    };

    for file in files {
        let parts: Vec<&str> = file.rfilename.split('/').collect();
        insert_into_tree(&mut root, &parts, 0, &file);
    }

    // Sort children at each level (directories first, then alphabetically)
    sort_tree_recursive(&mut root);

    // Calculate directory sizes (sum of all files within)
    calculate_directory_sizes(&mut root);

    root
}

/// Calculate total size for each directory recursively
fn calculate_directory_sizes(node: &mut FileTreeNode) -> u64 {
    if node.is_dir {
        let total: u64 = node
            .children
            .iter_mut()
            .map(calculate_directory_sizes)
            .sum();
        node.size = Some(total);
        total
    } else {
        node.size.unwrap_or(0)
    }
}

fn insert_into_tree(node: &mut FileTreeNode, parts: &[&str], depth: usize, file: &RepoFile) {
    if parts.is_empty() {
        return;
    }

    let current_part = parts[0];
    let is_last = parts.len() == 1;

    // Find or create child node
    let child_pos = node
        .children
        .iter()
        .position(|child| child.name == current_part);

    let child = if let Some(pos) = child_pos {
        &mut node.children[pos]
    } else {
        let new_node = FileTreeNode {
            name: current_part.to_string(),
            path: if node.path.is_empty() {
                current_part.to_string()
            } else {
                format!("{}/{}", node.path, current_part)
            },
            is_dir: !is_last,
            size: if is_last { file.size } else { None },
            children: Vec::new(),
            expanded: false,
            depth: depth + 1,
        };
        node.children.push(new_node);
        node.children.last_mut().unwrap()
    };

    if !is_last {
        insert_into_tree(child, &parts[1..], depth + 1, file);
    }
}

fn sort_tree_recursive(node: &mut FileTreeNode) {
    node.children.sort_by(|a, b| {
        // Directories before files
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    for child in &mut node.children {
        sort_tree_recursive(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_file(path: &str, size: u64) -> RepoFile {
        RepoFile {
            rfilename: path.to_string(),
            size: Some(size),
            oid: None,
            lfs: None,
        }
    }
    // ---- build_file_tree ----

    #[test]
    fn build_file_tree_nests_and_sorts() {
        let files = vec![
            repo_file("a/b.gguf", 100),
            repo_file("a/c.txt", 50),
            repo_file("d.bin", 30),
            repo_file("e/f/g.gguf", 7),
        ];

        let root = build_file_tree(files);

        assert_eq!(root.name, "");
        assert!(root.is_dir);
        assert_eq!(root.depth, 0);
        // Directories first ("a", "e"), then files ("d.bin").
        let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "e", "d.bin"]);
        assert_eq!(root.size, Some(187));

        let dir_a = &root.children[0];
        assert!(dir_a.is_dir);
        assert_eq!(dir_a.path, "a");
        assert_eq!(dir_a.size, Some(150));
        let a_names: Vec<&str> = dir_a.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(a_names, vec!["b.gguf", "c.txt"]);

        let b = &dir_a.children[0];
        assert!(!b.is_dir);
        assert_eq!(b.path, "a/b.gguf");
        assert_eq!(b.size, Some(100));
        assert_eq!(b.depth, 2);

        // Deeply nested branch: e/f/g.gguf
        let dir_e = &root.children[1];
        assert_eq!(dir_e.size, Some(7));
        let dir_f = &dir_e.children[0];
        assert_eq!(dir_f.name, "f");
        let g = &dir_f.children[0];
        assert_eq!(g.name, "g.gguf");
        assert_eq!(g.path, "e/f/g.gguf");
        assert_eq!(g.depth, 3);

        // Plain file at root level.
        let d = &root.children[2];
        assert!(!d.is_dir);
        assert_eq!(d.path, "d.bin");
        assert_eq!(d.size, Some(30));
    }
}
