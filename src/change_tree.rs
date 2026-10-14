//! The change list as a tree: files under the directories they live in, the
//! way a review reads, instead of a flat list of paths. Single-child
//! directory chains collapse into one row (`src/layer/`), a collapsed
//! directory shows how many files and lines it holds, and directories sort
//! before files as they do in every file browser.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use crate::workspace::WorkspaceChange;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeRow {
    Directory {
        /// The directory relative to the repository, the collapse key.
        path: PathBuf,
        /// The row's label: one segment, or a collapsed chain like `a/b`.
        name: String,
        depth: usize,
        collapsed: bool,
        /// Changed files beneath it, at any depth.
        files: usize,
        insertions: Option<u64>,
        deletions: Option<u64>,
    },
    File {
        /// Index into the snapshot's changes.
        change: usize,
        name: String,
        depth: usize,
    },
}

#[derive(Default)]
struct Node {
    directories: BTreeMap<String, Node>,
    files: Vec<(String, usize)>,
}

impl Node {
    fn insert(&mut self, components: &[String], name: String, change: usize) {
        match components.split_first() {
            Some((head, rest)) => self
                .directories
                .entry(head.clone())
                .or_default()
                .insert(rest, name, change),
            None => self.files.push((name, change)),
        }
    }

    fn totals(&self, changes: &[WorkspaceChange]) -> (usize, Option<u64>, Option<u64>) {
        let mut files = 0;
        let mut insertions = None;
        let mut deletions = None;
        for (_, change) in &self.files {
            files += 1;
            if let Some(change) = changes.get(*change) {
                if let Some(added) = change.insertions {
                    insertions = Some(insertions.unwrap_or(0) + added);
                }
                if let Some(removed) = change.deletions {
                    deletions = Some(deletions.unwrap_or(0) + removed);
                }
            }
        }
        for child in self.directories.values() {
            let (child_files, child_insertions, child_deletions) = child.totals(changes);
            files += child_files;
            if let Some(added) = child_insertions {
                insertions = Some(insertions.unwrap_or(0) + added);
            }
            if let Some(removed) = child_deletions {
                deletions = Some(deletions.unwrap_or(0) + removed);
            }
        }
        (files, insertions, deletions)
    }
}

/// Lays the changes out as rows, honoring `collapsed` directories.
pub(crate) fn tree_rows(changes: &[WorkspaceChange], collapsed: &HashSet<PathBuf>) -> Vec<TreeRow> {
    let mut root = Node::default();
    for (index, change) in changes.iter().enumerate() {
        let mut components: Vec<String> = change
            .path
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some(name) = components.pop() else {
            continue;
        };
        root.insert(&components, name, index);
    }
    let mut rows = Vec::new();
    emit(&root, Path::new(""), 0, changes, collapsed, &mut rows);
    rows
}

fn emit(
    node: &Node,
    prefix: &Path,
    depth: usize,
    changes: &[WorkspaceChange],
    collapsed: &HashSet<PathBuf>,
    rows: &mut Vec<TreeRow>,
) {
    for (segment, child) in &node.directories {
        // Walk down single-child chains so `a/b/c/file` reads as one row
        // `a/b/c` rather than three nested rows with nothing in them.
        let mut name = segment.clone();
        let mut path = prefix.join(segment);
        let mut child = child;
        while child.files.is_empty() && child.directories.len() == 1 {
            let (next_segment, next_child) = child
                .directories
                .iter()
                .next()
                .expect("a single-child directory has one child");
            name.push('/');
            name.push_str(next_segment);
            path = path.join(next_segment);
            child = next_child;
        }
        let (files, insertions, deletions) = child.totals(changes);
        let is_collapsed = collapsed.contains(&path);
        rows.push(TreeRow::Directory {
            path: path.clone(),
            name,
            depth,
            collapsed: is_collapsed,
            files,
            insertions,
            deletions,
        });
        if !is_collapsed {
            emit(child, &path, depth + 1, changes, collapsed, rows);
        }
    }
    let mut files = node.files.clone();
    files.sort_by(|(left, _), (right, _)| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    for (name, change) in files {
        rows.push(TreeRow::File {
            change,
            name,
            depth,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, path::PathBuf};

    use super::{TreeRow, tree_rows};
    use crate::workspace::WorkspaceChange;

    fn change(path: &str, insertions: Option<u64>, deletions: Option<u64>) -> WorkspaceChange {
        WorkspaceChange {
            status: " M".into(),
            path: PathBuf::from(path),
            insertions,
            deletions,
        }
    }

    fn labels(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                TreeRow::Directory {
                    name,
                    depth,
                    collapsed,
                    files,
                    ..
                } => format!(
                    "{}{name}/ [{files}]{}",
                    "  ".repeat(*depth),
                    if *collapsed { " …" } else { "" }
                ),
                TreeRow::File { name, depth, .. } => format!("{}{name}", "  ".repeat(*depth)),
            })
            .collect()
    }

    #[test]
    fn directories_come_first_and_single_child_chains_collapse_into_one_row() {
        let changes = [
            change("src/main.rs", Some(10), Some(2)),
            change("README.md", Some(1), Some(0)),
            change("src/layer/attempt.rs", Some(5), Some(1)),
            change("docs/handoff/notes/today.md", None, None),
            change("src/Zebra.rs", Some(0), Some(3)),
        ];
        let rows = tree_rows(&changes, &HashSet::new());
        assert_eq!(
            labels(&rows),
            vec![
                "docs/handoff/notes/ [1]",
                "  today.md",
                "src/ [3]",
                "  layer/ [1]",
                "    attempt.rs",
                "  main.rs",
                "  Zebra.rs",
                "README.md",
            ]
        );
        let TreeRow::Directory {
            path,
            insertions,
            deletions,
            ..
        } = &rows[2]
        else {
            panic!("src is a directory row");
        };
        assert_eq!(path, &PathBuf::from("src"));
        assert_eq!((insertions, deletions), (&Some(15), &Some(6)));
        let TreeRow::Directory {
            path,
            insertions,
            deletions,
            ..
        } = &rows[0]
        else {
            panic!("docs chain is a directory row");
        };
        assert_eq!(path, &PathBuf::from("docs/handoff/notes"));
        assert_eq!((insertions, deletions), (&None, &None));
        let TreeRow::File { change, .. } = &rows[7] else {
            panic!("README is a file row");
        };
        assert_eq!(*change, 1);
    }

    #[test]
    fn a_collapsed_directory_hides_its_descendants_and_keeps_its_totals() {
        let changes = [
            change("src/main.rs", Some(10), Some(2)),
            change("src/layer/attempt.rs", Some(5), Some(1)),
            change("Cargo.toml", Some(1), Some(1)),
        ];
        let collapsed = HashSet::from([PathBuf::from("src")]);
        let rows = tree_rows(&changes, &collapsed);
        assert_eq!(labels(&rows), vec!["src/ [2] …", "Cargo.toml"]);
        let nested = HashSet::from([PathBuf::from("src/layer")]);
        assert_eq!(
            labels(&tree_rows(&changes, &nested)),
            vec!["src/ [2]", "  layer/ [1] …", "  main.rs", "Cargo.toml"]
        );
    }
}
