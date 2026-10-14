//! State namespaces are a build property, independent of optimization and bundle location.
//! Path selection does no filesystem I/O and is safe before the GUI starts.

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
        env::var_os("OMPEROR_STATE_DIR").as_deref(),
        &env::temp_dir(),
    )
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
    explicit: Option<&OsStr>,
    temporary: &Path,
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
    let name = if channel == Channel::Production {
        "Omperor"
    } else {
        "Omperor Dev"
    };
    Ok(Path::new(home)
        .join("Library/Application Support")
        .join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_keep_all_owned_state_in_disjoint_roots() {
        let home = Some(OsStr::new("/home/test"));
        let production = resolve(Channel::Production, home, None, Path::new("/tmp")).unwrap();
        let development = resolve(Channel::Development, home, None, Path::new("/tmp")).unwrap();
        assert_eq!(
            production,
            Path::new("/home/test/Library/Application Support/Omperor")
        );
        assert_eq!(
            development,
            Path::new("/home/test/Library/Application Support/Omperor Dev")
        );
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

    #[test]
    fn explicit_test_roots_are_absolute_and_do_not_need_a_home() {
        let root = resolve(
            Channel::Development,
            None,
            Some(OsStr::new("/tmp/run-state")),
            Path::new("/tmp"),
        )
        .unwrap();
        assert_eq!(root, Path::new("/tmp/run-state"));
        assert!(
            resolve(
                Channel::Production,
                None,
                Some(OsStr::new("relative")),
                Path::new("/tmp")
            )
            .is_err()
        );
        assert!(
            resolve(
                Channel::Development,
                Some(OsStr::new("")),
                None,
                Path::new("/tmp")
            )
            .is_err()
        );
    }

    #[test]
    fn preview_never_inherits_a_real_state_root() {
        let root = resolve(
            Channel::Preview,
            None,
            Some(OsStr::new("/real/state")),
            Path::new("/tmp"),
        )
        .unwrap();
        assert_eq!(root, Path::new("/tmp/omperor-design-preview"));
    }
}
