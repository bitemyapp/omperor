//! Opening the project, or one file in it, in the user's editor or terminal,
//! and a sign-in page in their browser.
//!
//! Omperor does not embed an editor, a terminal, or a browser: the user
//! already has the ones they like, so the app's job is to hand them the
//! directory, file, or page in one keystroke. This module knows which
//! applications are installed, how each one wants to be pointed at a target
//! from the command line, and which to use when no preference has been
//! stored. On macOS applications are bundles opened through `open`; on
//! Linux they are programs on `PATH`, started directly.

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

/// How an application opens a path when asked through `open`.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recipe {
    /// `open -a <App> <path>`: the application opens the path itself, which
    /// every editor and a few terminals support.
    OpenWith,
    /// `open -na <App> --args <arguments>`: a new instance, told where to start
    /// through its own flags; `{dir}` in an argument is replaced by the path.
    NewInstance(&'static [&'static str]),
}

/// How an application is started on Linux: the first of `programs` found
/// on `PATH`, run with `args`, in which `{dir}` is replaced by the path
/// (or page). A terminal also starts in the directory.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Recipe {
    programs: &'static [&'static str],
    args: &'static [&'static str],
}

/// One application Omperor knows how to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Launcher {
    /// The application's name, as its bundle is named under `/Applications`
    /// on macOS; on Linux, the name it goes by.
    pub(crate) name: &'static str,
    pub(crate) kind: LauncherKind,
    recipe: Recipe,
}

#[cfg(target_os = "macos")]
const fn editor(name: &'static str) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Editor,
        recipe: Recipe::OpenWith,
    }
}

#[cfg(target_os = "macos")]
const fn terminal(name: &'static str, recipe: Recipe) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Terminal,
        recipe,
    }
}

#[cfg(target_os = "macos")]
const fn browser(name: &'static str) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Browser,
        recipe: Recipe::OpenWith,
    }
}

/// Editors in the order they are preferred when nothing is stored: the
/// lighter, faster ones first, the IDEs after.
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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

#[cfg(not(target_os = "macos"))]
const fn editor(name: &'static str, programs: &'static [&'static str]) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Editor,
        recipe: Recipe { programs, args: &["{dir}"] },
    }
}

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
const fn browser(name: &'static str, programs: &'static [&'static str]) -> Launcher {
    Launcher {
        name,
        kind: LauncherKind::Browser,
        recipe: Recipe { programs, args: &["{dir}"] },
    }
}

/// Editors in the order they are preferred when nothing is stored: the
/// lighter, faster ones first, the IDEs after.
#[cfg(not(target_os = "macos"))]
pub(crate) const EDITORS: &[Launcher] = &[
    editor("Zed", &["zed", "zeditor", "zedit"]),
    editor("Cursor", &["cursor"]),
    editor("Visual Studio Code", &["code"]),
    editor("VSCodium", &["codium"]),
    editor("Sublime Text", &["subl"]),
    editor("Kate", &["kate"]),
    editor("GNOME Text Editor", &["gnome-text-editor"]),
    editor("IntelliJ IDEA", &["idea", "idea-ultimate", "idea-community"]),
    editor("RustRover", &["rustrover"]),
    editor("Emacs", &["emacs"]),
];

/// Terminals in preference order, ending with the desktop's default
/// terminal (xdg-terminal-exec), which starts in the directory it is
/// started from.
#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
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

#[cfg(target_os = "macos")]
impl Launcher {
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

/// How long a launch is watched for the program refusing its target (a
/// bad flag, a missing display) before it counts as started.
#[cfg(not(target_os = "macos"))]
const LAUNCH_WATCH: std::time::Duration = std::time::Duration::from_millis(800);

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
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
    use super::{BROWSERS, EDITORS, LauncherKind, TERMINALS, choose_among};
    #[cfg(target_os = "macos")]
    use super::Launcher;
    #[cfg(target_os = "macos")]
    use std::path::{Path, PathBuf};

    #[cfg(target_os = "macos")]
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

    #[cfg(target_os = "macos")]
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

    #[cfg(target_os = "macos")]
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

    #[cfg(target_os = "macos")]
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

    #[cfg(not(target_os = "macos"))]
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

    #[cfg(not(target_os = "macos"))]
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
