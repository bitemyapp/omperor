//! Platform filesystem watchers report changes from background threads.
//! Both backends use the index walk's gitignore rules.
use std::path::{Component, Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

#[path = "file_watcher_linux.rs"]
mod backend;

pub(crate) use backend::{ProjectWatcher, current_event_id};

/// Decides which changes deserve a refresh, by the same rules git uses to
/// decide what a checkout tracks: the user's global excludes, then each
/// `.gitignore` the index walk honored, scoped to its own directory, with the
/// deepest applicable file having the last word.
#[derive(Debug)]
pub(crate) struct ChangeFilter {
    root: PathBuf,
    /// Shallowest first, so a later (deeper) match overrides an earlier one.
    matchers: Vec<Gitignore>,
}

impl ChangeFilter {
    /// Reads the `.gitignore` of every directory in `ignore_dirs` (shallowest
    /// first, as `index_project_files` reports them), a repository's own
    /// `.git/info/exclude` where present, and the global excludes. Does file
    /// I/O, so it belongs on a worker thread.
    pub(crate) fn build(root: &Path, ignore_dirs: &[PathBuf]) -> Self {
        let mut matchers = Vec::with_capacity(ignore_dirs.len() + 2);
        // Global excludes apply everywhere at the lowest precedence; rooting
        // them at the project makes their patterns match as a root
        // `.gitignore`'s would.
        let (global, _) = GitignoreBuilder::new(root).build_global();
        if !global.is_empty() {
            matchers.push(global);
        }
        let mut excludes = GitignoreBuilder::new(root);
        excludes.add(root.join(".git/info/exclude"));
        if let Ok(excludes) = excludes.build()
            && !excludes.is_empty()
        {
            matchers.push(excludes);
        }
        for dir in ignore_dirs {
            let mut builder = GitignoreBuilder::new(dir);
            // A missing or unreadable file simply contributes nothing.
            builder.add(dir.join(".gitignore"));
            if dir != root {
                // A nested checkout's own excludes, if it is a full repository.
                builder.add(dir.join(".git/info/exclude"));
            }
            if let Ok(matcher) = builder.build()
                && !matcher.is_empty()
            {
                matchers.push(matcher);
            }
        }
        Self {
            root: root.to_owned(),
            matchers,
        }
    }

    /// Whether a change at the absolute `path` should refresh the workspace.
    /// `.git/index` and its lock never count: every `git status`, including
    /// the one the inspection itself runs, rewrites them, so counting them
    /// would loop. The rest of `.git` does count, so a commit or checkout in a
    /// terminal updates the branch and change list. Ignored paths never count,
    /// so a build writing under `target/` stays silent.
    pub(crate) fn is_relevant(&self, path: &Path, is_dir: bool) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            // Not under the root at all (the root itself): refresh.
            return true;
        };
        if git_index_write(relative) {
            return false;
        }
        let mut ignored = false;
        for matcher in &self.matchers {
            // A file governs only its own subtree; `strip` inside the matcher
            // makes the absolute path relative to that directory.
            if !path.starts_with(matcher.path()) {
                continue;
            }
            match matcher.matched_path_or_any_parents(path, is_dir) {
                Match::Ignore(_) => ignored = true,
                Match::Whitelist(_) => ignored = false,
                Match::None => {}
            }
        }
        !ignored
    }
}

/// Whether `relative` is an index file of the project's repository or of a
/// nested one (`.git/index`, `.git/modules/<name>/index`, and their locks).
fn git_index_write(relative: &Path) -> bool {
    relative
        .components()
        .any(|component| component == Component::Normal(".git".as_ref()))
        && matches!(
            relative.file_name().and_then(|name| name.to_str()),
            Some("index" | "index.lock")
        )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{ChangeFilter, git_index_write};

    #[test]
    fn git_index_writes_never_count() {
        // Our own `git status` rewrites the index; a commit or checkout
        // touches HEAD and the reflog, which do count.
        assert!(git_index_write(Path::new(".git/index")));
        assert!(git_index_write(Path::new(".git/index.lock")));
        assert!(git_index_write(Path::new(".git/modules/vendor/index")));
        assert!(git_index_write(Path::new("sub/.git/index.lock")));
        assert!(!git_index_write(Path::new(".git/HEAD")));
        assert!(!git_index_write(Path::new(".git/logs/HEAD")));
        assert!(!git_index_write(Path::new(".git/refs/heads/main")));
        assert!(!git_index_write(Path::new("src/index.rs")));
    }

    #[test]
    fn nested_ignore_files_are_scoped_and_the_deepest_wins() {
        let root = std::env::temp_dir().join(format!("omperor-watch-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, contents: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
            fs::write(path, contents).expect("fixture file");
        };
        write(".gitignore", "build-output/\n*.scratchtmp\n");
        write("sub/.gitignore", "*.scratchlog\n!keep.scratchtmp\n");
        let filter = ChangeFilter::build(&root, &[root.clone(), root.join("sub")]);
        let relevant =
            |relative: &str, is_dir: bool| filter.is_relevant(&root.join(relative), is_dir);

        // Root rules apply everywhere.
        assert!(!relevant("build-output/artifact", false));
        assert!(!relevant("build-output", true));
        assert!(!relevant("note.scratchtmp", false));
        assert!(!relevant("sub/deep/note.scratchtmp", false));
        // A nested file governs only its own subtree...
        assert!(!relevant("sub/run.scratchlog", false));
        assert!(relevant("run.scratchlog", false));
        // ...and, being deeper, overrides the root for paths it whitelists.
        assert!(relevant("sub/keep.scratchtmp", false));
        // Ordinary paths, and the ignore files themselves, count.
        assert!(relevant("src/main.rs", false));
        assert!(relevant("sub/.gitignore", false));
        // The index never counts; the rest of `.git` does.
        assert!(!relevant(".git/index", false));
        assert!(relevant(".git/HEAD", false));

        let _ = fs::remove_dir_all(&root);
    }
}
