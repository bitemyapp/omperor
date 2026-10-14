//! Opening the project, or one file in it, in the user's editor or terminal,
//! and a sign-in page in their browser.
//!
//! Omperor does not embed an editor, a terminal, or a browser: the user
//! already has the ones they like, so the app's job is to hand them the
//! directory, file, or page in one keystroke. This module knows which
//! applications are installed, how each one wants to be pointed at a target
//! from the command line, and which to use when no preference has been
//! stored.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LauncherKind {
    Editor,
    Terminal,
    Browser,
}

impl LauncherKind {
    pub(crate) const fn noun(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Terminal => "terminal",
            Self::Browser => "browser",
        }
    }
}

/// How an application opens a path when asked through `open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recipe {
    /// `open -a <App> <path>`: the application opens the path itself, which
    /// every editor and a few terminals support.
    OpenWith,
    /// `open -na <App> --args <arguments>`: a new instance, told where to start
    /// through its own flags; `{dir}` in an argument is replaced by the path.
    NewInstance(&'static [&'static str]),
}

/// One application Omperor knows how to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Launcher {
    /// The application's name, as its bundle is named under `/Applications`.
    pub(crate) name: &'static str,
    pub(crate) kind: LauncherKind,
    recipe: Recipe,
}

const fn editor(name: &'static str) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Editor,
        recipe: Recipe::OpenWith,
    }
}

const fn terminal(name: &'static str, recipe: Recipe) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Terminal,
        recipe,
    }
}

const fn browser(name: &'static str) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Browser,
        recipe: Recipe::OpenWith,
    }
}

/// Editors in the order they are preferred when nothing is stored: the
/// lighter, faster ones first, the IDEs after.
pub(crate) const EDITORS: &[Launcher] = &[
    editor("Zed"),
    editor("Cursor"),
    editor("Visual Studio Code"),
    editor("Sublime Text"),
    editor("Nova"),
    editor("BBEdit"),
    editor("TextMate"),
    editor("Antigravity"),
    editor("Xcode"),
    editor("IntelliJ IDEA"),
    editor("RustRover"),
    editor("Emacs"),
];

/// Terminals in preference order, ending with the one every Mac has.
pub(crate) const TERMINALS: &[Launcher] = &[
    terminal(
        "Ghostty",
        Recipe::NewInstance(&["--working-directory={dir}"]),
    ),
    terminal("iTerm", Recipe::OpenWith),
    terminal("WezTerm", Recipe::NewInstance(&["start", "--cwd", "{dir}"])),
    terminal("kitty", Recipe::NewInstance(&["--directory", "{dir}"])),
    terminal(
        "Alacritty",
        Recipe::NewInstance(&["--working-directory", "{dir}"]),
    ),
    terminal("Warp", Recipe::OpenWith),
    terminal("Terminal", Recipe::OpenWith),
];

/// Browsers Omperor can be told to use for a sign-in page. There is no
/// preference order here: with nothing stored, the page opens in the
/// system's default browser, whichever that is.
pub(crate) const BROWSERS: &[Launcher] = &[
    browser("Safari"),
    browser("Google Chrome"),
    browser("Firefox"),
    browser("Arc"),
    browser("Brave Browser"),
    browser("Microsoft Edge"),
    browser("Orion"),
    browser("Vivaldi"),
    browser("Zen"),
    browser("Opera"),
];

impl Launcher {
    pub(crate) fn candidates(kind: LauncherKind) -> &'static [Launcher] {
        match kind {
            LauncherKind::Editor => EDITORS,
            LauncherKind::Terminal => TERMINALS,
            LauncherKind::Browser => BROWSERS,
        }
    }

    #[cfg(test)]
    pub(crate) fn named(kind: LauncherKind, name: &str) -> Option<Launcher> {
        Self::candidates(kind)
            .iter()
            .copied()
            .find(|launcher| launcher.name == name)
    }

    /// Where the application's bundle may live; the first that exists wins.
    fn bundle_candidates(&self, home: Option<&Path>) -> Vec<PathBuf> {
        let bundle = format!("{}.app", self.name);
        let mut candidates = vec![
            PathBuf::from("/Applications").join(&bundle),
            PathBuf::from("/System/Applications/Utilities").join(&bundle),
            PathBuf::from("/System/Applications").join(&bundle),
        ];
        if let Some(home) = home {
            candidates.push(home.join("Applications").join(&bundle));
        }
        candidates
    }

    pub(crate) fn is_installed(&self) -> bool {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        self.bundle_candidates(home.as_deref())
            .iter()
            .any(|path| path.is_dir())
    }

    /// The `open` invocation that points the application at `target`.
    pub(crate) fn open_command(&self, target: &Path) -> Vec<String> {
        let target = target.to_string_lossy().into_owned();
        match self.recipe {
            Recipe::OpenWith => vec!["-a".into(), self.name.into(), target],
            Recipe::NewInstance(arguments) => {
                let mut command = vec!["-na".into(), self.name.into(), "--args".into()];
                command.extend(
                    arguments
                        .iter()
                        .map(|argument| argument.replace("{dir}", &target)),
                );
                command
            }
        }
    }

    /// Opens `target` in the application. Waits for `open` to hand the path
    /// over, which can take a moment while the app launches, so this belongs
    /// on a worker, never on the main thread.
    pub(crate) fn launch(&self, target: &Path) -> Result<(), String> {
        self.run_open(self.open_command(target), &target.display().to_string())
    }

    /// Opens a web page in the application, which every browser takes the
    /// same way a file is taken. On a worker, as `launch` is.
    pub(crate) fn launch_url(&self, url: &str) -> Result<(), String> {
        self.run_open(vec!["-a".into(), self.name.into(), url.into()], url)
    }

    fn run_open(&self, arguments: Vec<String>, target: &str) -> Result<(), String> {
        let output = Command::new("/usr/bin/open")
            .args(arguments)
            .output()
            .map_err(|error| format!("could not run open: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        Err(if detail.is_empty() {
            format!("{} did not accept {target}", self.name)
        } else {
            detail.to_owned()
        })
    }
}

/// Every candidate of `kind` that is installed, in preference order.
pub(crate) fn installed(kind: LauncherKind) -> Vec<Launcher> {
    Launcher::candidates(kind)
        .iter()
        .copied()
        .filter(Launcher::is_installed)
        .collect()
}

/// The application to use: the stored preference when it is still installed,
/// otherwise the first installed candidate.
pub(crate) fn choose(kind: LauncherKind, preference: Option<&str>) -> Option<Launcher> {
    choose_among(&installed(kind), preference)
}

fn choose_among(available: &[Launcher], preference: Option<&str>) -> Option<Launcher> {
    preference
        .and_then(|name| {
            available
                .iter()
                .copied()
                .find(|launcher| launcher.name == name)
        })
        .or_else(|| available.first().copied())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{BROWSERS, EDITORS, Launcher, LauncherKind, TERMINALS, choose_among};

    #[test]
    fn browsers_are_handed_a_page_the_way_editors_are_handed_a_file() {
        assert!(
            BROWSERS
                .iter()
                .all(|browser| browser.kind == LauncherKind::Browser)
        );
        let safari = Launcher::named(LauncherKind::Browser, "Safari").expect("Safari is known");
        assert_eq!(
            safari.open_command(Path::new("https://auth.example/authorize?state=1")),
            vec!["-a", "Safari", "https://auth.example/authorize?state=1"]
        );
    }

    #[test]
    fn editors_open_the_path_with_the_app() {
        let zed = Launcher::named(LauncherKind::Editor, "Zed").expect("Zed is a known editor");
        assert_eq!(
            zed.open_command(Path::new("/work/omperor")),
            vec!["-a", "Zed", "/work/omperor"]
        );
        let file = zed.open_command(Path::new("/work/omperor/src/main.rs"));
        assert_eq!(file[2], "/work/omperor/src/main.rs");
    }

    #[test]
    fn terminals_start_a_new_instance_in_the_directory() {
        let ghostty = Launcher::named(LauncherKind::Terminal, "Ghostty").expect("Ghostty is known");
        assert_eq!(
            ghostty.open_command(Path::new("/work/omperor")),
            vec![
                "-na",
                "Ghostty",
                "--args",
                "--working-directory=/work/omperor"
            ]
        );
        let wezterm = Launcher::named(LauncherKind::Terminal, "WezTerm").expect("WezTerm is known");
        assert_eq!(
            wezterm.open_command(Path::new("/work/x y")),
            vec!["-na", "WezTerm", "--args", "start", "--cwd", "/work/x y"]
        );
        let terminal =
            Launcher::named(LauncherKind::Terminal, "Terminal").expect("Terminal is known");
        assert_eq!(
            terminal.open_command(Path::new("/work")),
            vec!["-a", "Terminal", "/work"]
        );
    }

    #[test]
    fn bundles_are_looked_up_in_the_usual_places() {
        let terminal =
            Launcher::named(LauncherKind::Terminal, "Terminal").expect("Terminal is known");
        let candidates = terminal.bundle_candidates(Some(Path::new("/Users/me")));
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/Applications/Terminal.app"),
                PathBuf::from("/System/Applications/Utilities/Terminal.app"),
                PathBuf::from("/System/Applications/Terminal.app"),
                PathBuf::from("/Users/me/Applications/Terminal.app"),
            ]
        );
    }

    #[test]
    fn the_stored_preference_wins_while_it_is_installed() {
        let available = [TERMINALS[0], TERMINALS[6]];
        assert_eq!(
            choose_among(&available, Some("Terminal")).map(|launcher| launcher.name),
            Some("Terminal")
        );
        assert_eq!(
            choose_among(&available, Some("Gone")).map(|launcher| launcher.name),
            Some("Ghostty")
        );
        assert_eq!(
            choose_among(&available, None).map(|launcher| launcher.name),
            Some("Ghostty")
        );
        assert_eq!(choose_among(&[], Some("Zed")), None);
        assert!(
            EDITORS
                .iter()
                .all(|launcher| launcher.kind == LauncherKind::Editor)
        );
        assert!(
            TERMINALS
                .iter()
                .all(|launcher| launcher.kind == LauncherKind::Terminal)
        );
    }
}
