//! State namespaces are a build property (the `production` feature), independent of
//! optimization. Path selection is safe before the GUI starts: it reads the environment
//! and does no filesystem I/O.

use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
    Production,
    Development,
}

impl Channel {
    pub(crate) const fn app_name(self) -> &'static str {
        match self {
            Self::Production => "Omperor",
            Self::Development => "Omperor Dev",
        }
    }

    pub(crate) const fn current() -> Self {
        if cfg!(feature = "production") {
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

/// `OMPEROR_STATE_DIR`, when set, overrides the channel's default root.
pub(crate) fn root_for(channel: Channel) -> Result<PathBuf, Error> {
    resolve(
        channel,
        env::var_os("HOME").as_deref(),
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var_os("OMPEROR_STATE_DIR").as_deref(),
    )
}

pub(crate) fn root() -> Result<PathBuf, Error> {
    root_for(Channel::current())
}

/// Evals never default to the installed app's accounts or mutable state, even
/// when invoked through its executable. An explicit state root remains usable.
pub(crate) fn eval_root() -> Result<PathBuf, Error> {
    root_for(Channel::Development)
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
) -> Result<PathBuf, Error> {
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
    let data_home = data_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| Path::new(home).join(".local/share"));
    Ok(data_home.join(if production { "omperor" } else { "omperor-dev" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(channel: Channel, home: Option<&OsStr>) -> Result<PathBuf, Error> {
        resolve(channel, home, None, None)
    }

    #[test]
    fn channels_keep_all_owned_state_in_disjoint_roots() {
        let home = Some(OsStr::new("/home/test"));
        let production = defaults(Channel::Production, home).unwrap();
        let development = defaults(Channel::Development, home).unwrap();
        assert_eq!(production, Path::new("/home/test/.local/share/omperor"));
        assert_eq!(development, Path::new("/home/test/.local/share/omperor-dev"));
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
    fn roots_follow_xdg_data_home() {
        let home = Some(OsStr::new("/home/test"));
        let root = |data_home| {
            resolve(Channel::Development, home, data_home, None).unwrap()
        };
        assert_eq!(root(Some(OsStr::new("/data"))), Path::new("/data/omperor-dev"));
        // A relative XDG_DATA_HOME is invalid and ignored, as the spec says.
        assert_eq!(
            root(Some(OsStr::new("relative"))),
            Path::new("/home/test/.local/share/omperor-dev")
        );
    }

    #[test]
    fn explicit_test_roots_are_absolute_and_do_not_need_a_home() {
        let explicit = |channel, home, value| {
            resolve(channel, home, None, Some(OsStr::new(value)))
        };
        assert_eq!(
            explicit(Channel::Development, None, "/tmp/run-state").unwrap(),
            Path::new("/tmp/run-state")
        );
        assert!(explicit(Channel::Production, None, "relative").is_err());
        assert!(defaults(Channel::Development, Some(OsStr::new(""))).is_err());
    }
}
