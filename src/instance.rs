//! One Omperor at a time (one per channel: the installed app and a Dev
//! build run side by side): a second launch hands its arguments to the
//! running one, over `$XDG_RUNTIME_DIR/aizome-app.<channel>.sock`, and exits.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// Where the running Omperor listens.
pub(crate) fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    let name = match crate::app_state::Channel::current() {
        crate::app_state::Channel::Production => "omperor",
        crate::app_state::Channel::Development => "omperor-dev",
    };
    Some(PathBuf::from(dir).join(format!("aizome-app.{name}.sock")))
}

/// Either the running Omperor took `args` (this launch should exit), or
/// this one is the first: it listens for later launches.
pub(crate) enum Instance {
    Handed,
    First(Option<UnixListener>),
}

pub(crate) fn claim(args: &[String]) -> Instance {
    let Some(path) = socket_path() else { return Instance::First(None) };
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let line = serde_json::to_string(args).unwrap_or_else(|_| "[]".to_owned());
        if writeln!(stream, "{line}").is_ok() {
            return Instance::Handed;
        }
    }
    // Nobody answered: a socket left by an Omperor that's gone.
    let _ = std::fs::remove_file(&path);
    Instance::First(UnixListener::bind(&path).ok())
}

/// Hear later launches' arguments, calling `take` with each.
pub(crate) fn listen(listener: UnixListener, take: impl Fn(Vec<String>) -> bool + Send + 'static) {
    let _ = std::thread::Builder::new().name("omperor-instance".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                let args: Vec<String> = serde_json::from_str(line.trim()).unwrap_or_default();
                if !take(args) {
                    break;
                }
            }
        }
    });
}

/// Take the socket away when Omperor quits.
pub(crate) fn release() {
    if let Some(path) = socket_path() {
        let _ = std::fs::remove_file(path);
    }
}
