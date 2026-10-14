//! State namespaces are a build property, independent of optimization and bundle location.
//! Path selection is safe before the GUI starts. It does no filesystem I/O, except that on
//! Linux one existence check, made once per root, keeps a root in its pre-XDG location.

use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
    Production,
    Development,
    Preview,
}

impl Channel {
    pub(crate) const fn app_name(self) -> &'static str {
        match self {
            Self::Production => "Omperor",
            Self::Development | Self::Preview => "Omperor Dev",
        }
    }

    pub(crate) const fn current() -> Self {
        if cfg!(feature = "design-preview") {
            Self::Preview
        } else if cfg!(feature = "production") {
            Self::Production
        } else {
            Self::Development
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("HOME is unavailable")]
    HomeUnavailable,
    #[error("OMPEROR_STATE_DIR must be an absolute directory path: {0}")]
    RelativeOverride(PathBuf),
}

/// Explicit test roots override channel defaults, but previews never read them.
pub(crate) fn root_for(channel: Channel) -> Result<PathBuf, Error> {
    resolve(
        channel,
        env::var_os("HOME").as_deref(),
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var_os("OMPEROR_STATE_DIR").as_deref(),
        &env::temp_dir(),
        &legacy_root_exists,
    )
}

/// Whether a Linux root is still in the macOS-style location that Linux
/// builds used before they followed XDG. Asked once per root, so a process
/// never changes its mind about where its state lives.
fn legacy_root_exists(legacy: &Path) -> bool {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };
    static ANSWERS: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let mut answers = ANSWERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *answers
        .entry(legacy.to_path_buf())
        .or_insert_with(|| legacy.is_dir())
}

pub(crate) fn root() -> Result<PathBuf, Error> {
    root_for(Channel::current())
}

/// Evals never default to the installed app's accounts or mutable state, even
/// when invoked through its executable. An explicit state root remains usable.
pub(crate) fn eval_root() -> Result<PathBuf, Error> {
    root_for(if cfg!(feature = "design-preview") {
        Channel::Preview
    } else {
        Channel::Development
    })
}

/// A headless, read-only diagnostic for packaging and test harnesses.
pub(crate) fn dispatch(args: &[std::ffi::OsString]) -> Option<i32> {
    if args.first().is_none_or(|arg| arg != "--state-paths") {
        return None;
    }
    if args.len() != 1 {
        eprintln!("Usage: omperor --state-paths");
        return Some(2);
    }
    Some(match root().and_then(|root| Ok((root, eval_root()?))) {
        Ok((root, eval_root)) => {
            let channel = match Channel::current() {
                Channel::Production => "production",
                Channel::Development => "development",
                Channel::Preview => "preview",
            };
            println!(
                "{}",
                serde_json::json!({
                    "schema_version": 1, "channel": channel, "root": root,
                    "preferences": root.join("omperor.sqlite3"),
                    "sessions": root.join("omp/sessions"), "data": root.join("omp/data"),
                    "archives": root.join("archives"), "worktrees": root.join("worktrees"),
                    "logs": root.join("logs"),
                    "eval_root": eval_root,
                })
            );
            0
        }
        Err(error) => {
            eprintln!("{error}");
            2
        }
    })
}

fn resolve(
    channel: Channel,
    home: Option<&OsStr>,
    data_home: Option<&OsStr>,
    explicit: Option<&OsStr>,
    temporary: &Path,
    legacy_exists: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, Error> {
    if channel == Channel::Preview {
        return Ok(temporary.join("omperor-design-preview"));
    }
    if let Some(explicit) = explicit.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(explicit);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err(Error::RelativeOverride(path))
        };
    }
    let home = home
        .filter(|value| !value.is_empty())
        .ok_or(Error::HomeUnavailable)?;
    let production = channel == Channel::Production;
    let name = if production { "Omperor" } else { "Omperor Dev" };
    let apple = Path::new(home).join("Library/Application Support").join(name);
    if cfg!(target_os = "macos") {
        return Ok(apple);
    }
    // Linux follows the XDG base directories. Roots that Linux builds made
    // in the macOS location before then stay where they are.
    if legacy_exists(&apple) {
        return Ok(apple);
    }
    let data_home = data_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| Path::new(home).join(".local/share"));
    Ok(data_home.join(if production { "omperor" } else { "omperor-dev" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never(_: &Path) -> bool {
        false
    }

    fn defaults(channel: Channel, home: Option<&OsStr>) -> Result<PathBuf, Error> {
        resolve(channel, home, None, None, Path::new("/tmp"), &never)
    }

    #[test]
    fn channels_keep_all_owned_state_in_disjoint_roots() {
        let home = Some(OsStr::new("/home/test"));
        let production = defaults(Channel::Production, home).unwrap();
        let development = defaults(Channel::Development, home).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(
                production,
                Path::new("/home/test/Library/Application Support/Omperor")
            );
            assert_eq!(
                development,
                Path::new("/home/test/Library/Application Support/Omperor Dev")
            );
        } else {
            assert_eq!(production, Path::new("/home/test/.local/share/omperor"));
            assert_eq!(development, Path::new("/home/test/.local/share/omperor-dev"));
        }
        for owned in [
            "omperor.sqlite3",
            "omp/data",
            "omp/sessions",
            "worktrees",
            "archives",
        ] {
            assert_ne!(production.join(owned), development.join(owned));
            assert!(!development.join(owned).starts_with(&production));
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_follows_xdg_data_home_and_keeps_existing_roots_in_place() {
        let home = Some(OsStr::new("/home/test"));
        let data_home = Some(OsStr::new("/data"));
        let root = |data_home, legacy: &dyn Fn(&Path) -> bool| {
            resolve(Channel::Development, home, data_home, None, Path::new("/tmp"), legacy)
                .unwrap()
        };
        assert_eq!(root(data_home, &never), Path::new("/data/omperor-dev"));
        // A relative XDG_DATA_HOME is invalid and ignored, as the spec says.
        assert_eq!(
            root(Some(OsStr::new("relative")), &never),
            Path::new("/home/test/.local/share/omperor-dev")
        );
        let legacy = Path::new("/home/test/Library/Application Support/Omperor Dev");
        assert_eq!(root(data_home, &|path| path == legacy), legacy);
    }

    #[test]
    fn explicit_test_roots_are_absolute_and_do_not_need_a_home() {
        let explicit = |channel, home, value| {
            resolve(channel, home, None, Some(OsStr::new(value)), Path::new("/tmp"), &never)
        };
        assert_eq!(
            explicit(Channel::Development, None, "/tmp/run-state").unwrap(),
            Path::new("/tmp/run-state")
        );
        assert!(explicit(Channel::Production, None, "relative").is_err());
        assert!(defaults(Channel::Development, Some(OsStr::new(""))).is_err());
    }

    #[test]
    fn preview_never_inherits_a_real_state_root() {
        let root = resolve(
            Channel::Preview,
            None,
            None,
            Some(OsStr::new("/real/state")),
            Path::new("/tmp"),
            &|_| true,
        )
        .unwrap();
        assert_eq!(root, Path::new("/tmp/omperor-design-preview"));
    }
}
