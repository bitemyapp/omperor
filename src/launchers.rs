//! Opening the project, or one file in it, in the user's editor or terminal,
//! and a sign-in page in their browser.
//!
//! Omperor does not embed an editor, a terminal, or a browser: the user
//! already has the ones they like, so the app's job is to hand them the
//! directory, file, or page in one keystroke. This module knows which
//! applications are installed, how each one wants to be pointed at a target
//! from the command line, and which to use when no preference has been
//! stored: programs on `PATH`, started directly.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// How an application is started on Linux: the first of `programs` found
/// on `PATH`, run with `args`, in which `{dir}` is replaced by the path
/// (or page). A terminal also starts in the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Recipe {
    programs: &'static [&'static str],
    args: &'static [&'static str],
}

/// One application Omperor knows how to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Launcher {
    /// The name the application goes by.
    pub(crate) name: &'static str,
    pub(crate) kind: LauncherKind,
    recipe: Recipe,
}

const fn editor(name: &'static str, programs: &'static [&'static str]) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Editor,
        recipe: Recipe { programs, args: &["{dir}"] },
    }
}

const fn terminal(
    name: &'static str,
    programs: &'static [&'static str],
    args: &'static [&'static str],
) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Terminal,
        recipe: Recipe { programs, args },
    }
}

const fn browser(name: &'static str, programs: &'static [&'static str]) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Browser,
        recipe: Recipe { programs, args: &["{dir}"] },
    }
}

/// Editors in the order they are preferred when nothing is stored: the
/// lighter, faster ones first, the IDEs after.
pub(crate) const EDITORS: &[Launcher] = &[
    editor("Zed", &["zed", "zeditor", "zedit"]),
    editor("Cursor", &["cursor"]),
    editor("Visual Studio Code", &["code"]),
    editor("VSCodium", &["codium"]),
    editor("Sublime Text", &["subl"]),
    editor("Sumi", &["aizome-sumi"]),
    editor("Kate", &["kate"]),
    editor("GNOME Text Editor", &["gnome-text-editor"]),
    editor("IntelliJ IDEA", &["idea", "idea-ultimate", "idea-community"]),
    editor("RustRover", &["rustrover"]),
    editor("Emacs", &["emacs"]),
];

/// Terminals in preference order, ending with the desktop's default
/// terminal (xdg-terminal-exec), which starts in the directory it is
/// started from.
pub(crate) const TERMINALS: &[Launcher] = &[
    terminal("Ghostty", &["ghostty"], &["--working-directory={dir}"]),
    terminal("WezTerm", &["wezterm"], &["start", "--cwd", "{dir}"]),
    terminal("kitty", &["kitty"], &["--directory", "{dir}"]),
    terminal("Alacritty", &["alacritty"], &["--working-directory", "{dir}"]),
    terminal("foot", &["foot"], &["--working-directory={dir}"]),
    terminal("Konsole", &["konsole"], &["--workdir", "{dir}"]),
    terminal("Ptyxis", &["ptyxis"], &["--new-window", "--working-directory={dir}"]),
    terminal("GNOME Console", &["kgx"], &["--working-directory={dir}"]),
    terminal("GNOME Terminal", &["gnome-terminal"], &["--working-directory={dir}"]),
    terminal("Xfce Terminal", &["xfce4-terminal"], &["--working-directory={dir}"]),
    terminal("Default terminal", &["xdg-terminal-exec"], &[]),
];

/// Browsers Omperor can be told to use for a sign-in page. There is no
/// preference order here: with nothing stored, the page opens in the
/// system's default browser, whichever that is.
pub(crate) const BROWSERS: &[Launcher] = &[
    browser("Firefox", &["firefox"]),
    browser("Google Chrome", &["google-chrome-stable", "google-chrome"]),
    browser("Chromium", &["chromium", "chromium-browser"]),
    browser("Brave Browser", &["brave", "brave-browser"]),
    browser("Microsoft Edge", &["microsoft-edge-stable", "microsoft-edge"]),
    browser("Vivaldi", &["vivaldi-stable", "vivaldi"]),
    browser("Zen", &["zen", "zen-browser"]),
    browser("Opera", &["opera"]),
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
}

/// How long a launch is watched for the program refusing its target (a
/// bad flag, a missing display) before it counts as started.
const LAUNCH_WATCH: std::time::Duration = std::time::Duration::from_millis(800);

impl Launcher {
    /// The program that starts the application: the first of its names
    /// found on `PATH`.
    fn program(&self) -> Option<PathBuf> {
        self.program_in(&std::env::var_os("PATH")?)
    }

    /// The first of the application's names found in the search path `path`.
    fn program_in(&self, path: &std::ffi::OsStr) -> Option<PathBuf> {
        self.recipe.programs.iter().find_map(|name| {
            std::env::split_paths(path)
                .map(|directory| directory.join(name))
                .find(|candidate| is_executable(candidate))
        })
    }

    pub(crate) fn is_installed(&self) -> bool {
        self.program().is_some()
    }

    /// The arguments that point the application at `target`.
    pub(crate) fn arguments(&self, target: &str) -> Vec<String> {
        self.recipe
            .args
            .iter()
            .map(|argument| argument.replace("{dir}", target))
            .collect()
    }

    /// Opens `target` in the application. Watches the start for a moment,
    /// so this belongs on a worker, never on the main thread.
    pub(crate) fn launch(&self, target: &Path) -> Result<(), String> {
        let directory = (self.kind == LauncherKind::Terminal).then_some(target);
        self.spawn(&target.to_string_lossy(), directory)
    }

    /// Opens a web page in the application. On a worker, as `launch` is.
    pub(crate) fn launch_url(&self, url: &str) -> Result<(), String> {
        self.spawn(url, None)
    }

    fn spawn(&self, target: &str, directory: Option<&Path>) -> Result<(), String> {
        let program = self
            .program()
            .ok_or_else(|| format!("{} is no longer installed", self.name))?;
        self.spawn_program(&program, target, directory)
    }

    fn spawn_program(&self, program: &Path, target: &str, directory: Option<&Path>) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;

        let mut command = Command::new(program);
        command
            .args(self.arguments(target))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own process group, so the application outlives Omperor
            // and a signal meant for Omperor's group.
            .process_group(0);
        if let Some(directory) = directory {
            command.current_dir(directory);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start {}: {error}", program.display()))?;
        let started = std::time::Instant::now();
        while started.elapsed() < LAUNCH_WATCH {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(format!("{} did not accept {target} ({status})", self.name)),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(error) => return Err(format!("could not watch {}: {error}", self.name)),
            }
        }
        // Still running: an editor or terminal of its own, reaped when the
        // user closes it.
        std::thread::Builder::new()
            .name("omperor-launched".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(|_| ())
            .map_err(|error| format!("could not watch {}: {error}", self.name))
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Every candidate of `kind` that is installed, in preference order.
fn installed(kind: LauncherKind) -> Vec<Launcher> {
    Launcher::candidates(kind)
        .iter()
        .copied()
        .filter(Launcher::is_installed)
        .collect()
}

/// The applications found installed, by kind. Finding them walks `PATH`,
/// so they are read once, on a worker, and kept.
#[derive(Debug, Clone, Default)]
pub(crate) struct Installed {
    editors: Vec<Launcher>,
    terminals: Vec<Launcher>,
    browsers: Vec<Launcher>,
}

impl Installed {
    pub(crate) fn detect() -> Self {
        Self {
            editors: installed(LauncherKind::Editor),
            terminals: installed(LauncherKind::Terminal),
            browsers: installed(LauncherKind::Browser),
        }
    }

    pub(crate) fn of(&self, kind: LauncherKind) -> &[Launcher] {
        match kind {
            LauncherKind::Editor => &self.editors,
            LauncherKind::Terminal => &self.terminals,
            LauncherKind::Browser => &self.browsers,
        }
    }

    /// The application to use: the stored preference when it is still
    /// installed, otherwise the first installed candidate.
    pub(crate) fn choose(&self, kind: LauncherKind, preference: Option<&str>) -> Option<Launcher> {
        choose_among(self.of(kind), preference)
    }
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
    use super::{BROWSERS, EDITORS, LauncherKind, TERMINALS, choose_among};
    #[test]
    fn linux_programs_are_handed_the_path_through_their_own_flags() {
        use super::Launcher;
        let zed = Launcher::named(LauncherKind::Editor, "Zed").expect("Zed is a known editor");
        assert_eq!(zed.arguments("/work/omperor/src/main.rs"), vec!["/work/omperor/src/main.rs"]);
        let ghostty = Launcher::named(LauncherKind::Terminal, "Ghostty").expect("Ghostty is known");
        assert_eq!(ghostty.arguments("/work/x y"), vec!["--working-directory=/work/x y"]);
        let wezterm = Launcher::named(LauncherKind::Terminal, "WezTerm").expect("WezTerm is known");
        assert_eq!(wezterm.arguments("/work"), vec!["start", "--cwd", "/work"]);
        let firefox = Launcher::named(LauncherKind::Browser, "Firefox").expect("Firefox is known");
        assert_eq!(
            firefox.arguments("https://auth.example/authorize?state=1"),
            vec!["https://auth.example/authorize?state=1"]
        );
        for launcher in EDITORS.iter().chain(TERMINALS).chain(BROWSERS) {
            assert!(!launcher.recipe.programs.is_empty(), "{}", launcher.name);
            // Every one but the desktop's default terminal is told its target.
            let told = launcher.recipe.args.iter().any(|argument| argument.contains("{dir}"));
            assert!(told || launcher.name == "Default terminal", "{}", launcher.name);
        }
    }

    #[test]
    fn linux_programs_are_found_on_path_and_started_detached() {
        use super::{Launcher, Recipe};
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("omperor-launchers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let marker = dir.join("marker");
        script("omperor-test-editor", &format!("pwd -P > '{0}'; echo \"$1\" >> '{0}'", marker.display()));
        script("omperor-test-refuses", "exit 3");
        std::fs::write(dir.join("omperor-test-plain"), "not a program").unwrap();
        let launcher = |programs: &'static [&'static str], kind| Launcher {
            name: "Test",
            kind,
            recipe: Recipe { programs, args: &["{dir}"] },
        };
        let path = std::env::join_paths([dir.clone(), "/nonexistent".into()]).unwrap();

        assert!(launcher(&["omperor-test-plain"], LauncherKind::Editor).program_in(&path).is_none(), "not executable");
        assert!(launcher(&["omperor-test-missing"], LauncherKind::Editor).program_in(&path).is_none());
        let terminal = launcher(&["omperor-test-missing", "omperor-test-editor"], LauncherKind::Terminal);
        let program = terminal.program_in(&path).expect("the second name is found");
        let target = dir.to_string_lossy();
        terminal.spawn_program(&program, &target, Some(&dir)).expect("started");
        let written = std::fs::read_to_string(&marker).unwrap();
        let canonical = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(written, format!("{}\n{target}\n", canonical.display()), "started in the directory, told its path");
        let refuses = launcher(&["omperor-test-refuses"], LauncherKind::Editor);
        let program = refuses.program_in(&path).expect("found");
        let refused = refuses.spawn_program(&program, &target, None);
        assert!(refused.is_err_and(|error| error.contains("did not accept")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stored_preference_wins_while_it_is_installed() {
        let first = TERMINALS[0];
        let last = *TERMINALS.last().expect("terminals");
        let available = [first, last];
        assert_eq!(
            choose_among(&available, Some(last.name)).map(|launcher| launcher.name),
            Some(last.name)
        );
        assert_eq!(
            choose_among(&available, Some("Gone")).map(|launcher| launcher.name),
            Some(first.name)
        );
        assert_eq!(
            choose_among(&available, None).map(|launcher| launcher.name),
            Some(first.name)
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
        assert!(
            BROWSERS
                .iter()
                .all(|launcher| launcher.kind == LauncherKind::Browser)
        );
    }
}
