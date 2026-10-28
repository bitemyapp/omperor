//! The composer's slash commands: what `/` offers, how the list filters as
//! the user types, and how a submitted line is recognised. Pure logic, so it
//! is tested on its own; the composer draws the list (`ui::composer`).

/// One command the composer understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Command {
    /// Without the slash.
    pub(crate) name: &'static str,
    /// What follows the name, for the list's hint; empty when it takes none,
    /// bracketed when every argument is optional.
    pub(crate) arguments: &'static str,
    pub(crate) summary: &'static str,
    /// Offered only with the mock backend (or developer mode).
    pub(crate) mock_only: bool,
}

impl Command {
    /// Whether it needs an argument, so choosing it cannot run it at once.
    pub(crate) fn takes_arguments(&self) -> bool {
        !self.arguments.is_empty() && !self.arguments.starts_with('[')
    }

    /// What choosing it puts in the composer.
    pub(crate) fn completion(&self) -> String {
        if self.takes_arguments() {
            format!("/{} ", self.name)
        } else {
            format!("/{}", self.name)
        }
    }
}

/// Every command, in the order the list shows them.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        name: "usage",
        arguments: "[all]",
        summary: "This session's tokens, cache, time, sub-agents and subscription",
        mock_only: false,
    },
    Command {
        name: "omperor",
        arguments: "[request | off]",
        summary: "Let the agent list, delete and archive Omperor's sessions and projects",
        mock_only: false,
    },
    Command {
        name: "mock",
        arguments: "scenario=subagents|answer|markdown|… ttft= rate=",
        summary: "Script a turn from the mock backend",
        mock_only: true,
    },
];

/// The commands offered now.
pub(crate) fn available(mock: bool) -> Vec<Command> {
    COMMANDS
        .iter()
        .filter(|command| mock || !command.mock_only)
        .copied()
        .collect()
}

/// The word being typed when the composer holds only `/word` and the caret
/// (in characters) is within it: what the list filters by (without the
/// slash). `None` closes the list.
pub(crate) fn active_query(text: &str, caret: usize) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let word = &rest[..word_end];
    let word_chars = 1 + word.chars().count();
    (caret >= 1 && caret <= word_chars && word_end == rest.len()).then_some(word)
}

/// The commands matching `query`, best first: a name that starts with it,
/// then one containing it, then a subsequence match; ties keep list order.
pub(crate) fn filter(commands: &[Command], query: &str) -> Vec<Command> {
    let query = query.to_lowercase();
    let mut ranked: Vec<(u8, usize, Command)> = commands
        .iter()
        .enumerate()
        .filter_map(|(order, command)| {
            let name = command.name;
            let rank = if name.starts_with(&query) {
                0
            } else if name.contains(&query) {
                1
            } else if is_subsequence(&query, name) {
                2
            } else {
                return None;
            };
            Some((rank, order, *command))
        })
        .collect();
    ranked.sort_by_key(|(rank, order, _)| (*rank, *order));
    ranked.into_iter().map(|(_, _, command)| command).collect()
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut rest = haystack.chars();
    needle.chars().all(|c| rest.any(|h| h == c))
}

/// A composer line Omperor answers itself, never sent as a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Local {
    /// `/usage`, or `/usage all` for every signed-in account's limits.
    Usage { all: bool },
    /// `/usage` with something else after it; the line is answered with a
    /// note rather than sent.
    UsageArguments(String),
    /// `/omperor`, or `/omperor <request>`: offer the agent Omperor's own
    /// tools in this session, then send the request, if any, as the prompt.
    Omperor(String),
    /// `/omperor off`.
    OmperorOff,
}

/// The rest of the line after `/name`, when the line is that command:
/// `/names` is a different word, so not.
fn command_rest<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let rest = text.trim_start().strip_prefix('/')?.strip_prefix(name)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(rest)
}

/// Recognises a submitted line that Omperor handles locally.
pub(crate) fn parse_local(text: &str) -> Option<Local> {
    if let Some(rest) = command_rest(text, "omperor") {
        // The request keeps its own lines and spacing.
        let request = rest.trim();
        return Some(if request.eq_ignore_ascii_case("off") {
            Local::OmperorOff
        } else {
            Local::Omperor(request.to_owned())
        });
    }
    let trimmed = text.trim();
    let rest = trimmed.strip_prefix("/usage")?;
    if rest.is_empty() {
        return Some(Local::Usage { all: false });
    }
    // "/usages" is a prompt, not the command.
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    match rest.trim() {
        all if all.eq_ignore_ascii_case("all") => Some(Local::Usage { all: true }),
        other => Some(Local::UsageArguments(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_opens_only_at_the_start_and_within_the_first_word() {
        assert_eq!(active_query("/", 1), Some(""));
        assert_eq!(active_query("/us", 3), Some("us"));
        assert_eq!(active_query("/us", 2), Some("us"));
        // Before the slash, after a space, or not at the start: closed.
        assert_eq!(active_query("/us", 0), None);
        assert_eq!(active_query("/mock scenario", 14), None);
        assert_eq!(active_query("/mock ", 6), None);
        assert_eq!(active_query(" /us", 4), None);
        assert_eq!(active_query("hello /us", 9), None);
        assert_eq!(active_query("", 0), None);
    }

    #[test]
    fn filter_ranks_prefix_then_contains_then_subsequence() {
        let all = available(true);
        let names = |query: &str| filter(&all, query).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(""), ["usage", "omperor", "mock"]);
        assert_eq!(names("US"), ["usage"]);
        assert_eq!(names("mo"), ["mock", "omperor"]);
        assert_eq!(names("sage"), ["usage"]);
        assert_eq!(names("mk"), ["mock"]);
        assert!(names("zz").is_empty());
    }

    #[test]
    fn mock_is_offered_only_with_the_mock() {
        assert_eq!(available(false).iter().map(|c| c.name).collect::<Vec<_>>(), ["usage", "omperor"]);
        assert!(available(true).iter().any(|c| c.name == "mock"));
        let mock = COMMANDS.iter().find(|c| c.name == "mock").unwrap();
        assert!(mock.takes_arguments());
        assert_eq!(mock.completion(), "/mock ");
        assert_eq!(COMMANDS[0].completion(), "/usage");
        // An optional argument still runs at once when chosen.
        assert!(!COMMANDS[0].takes_arguments());
    }

    #[test]
    fn usage_is_local_and_nothing_else_is() {
        assert_eq!(parse_local("/usage"), Some(Local::Usage { all: false }));
        assert_eq!(parse_local("  /usage \n"), Some(Local::Usage { all: false }));
        assert_eq!(parse_local("/usage all"), Some(Local::Usage { all: true }));
        assert_eq!(parse_local("/usage  ALL "), Some(Local::Usage { all: true }));
        assert_eq!(parse_local("/usage everything"), Some(Local::UsageArguments("everything".into())));
        assert_eq!(parse_local("/usages"), None);
        assert_eq!(parse_local("usage"), None);
        assert_eq!(parse_local("/mock scenario=subagents"), None);
        assert_eq!(parse_local("tell me about /usage"), None);
    }

    #[test]
    fn omperor_turns_the_tools_on_and_keeps_the_request() {
        assert_eq!(parse_local("/omperor"), Some(Local::Omperor(String::new())));
        assert_eq!(
            parse_local("/omperor delete my mock sessions\nand empty drafts "),
            Some(Local::Omperor("delete my mock sessions\nand empty drafts".into()))
        );
        assert_eq!(parse_local("/omperor OFF"), Some(Local::OmperorOff));
        assert_eq!(parse_local("/omperors"), None);
        assert_eq!(parse_local("ask /omperor"), None);
        let omperor = COMMANDS.iter().find(|c| c.name == "omperor").unwrap();
        // Chosen from the list, it turns the tools on at once.
        assert!(!omperor.takes_arguments());
    }
}
