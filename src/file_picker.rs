//! The composer's `@` file-reference picker: indexing a project's files and
//! ranking them against a partial query. Pure logic with no AppKit, so it runs
//! on the worker pool and can be tested (and benchmarked) on its own.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::UNIX_EPOCH;

/// One file in a project's `@` index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectFile {
    /// Path relative to the project root.
    pub(crate) path: String,
    /// How many directories deep the file sits (the `/` count), cached so the
    /// browse order never rescans paths while sorting.
    depth: u32,
    /// Modification time in milliseconds since the Unix epoch; 0 when the
    /// metadata could not be read.
    modified: u64,
}

impl ProjectFile {
    pub(crate) fn new(path: String, modified: u64) -> Self {
        let depth = u32::try_from(path.matches('/').count()).unwrap_or(u32::MAX);
        Self {
            path,
            depth,
            modified,
        }
    }
}

/// What one walk of a project yields: its files in browse order, and the
/// directories whose `.gitignore` the walk honored (shallowest first), so the
/// change watcher can apply the very same rules without a second walk.
#[derive(Debug, Default)]
pub(crate) struct ProjectIndex {
    pub(crate) files: Vec<ProjectFile>,
    pub(crate) ignore_dirs: Vec<PathBuf>,
}

/// Browse order, what a bare `@` offers: most recently modified first (what
/// the user is most likely about to refer to), then shallowest, then by path,
/// so a fresh checkout whose files share one timestamp reads as a root listing.
pub(crate) fn browse_order(left: &ProjectFile, right: &ProjectFile) -> Ordering {
    right
        .modified
        .cmp(&left.modified)
        .then(left.depth.cmp(&right.depth))
        .then_with(|| left.path.cmp(&right.path))
}

/// Lists a project's files as paths relative to `directory`, honoring
/// `.gitignore` but descending into every subdirectory — including nested git
/// repositories and submodules (`git ls-files` stops at those and reports only
/// the gitlink, so a walk, like `fd`, is needed to reach the files inside).
/// Hidden files are skipped, matching `fd`'s default. The walk is parallel
/// because every file is also stat-ed for its modification time, which would
/// double a serial walk's time on a large monorepo. The files come back in
/// browse order, sorted once here rather than on every empty query.
pub(crate) fn index_project_files(directory: &Path) -> ProjectIndex {
    let walker = ignore::WalkBuilder::new(directory)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .build_parallel();
    let files = Mutex::new(Vec::new());
    let ignore_dirs = Mutex::new(Vec::new());
    walker.run(|| {
        Box::new(|entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            let kind = entry.file_type();
            if kind.is_some_and(|kind| kind.is_dir()) {
                // A directory's own `.gitignore` is hidden, so the walk never
                // yields it as an entry; probe for it. Only directories the
                // walk actually enters are probed, so a `.gitignore` under an
                // ignored tree is never consulted — exactly as git does.
                if entry.path().join(".gitignore").is_file() {
                    ignore_dirs
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(entry.path().to_path_buf());
                }
                return ignore::WalkState::Continue;
            }
            if !kind.is_some_and(|kind| kind.is_file()) {
                return ignore::WalkState::Continue;
            }
            let Some(path) = entry
                .path()
                .strip_prefix(directory)
                .ok()
                .and_then(|relative| relative.to_str())
            else {
                return ignore::WalkState::Continue;
            };
            let modified = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |age| u64::try_from(age.as_millis()).unwrap_or(u64::MAX));
            let mut files = files.lock().unwrap_or_else(PoisonError::into_inner);
            files.push(ProjectFile::new(path.to_owned(), modified));
            // A ceiling only to bound memory on a pathological tree; large
            // monorepos (tens of thousands of files, vendored submodules and
            // all) index in full.
            if files.len() >= 200_000 {
                ignore::WalkState::Quit
            } else {
                ignore::WalkState::Continue
            }
        })
    });
    let mut files = files.into_inner().unwrap_or_else(PoisonError::into_inner);
    files.sort_unstable_by(browse_order);
    files.dedup_by(|left, right| left.path == right.path);
    let mut ignore_dirs = ignore_dirs
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    // Path order puts every directory before its descendants.
    ignore_dirs.sort();
    ProjectIndex { files, ignore_dirs }
}

/// Ranks project files against a partial `@` query, best matches first and
/// capped at `limit`. An empty query offers the index's leading entries, which
/// `index_project_files` keeps in browse order.
pub(crate) fn filter_project_files(
    files: &[ProjectFile],
    query: &str,
    limit: usize,
) -> Vec<String> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return files
            .iter()
            .take(limit)
            .map(|file| file.path.clone())
            .collect();
    }
    let mut scored: Vec<(u8, usize, &str)> = files
        .iter()
        .filter_map(|file| {
            let lower = file.path.to_lowercase();
            let base = lower.rsplit('/').next().unwrap_or(lower.as_str());
            let score = if base.starts_with(&query) {
                0
            } else if base.contains(&query) {
                1
            } else if lower.contains(&query) {
                2
            } else if is_subsequence(&query, &lower) {
                3
            } else {
                return None;
            };
            Some((score, file.path.len(), file.path.as_str()))
        })
        .collect();
    scored.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(right.2))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, path)| path.to_owned())
        .collect()
}

/// Whether `needle`'s characters appear in order within `haystack`.
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|target| chars.any(|char| char == target))
}

/// Extracts the active `@` reference token ending at the caret from the text
/// before it: returns the query (after `@`) and the UTF-16 location of the `@`.
/// `None` when there is no `@` word at the caret.
pub(crate) fn active_at_query(prefix: &str, caret: usize) -> Option<(String, usize)> {
    let at_byte = prefix.rfind('@')?;
    let query = &prefix[at_byte + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    // The `@` must start a word (preceded by whitespace or the start), so an
    // email address does not trigger the picker.
    let starts_word = prefix[..at_byte]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    if !starts_word {
        return None;
    }
    let token_utf16 = prefix[at_byte..].encode_utf16().count();
    let query_start = caret.checked_sub(token_utf16)?;
    Some((query.to_owned(), query_start))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        ProjectFile, active_at_query, browse_order, filter_project_files, index_project_files,
    };

    fn file(path: &str, modified: u64) -> ProjectFile {
        ProjectFile::new(path.to_owned(), modified)
    }

    #[test]
    fn at_query_only_triggers_on_a_word_starting_at_signal() {
        // A trailing "@token" is a query; the '@' must start a word.
        assert_eq!(
            active_at_query("read @src/ma", 12),
            Some(("src/ma".to_owned(), 5))
        );
        assert_eq!(active_at_query("@", 1), Some((String::new(), 0)));
        // Whitespace after '@' ends the token; an email '@' does not start one.
        assert_eq!(active_at_query("@src now", 8), None);
        assert_eq!(active_at_query("me@host", 7), None);
        assert_eq!(active_at_query("no reference here", 17), None);
    }

    #[test]
    fn file_filter_ranks_basename_matches_first() {
        let files = vec![
            file("src/main.rs", 0),
            file("src/chat.rs", 0),
            file("docs/mainframe.md", 0),
            file("tests/main_helpers.rs", 0),
        ];
        let matches = filter_project_files(&files, "main", 10);
        // Basename-prefix ("main.rs", "main_helpers.rs") outrank a basename
        // substring ("mainframe.md"); "chat.rs" does not match at all.
        assert_eq!(matches[0], "src/main.rs");
        assert!(matches.contains(&"tests/main_helpers.rs".to_owned()));
        assert!(!matches.contains(&"src/chat.rs".to_owned()));

        // A subsequence still matches when nothing better does.
        assert_eq!(filter_project_files(&files, "cht", 10), vec!["src/chat.rs"]);
        // The limit is honored.
        assert_eq!(filter_project_files(&files, "", 2).len(), 2);
    }

    #[test]
    fn browse_order_is_recent_then_shallow_then_path() {
        let mut files = vec![
            file("src/deep/old.rs", 1_000),
            file("README.md", 1_000),
            file("src/edited.rs", 5_000),
            file("Cargo.toml", 1_000),
        ];
        files.sort_unstable_by(browse_order);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        // The newest file leads regardless of depth; among equal timestamps
        // shallow paths precede deep ones and ties fall back to path order.
        assert_eq!(
            paths,
            [
                "src/edited.rs",
                "Cargo.toml",
                "README.md",
                "src/deep/old.rs"
            ]
        );
        // An empty query is a prefix of that order.
        assert_eq!(
            filter_project_files(&files, "", 2),
            vec!["src/edited.rs", "Cargo.toml"]
        );
    }

    #[test]
    fn index_scopes_nested_ignore_files_and_reports_them() {
        let root = std::env::temp_dir().join(format!("omperor-index-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // An empty `.git` is enough for the walker to treat this as a
        // repository and honor its ignore files.
        fs::create_dir_all(root.join(".git")).expect("fixture");
        let write = |relative: &str, contents: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
            fs::write(path, contents).expect("fixture file");
        };
        write(".gitignore", "build-output/\n");
        write("build-output/artifact.txt", "");
        write("src/main.rs", "");
        write("sub/.gitignore", "*.scratch\n");
        write("sub/notes.scratch", "");
        write("sub/lib.rs", "");
        write("root.scratch", "");

        let index = index_project_files(&root);
        let mut paths: Vec<&str> = index.files.iter().map(|file| file.path.as_str()).collect();
        paths.sort_unstable();
        // `*.scratch` is scoped to `sub/`; `build-output/` is root-wide.
        assert_eq!(paths, ["root.scratch", "src/main.rs", "sub/lib.rs"]);
        assert_eq!(index.ignore_dirs, vec![root.clone(), root.join("sub")]);

        let _ = fs::remove_dir_all(&root);
    }
}
