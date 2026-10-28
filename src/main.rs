//! Omperor: a native Aizome app for working with coding agents.
//!
//! The agent is oh-my-pi (OMP), run as a child process per session and
//! spoken to over its typed RPC (`omp_runtime`). The interface is
//! Aizome's kit (`app` holds the state and what changes it, `ui` draws
//! it). `omperor eval …` runs headless evaluations with the same runtime.

mod account_alert;
mod agents;
mod app;
mod app_state;
mod archive;
mod attachments;
mod cache_policy;
mod change_tree;
mod chat;
mod composer;
mod eval;
mod eval_report;
mod eval_suite;
mod file_picker;
mod file_watcher;
mod housekeeping;
mod instance;
mod journal;
mod launchers;
mod live_sessions;
mod markdown;
mod metrics;
mod mock_inference;
mod omp_runtime;
mod omperor_tools;
mod perf;
mod preferences;
mod rules;
mod session;
mod session_state;
mod slash_commands;
mod timeline;
mod transcripts;
mod ui;
mod usage;
mod usage_card;
mod wake;
mod workspace;

pub(crate) use rules::AccountEntry;

/// The app's id on the desktop: its windows, its desktop entry, its icon.
/// Aizome's compositor gives ❖ to `org.aizome.*` apps as their command key.
pub(crate) const APP_ID: &str = if cfg!(feature = "production") { "org.aizome.Omperor" } else { "org.aizome.OmperorDev" };

const HELP: &str = "Usage: omperor [FOLDER]...
       omperor eval <command> [options]

Omperor works with coding agents: give one a project folder and a task, and
follow its reasoning, its tool calls, its sub-agents and the changes it
leaves, as they happen. A folder given opens as a project. A second launch
hands its folders to the Omperor already running.

  -h, --help      show this
  --state-paths   print where this edition keeps its state, as JSON

Environment:
  OMPEROR_STATE_DIR        keep Omperor's state here instead of
                           $XDG_DATA_HOME/omperor (or omperor-dev)
  OMPEROR_OMP_EXECUTABLE   the OMP helper to run
  OMPEROR_PROJECT          the folder to open first
";

fn main() {
    // Headless commands come first: they open no window.
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(code) = app_state::dispatch(&arguments).or_else(|| eval::dispatch(arguments)) {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return;
    }
    let _ = perf::initialize();
    let listener = match instance::claim(&args) {
        instance::Instance::Handed => return,
        instance::Instance::First(listener) => listener,
    };
    let mut app = app::App::new(listener);
    app.pending_folders = args.iter().filter(|a| !a.starts_with('-')).map(std::path::PathBuf::from).collect();
    aizome_kit::run(app);
    instance::release();
}
