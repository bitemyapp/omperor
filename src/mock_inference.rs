//! The local developer backend's contract: what a mock turn is made of.
//!
//! A mock turn is a *script* — a schedule of the same events a real OMP turn
//! produces (reasoning, tool calls with durations and results, sub-agents,
//! the answer streamed in chunks, an inference receipt, the turn's end) —
//! built deterministically from the prompt, so the whole interface can be
//! exercised without a provider, credentials, files, or tools. Sizes and
//! rates are Unicode scalar values, not provider tokens or UTF-8 bytes.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(crate) const MODEL: &str = "mock/deterministic";
pub(crate) const PROVIDER: &str = "mock";
/// The tour's answer to a request for a change, and to a question.
const TOUR_CHANGE: &str = include_str!("mock_tour_change.md");
const TOUR_EXPLAIN: &str = include_str!("mock_tour_explain.md");
/// The models the tour reports, so the composer reads as it does with a
/// real provider signed in.
pub(crate) const TOUR_MODELS: [&str; 3] = [
    "anthropic/claude-opus-5-5",
    "anthropic/claude-sonnet-5-5",
    "anthropic/claude-haiku-5-5",
];

/// Set by `--readme-shots`: every plain prompt is a [`Scenario::Tour`] turn.
static TOUR: AtomicBool = AtomicBool::new(false);

pub(crate) fn start_tour() {
    TOUR.store(true, Ordering::Relaxed);
}

pub(crate) fn tour() -> bool {
    TOUR.load(Ordering::Relaxed)
}
pub(crate) const EXAMPLE: &str = "/mock scenario=subagents ttft=800";
pub(crate) const MAX_SIZE: usize = 1_000_000;
const MAX_RATE: u32 = 80_000;
const MAX_TTFT_MS: u64 = 60_000;
/// The answer to a scripted turn, when the prompt does not say.
const AGENT_SIZE: usize = 600;
/// The old stress default: a long stream and nothing else.
const STRESS_SIZE: usize = 4000;
const RATE: u32 = 240;
const TTFT_MS: u64 = 1_200;
const SLOW_TTFT_MS: u64 = 8_000;
/// The Markdown scenario answers almost at once: it exists to look at the
/// renderer, not at the wait.
const MARKDOWN_TTFT_MS: u64 = 250;
/// The largest file `file=` reads, in bytes; a four-byte scalar at most per
/// character keeps it within `MAX_SIZE` characters' reach.
const MAX_FILE_BYTES: u64 = 4 * MAX_SIZE as u64;
/// The Markdown stress document: every construct the transcript draws, once.
/// A `markdown` turn answers with it verbatim — front matter first, no lead.
pub(crate) const STRESS_DOCUMENT: &str = include_str!("mock_markdown_stress.md");
/// The showcase's document: the stress document with every callout kind, a
/// named code file and a longer diff, for looking at rather than timing.
pub(crate) const SHOWCASE_DOCUMENT: &str = include_str!("mock_showcase.md");
/// Characters a token, for the receipt's counts.
const CHARS_PER_TOKEN: usize = 4;
/// The context a real turn carries besides the prompt.
const CONTEXT_TOKENS: u64 = 1_200;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    #[default]
    Omp,
    Mock,
}

impl Backend {
    pub(crate) fn from_stored(value: Option<&str>) -> Self {
        match value {
            Some("mock") => Self::Mock,
            _ => Self::Omp,
        }
    }

    pub(crate) fn stored(self) -> &'static str {
        match self {
            Self::Omp => "omp",
            Self::Mock => "mock",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Template {
    Text,
    Markdown,
    Code,
    Table,
    Unicode,
    /// The Markdown stress document.
    Stress,
    /// The showcase scenario's document.
    Showcase,
}

impl Template {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "markdown" => Ok(Self::Markdown),
            "code" => Ok(Self::Code),
            "table" => Ok(Self::Table),
            "unicode" => Ok(Self::Unicode),
            "stress" => Ok(Self::Stress),
            _ => Err(
                "Unknown template. Choose text, markdown, code, table, unicode, or stress.".into(),
            ),
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Text => {
                "A clear interface keeps the reader in control. New output arrives without moving text that is already being read. Each task has a name, an outcome, and a place to return to.\n\n"
            }
            Self::Markdown => {
                "## A repeatable review\n\nThe **same input** produces the same output. Use this stream to inspect wrapping, selection, and scrolling.\n\n- [x] Stable content\n- [ ] Review the next section\n\n> Keep the reader's place as new text arrives.\n\nInline `code` and [a reference](https://example.com) share the line.\n\n```rust\nfn total(values: &[u64]) -> u64 {\n    values.iter().sum()\n}\n```\n\n"
            }
            Self::Code => {
                "```rust\n/// A deterministic reduction with no external state.\nfn summarize(values: &[u64]) -> (usize, u64) {\n    let count = values.len();\n    let total = values.iter().copied().sum();\n    (count, total)\n}\n\nassert_eq!(summarize(&[2, 3, 5]), (3, 10));\n```\n\n"
            }
            Self::Table => {
                "| Task | State | Records |\n| :--- | :--- | ---: |\n| Interface review | Complete | 24 |\n| Keyboard navigation | Running | 18 |\n| Session recovery | Needs review | 7 |\n\n"
            }
            Self::Unicode => {
                "## Text across languages\n\nCafé · naïve · résumé · Ελληνικά · 日本語 · العربية\n\nA rocket 🚀, a check ✓, and a composed accent e\u{301}. Text stays intact across streaming boundaries.\n\n"
            }
            Self::Stress => STRESS_DOCUMENT,
            Self::Showcase => SHOWCASE_DOCUMENT,
        }
    }
}

/// The shape of a scripted turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scenario {
    /// Reason, read and search the project, run a check, answer.
    Agent,
    /// Reason and answer; no tools.
    Answer,
    /// The agent turn with two sub-agents delegated and waited on.
    Subagents,
    /// The work starts, then the provider refuses the turn.
    Error,
    /// The sign-in is rejected before any token arrives.
    Auth,
    /// The agent turn after a long wait for the first token.
    Slow,
    /// The old stream: the template cycled to `size`, nothing else.
    Stress,
    /// A brief thought, then the Markdown stress document (or `file=`) as
    /// the whole answer: no tools, no lead line.
    Markdown,
    /// Everything a transcript shows, for looking at its presentation:
    /// thinking, tools (one of them failing, then retried), more thinking,
    /// and the Markdown stress document as the answer.
    Showcase,
    /// The README tour: work on a small Rust project that reads like a real
    /// session. A question is answered from one file read; anything else is
    /// a change made with two scouts, edits, a failing test and its fix.
    Tour,
    /// The agent tidies Omperor through its own tools, against the app's
    /// real answers: it lists the sessions, then asks to delete the mock
    /// sessions and drafts it finds. Needs `/omperor` first.
    Omperor,
}

impl Scenario {
    fn parse(value: &str) -> Result<Self, String> {
        Ok(match value {
            "agent" => Self::Agent,
            "answer" => Self::Answer,
            "subagents" | "sub-agents" | "delegate" => Self::Subagents,
            "error" | "fail" => Self::Error,
            "auth" => Self::Auth,
            "slow" => Self::Slow,
            "stress" | "stream" => Self::Stress,
            "markdown" | "markdown-stress" => Self::Markdown,
            "showcase" => Self::Showcase,
            "omperor" | "tidy" => Self::Omperor,
            "tour" => Self::Tour,
            _ => {
                return Err(
                    "Unknown scenario. Choose agent, answer, subagents, error, auth, slow, stress, markdown, showcase, tour, or omperor."
                        .into(),
                );
            }
        })
    }

    /// The scenario a plain prompt asks for, by the words in it.
    fn inferred(prompt: &str) -> Self {
        let lower = prompt.to_lowercase();
        let says = |words: &[&str]| words.iter().any(|word| lower.contains(word));
        if tour() {
            Self::Tour
        } else if says(&["showcase"]) {
            Self::Showcase
        } else if says(&["tidy", "clean up", "cleanup", "garbage"]) {
            Self::Omperor
        } else if says(&["markdown", "stress test"]) {
            Self::Markdown
        } else if says(&["sub-agent", "subagent", "delegate", "spawn"]) {
            Self::Subagents
        } else if says(&["revoke", "sign in", "sign-in", "401", "auth"]) {
            Self::Auth
        } else if says(&["fail", "error", "crash", "break"]) {
            Self::Error
        } else if says(&["slow", "wait"]) {
            Self::Slow
        } else if says(&["just answer", "no tools", "explain", "what is", "why"]) {
            Self::Answer
        } else {
            Self::Agent
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Answer => "answer",
            Self::Subagents => "subagents",
            Self::Error => "error",
            Self::Auth => "auth",
            Self::Slow => "slow",
            Self::Stress => "stress",
            Self::Markdown => "markdown",
            Self::Showcase => "showcase",
            Self::Tour => "tour",
            Self::Omperor => "omperor",
        }
    }

    fn default_ttft_ms(self) -> u64 {
        match self {
            Self::Slow => SLOW_TTFT_MS,
            Self::Stress => 0,
            Self::Markdown => MARKDOWN_TTFT_MS,
            _ => TTFT_MS,
        }
    }

    /// The template when the request names none: the Markdown scenario's
    /// document is the stress test.
    fn default_template(self) -> Template {
        match self {
            Self::Markdown => Template::Stress,
            Self::Showcase => Template::Showcase,
            _ => Template::Markdown,
        }
    }

    /// The answer's length when the request does not say: the Markdown
    /// scenario's is its document's, once.
    fn default_size(self, template: Template) -> usize {
        match self {
            Self::Stress => STRESS_SIZE,
            Self::Markdown | Self::Showcase => template.text().chars().count(),
            _ => AGENT_SIZE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) scenario: Scenario,
    pub(crate) template: Template,
    /// Characters a second for streamed text; zero is unpaced.
    pub(crate) rate: u32,
    /// Characters in the answer body.
    pub(crate) size: usize,
    /// Milliseconds before the first token.
    pub(crate) ttft_ms: u64,
    /// What was asked, which the script answers to.
    pub(crate) prompt: String,
    /// `file=`: a file in the session's project whose text replaces the
    /// template's. Read by [`Request::load_file`] on the runtime's thread.
    pub(crate) file: Option<String>,
    /// The text `file` held once read.
    pub(crate) document: Option<Arc<str>>,
    /// Whether `size=` was given; otherwise a file's length sets it.
    pub(crate) size_given: bool,
}

impl Request {
    /// A `/mock …` line sets the scenario and knobs explicitly; any other
    /// input is a prompt, and the words in it choose the scenario.
    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("Type a message, or a /mock line.".into());
        }
        let directive = trimmed == "/mock" || trimmed.starts_with("/mock ");
        if !directive {
            let scenario = Scenario::inferred(trimmed);
            let template = scenario.default_template();
            return Ok(Self {
                scenario,
                template,
                rate: RATE,
                size: scenario.default_size(template),
                ttft_ms: scenario.default_ttft_ms(),
                prompt: trimmed.to_owned(),
                file: None,
                document: None,
                size_given: false,
            });
        }
        let mut scenario = Scenario::Agent;
        let mut template = None;
        let mut rate = RATE;
        let mut size = None;
        let mut ttft_ms = None;
        let mut file = None;
        let mut seen = [false; 6];
        for word in trimmed.split_whitespace().skip(1) {
            let (key, value) = word
                .split_once('=')
                .ok_or_else(|| format!("Expected key=value, got {word:?}. Example: {EXAMPLE}"))?;
            let index = match key {
                "scenario" => 0,
                "template" => 1,
                "rate" => 2,
                "size" => 3,
                "ttft" => 4,
                "file" => 5,
                _ => {
                    return Err(format!(
                        "Unknown option {key:?}. Use scenario, template, rate, size, ttft, and file."
                    ));
                }
            };
            if std::mem::replace(&mut seen[index], true) {
                return Err(format!("Specify {key} only once."));
            }
            match key {
                "scenario" => scenario = Scenario::parse(value)?,
                "template" => template = Some(Template::parse(value)?),
                "rate" => {
                    rate = value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| *n <= MAX_RATE)
                        .ok_or("Rate must be a whole number from 0 to 80,000 characters/second.")?;
                }
                "size" => {
                    size = Some(
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|n| *n <= MAX_SIZE)
                            .ok_or("Size must be a whole number from 0 to 1,000,000 characters.")?,
                    );
                }
                "ttft" => {
                    ttft_ms = Some(
                        value
                            .parse::<u64>()
                            .ok()
                            .filter(|n| *n <= MAX_TTFT_MS)
                            .ok_or("ttft must be a whole number of milliseconds up to 60,000.")?,
                    );
                }
                "file" => {
                    if value.is_empty() {
                        return Err("file needs a path in the project, like file=notes.md.".into());
                    }
                    file = Some(value.to_owned());
                }
                _ => unreachable!(),
            }
        }
        let template = template.unwrap_or_else(|| scenario.default_template());
        Ok(Self {
            scenario,
            template,
            rate,
            size: size.unwrap_or_else(|| scenario.default_size(template)),
            ttft_ms: ttft_ms.unwrap_or_else(|| scenario.default_ttft_ms()),
            prompt: format!("mock {}", scenario.name()),
            file,
            document: None,
            size_given: size.is_some(),
        })
    }

    /// Reads `file=` against the session's project — on the runtime's
    /// thread, never the interface's. A file's text replaces the template,
    /// and sets the size when `size=` did not.
    pub(crate) fn load_file(&mut self, project: &Path) -> Result<(), String> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let path = project.join(file);
        let too_large = || format!("{} is larger than {MAX_SIZE} characters.", path.display());
        let length = std::fs::metadata(&path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?
            .len();
        if length > MAX_FILE_BYTES {
            return Err(too_large());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        let characters = text.chars().count();
        if characters > MAX_SIZE {
            return Err(too_large());
        }
        if !self.size_given {
            self.size = characters;
        }
        self.document = Some(Arc::from(text));
        Ok(())
    }

    /// The text the answer is cut from: the file's, else the template's.
    fn source(&self) -> &str {
        self.document.as_deref().unwrap_or(self.template.text())
    }

    /// The answer body: exactly `size` characters of the source, cycled.
    pub(crate) fn output(&self) -> String {
        self.source().chars().cycle().take(self.size).collect()
    }

    /// At most 20 paced updates/second; slow streams sleep until a character
    /// is due. A late consumer catches up in bounded chunks without busy-waiting.
    pub(crate) fn chunk_chars(&self) -> usize {
        if self.rate == 0 {
            4096
        } else {
            (self.rate as usize).div_ceil(20)
        }
    }

    pub(crate) fn due(&self, delivered: usize) -> Duration {
        if self.rate == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(delivered as f64 / f64::from(self.rate))
        }
    }

    /// A word from the prompt the script can search for and speak of: its
    /// subject, which a prompt tends to put last ("tell me about the
    /// sidebar"), so the last word that is not filler; failing that, the
    /// longest word; failing that, a default.
    fn keyword(&self) -> String {
        const FILLER: &[&str] = &[
            "mock", "please", "could", "would", "should", "about", "with", "that", "this", "then",
            "into", "from", "have", "tell", "show", "make", "give", "find", "look", "help", "want",
            "need", "like", "does", "your", "what", "when", "where", "which", "will", "just",
            "explain", "describe", "some", "them", "they", "there", "here", "much", "many", "very",
            "really", "thing", "things", "something",
        ];
        let words: Vec<String> = self
            .prompt
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| word.len() > 3 && word.chars().any(char::is_alphabetic))
            .map(str::to_lowercase)
            .collect();
        words
            .iter()
            .rev()
            .find(|word| !FILLER.contains(&word.as_str()))
            .or_else(|| words.iter().max_by_key(|word| word.len()))
            .cloned()
            .unwrap_or_else(|| "session".to_owned())
    }

    fn excerpt(&self) -> String {
        let mut excerpt: String = self.prompt.chars().take(60).collect();
        if self.prompt.chars().count() > 60 {
            excerpt.push('…');
        }
        excerpt
    }

    /// The turn's schedule: every event, at its offset from the start.
    pub(crate) fn script(&self) -> Script {
        if self.scenario == Scenario::Tour {
            return self.tour_script();
        }
        let mut script = Script::default();
        let mut at = Duration::ZERO;
        script.push(at, Action::Started);
        if self.scenario == Scenario::Stress {
            let last = script.stream(self, at, &self.output(), Action::Answer);
            script.push_receipt(self, Duration::ZERO, last, 0);
            script.push(last + Duration::from_millis(30), Action::Finish(Outcome::Completed));
            return script;
        }
        at += Duration::from_millis(self.ttft_ms);
        let first_token = at;
        if self.scenario == Scenario::Markdown {
            // A brief thought, then the document as the whole answer.
            let thought = "**Setting the document**\n\nNo tools; the answer is the document itself.";
            at = script.stream(self, at, thought, Action::Thinking);
            at += Duration::from_millis(100);
            let last = script.stream(self, at, &self.output(), Action::Answer);
            script.push_receipt(self, first_token, last, thought.chars().count());
            script.push(last + Duration::from_millis(40), Action::Finish(Outcome::Completed));
            return script;
        }
        let excerpt = self.excerpt();
        let keyword = self.keyword();
        let mut thinking_chars = 0;
        let opening = match self.scenario {
            Scenario::Auth => format!(
                "**Reading the request**\n\nThe prompt asks: {excerpt}. Reaching the provider first."
            ),
            Scenario::Answer => format!(
                "**Reading the request**\n\nThe prompt asks: {excerpt}. This needs no tools; answering directly."
            ),
            _ => format!(
                "**Reading the request**\n\nThe prompt asks: {excerpt}. I'll look at the project layout, search for `{keyword}`, and run a quick check before answering."
            ),
        };
        thinking_chars += opening.chars().count();
        at = script.stream(self, at, &opening, Action::Thinking);
        if self.scenario == Scenario::Auth {
            at += Duration::from_millis(400);
            script.push(
                at,
                Action::AuthFailure(
                    "inference Authentication error during Handshake (token_revoked) [http 401]: Encountered invalidated oauth token for user, failing request for account openai-codex:mock-account".into(),
                ),
            );
            return script;
        }
        let tools = self.scenario != Scenario::Answer;
        if tools {
            at += Duration::from_millis(150);
            at = script.tool(
                at,
                "call-1",
                "read",
                r#"{"path":"Cargo.toml"}"#,
                "Read the crate manifest",
                Duration::from_millis(350),
                None,
                "[package]\nname = \"omperor\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nobjc2 = \"0.6\"\nserde_json = \"1\"\n",
            );
            if self.scenario == Scenario::Subagents {
                at += Duration::from_millis(200);
                let children = vec![
                    Child {
                        id: "mock-child-1".into(),
                        label: "scout-layout".into(),
                        task: "Map the crate layout and report the modules that matter.".into(),
                        runs_for: Duration::from_millis(5_500),
                        output: "src/main.rs holds the AppKit delegate; chat.rs and native_markdown.rs render the transcript; omp_runtime.rs owns the child process.".into(),
                    },
                    Child {
                        id: "mock-child-2".into(),
                        label: "scout-tests".into(),
                        task: "List the test modules and what each one covers.".into(),
                        runs_for: Duration::from_millis(8_200),
                        output: "journal.rs (folding), timeline.rs (rows and marks), metrics.rs (throughput), account_alert.rs (attribution).".into(),
                    },
                ];
                let ids = children.iter().map(|child| child.id.clone()).collect::<Vec<_>>();
                let args = format!(
                    r#"{{"context":"Read-only scouting for: {excerpt}","tasks":[{{"name":"scout-layout","task":"{}"}},{{"name":"scout-tests","task":"{}"}}]}}"#,
                    children[0].task, children[1].task
                );
                script.push(
                    at,
                    Action::ToolStart {
                        call_id: "call-task".into(),
                        name: "task".into(),
                        args,
                        intent: "Delegate two read-only scouts".into(),
                    },
                );
                script.push(at, Action::Spawn(children.clone()));
                at += Duration::from_millis(120);
                script.push(
                    at,
                    Action::ToolFinish {
                        call_id: "call-task".into(),
                        name: "task".into(),
                        text: "Started scout-layout, scout-tests. Each child's result auto-delivers when it settles.".into(),
                        is_error: false,
                    },
                );
                let spawned = at;
                let mut settled = spawned;
                for child in &children {
                    let done = spawned + child.runs_for;
                    settled = settled.max(done);
                    script.push(done, Action::ChildDone { id: child.id.clone(), output: child.output.clone() });
                }
                at += Duration::from_millis(300);
                script.push(
                    at,
                    Action::ToolStart {
                        call_id: "call-hub".into(),
                        name: "hub".into(),
                        args: format!(r#"{{"op":"wait","ids":["{}","{}"]}}"#, ids[0], ids[1]),
                        intent: "Wait for both scouts".into(),
                    },
                );
                at = settled + Duration::from_millis(100);
                script.push(
                    at,
                    Action::ToolFinish {
                        call_id: "call-hub".into(),
                        name: "hub".into(),
                        text: "scout-layout: completed · scout-tests: completed".into(),
                        is_error: false,
                    },
                );
            }
            at += Duration::from_millis(150);
            at = script.tool(
                at,
                "call-2",
                "grep",
                &format!(r#"{{"pattern":"{keyword}","path":"src"}}"#),
                &format!("Find where {keyword} is handled"),
                Duration::from_millis(600),
                None,
                &format!(
                    "src/main.rs:412: fn {keyword}_state(&self) -> State\nsrc/chat.rs:88: // {keyword} rows read like the live transcript\nsrc/session.rs:140: {keyword}: Option<String>,\n"
                ),
            );
            if self.scenario == Scenario::Showcase {
                at += Duration::from_millis(150);
                at = script.tool_failed(
                    at,
                    "call-2b",
                    "bash",
                    r#"{"command":"cargo test --quiet transcript"}"#,
                    "Run the transcript tests",
                    Duration::from_millis(900),
                    "error[E0425]: cannot find value `measure` in this scope\n  --> src/chat.rs:212:17\n\nerror: could not compile `omperor` (bin \"omperor\" test) due to 1 previous error\n",
                );
            }
            at += Duration::from_millis(150);
            at = script.tool(
                at,
                "call-3",
                "bash",
                r#"{"command":"cargo check --quiet"}"#,
                "Check the crate compiles",
                Duration::from_millis(1_400),
                Some("    Checking omperor v0.1.0 (/project)\n"),
                "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.84s\n",
            );
        }
        if self.scenario == Scenario::Error {
            at += Duration::from_millis(300);
            let note = "**Composing the answer**\n\nThe check passed; drafting the summary.";
            at = script.stream(self, at, note, Action::Thinking);
            at += Duration::from_millis(500);
            script.push(
                at,
                Action::Finish(Outcome::Errored(
                    "inference RateLimited error during Handshake [http 429]: The usage limit has been reached for this account; it resets in 2 hours".into(),
                )),
            );
            return script;
        }
        at += Duration::from_millis(250);
        let closing = if tools {
            "**Composing the answer**\n\nThe manifest, the search, and the check agree; writing it up."
        } else {
            "**Composing the answer**"
        };
        thinking_chars += closing.chars().count();
        at = script.stream(self, at, closing, Action::Thinking);
        at += Duration::from_millis(200);
        let answer = if self.scenario == Scenario::Showcase { self.output() } else { self.answer(&keyword, tools) };
        let last = script.stream(self, at, &answer, Action::Answer);
        script.push_receipt(self, first_token, last, thinking_chars);
        script.push(last + Duration::from_millis(40), Action::Finish(Outcome::Completed));
        script
    }

    /// The README tour's turn: a question gets a reading and an
    /// explanation; a request gets the change, made the way an agent would.
    fn tour_script(&self) -> Script {
        let mut script = Script::default();
        let mut at = Duration::ZERO;
        script.push(at, Action::Started);
        at += Duration::from_millis(self.ttft_ms);
        let first_token = at;
        let mut thinking_chars = 0;
        let mut think = |script: &mut Script, at: Duration, text: &str| {
            thinking_chars += text.chars().count();
            script.stream(self, at, text, Action::Thinking)
        };
        if self.prompt.contains('?') {
            at = think(
                &mut script,
                at,
                "**Reading the request**\n\nThe question is what `measure::wrap` costs. Reading it to count the work done per word.",
            );
            at += Duration::from_millis(150);
            at = script.tool(at, "call-1", "read", r#"{"path":"src/measure.rs"}"#, "Read the wrapping code", Duration::from_millis(300), None, "//! Display width of text in terminal columns.\n\nuse unicode_width::UnicodeWidthStr;\n\npub fn wrap(text: &str, width: usize) -> Vec<String> {\n    let mut lines = Vec::new();\n    let mut line = String::new();\n    for word in text.split(' ') {\n        let needed = if line.is_empty() { word.width() } else { line.width() + 1 + word.width() };\n");
            at += Duration::from_millis(200);
            at = think(
                &mut script,
                at,
                "**Working it out**\n\nEach word measures the whole line built so far, so the cost per word grows with the line, not with the word. Bounded by the width, so still linear overall.",
            );
            at += Duration::from_millis(200);
            let last = script.stream(self, at, TOUR_EXPLAIN, Action::Answer);
            script.push_receipt(self, first_token, last, thinking_chars);
            script.push(last + Duration::from_millis(40), Action::Finish(Outcome::Completed));
            return script;
        }
        at = think(
            &mut script,
            at,
            "**Reading the request**\n\nReplies are cut every 72 bytes, which splits multi-byte characters. I'll find where output is printed, send two scouts to check the callers and the tests, then wrap by display width.",
        );
        at += Duration::from_millis(150);
        at = script.tool(at, "call-1", "read", r#"{"path":"src/chat.rs"}"#, "Read the chat loop", Duration::from_millis(300), None, "fn print_wrapped(text: &str) {\n    let mut out = io::stdout().lock();\n    for chunk in text.as_bytes().chunks(WIDTH) {\n        let _ = writeln!(out, \"{}\", String::from_utf8_lossy(chunk));\n    }\n}\n");
        at += Duration::from_millis(200);
        let children = vec![
            Child {
                id: "scout-callers".into(),
                label: "scout-callers".into(),
                task: "Find every place that prints model output.".into(),
                runs_for: Duration::from_millis(4_500),
                output: "Only chat::print_wrapped prints replies; main.rs calls chat::run once.".into(),
            },
            Child {
                id: "scout-tests".into(),
                label: "scout-tests".into(),
                task: "Check how the crate is tested and what a wrapping test needs.".into(),
                runs_for: Duration::from_millis(6_500),
                output: "No tests yet. Unit tests beside the code fit; unicode-width is already a dependency.".into(),
            },
        ];
        let args = format!(
            r#"{{"context":"Read-only scouting for width-aware wrapping","tasks":[{{"name":"scout-callers","task":"{}"}},{{"name":"scout-tests","task":"{}"}}]}}"#,
            children[0].task, children[1].task
        );
        script.push(at, Action::ToolStart { call_id: "call-task".into(), name: "task".into(), args, intent: "Send two read-only scouts".into() });
        script.push(at, Action::Spawn(children.clone()));
        at += Duration::from_millis(120);
        script.push(at, Action::ToolFinish { call_id: "call-task".into(), name: "task".into(), text: "Started scout-callers, scout-tests. Each child's result auto-delivers when it settles.".into(), is_error: false });
        let spawned = at;
        let mut settled = spawned;
        for child in &children {
            let done = spawned + child.runs_for;
            settled = settled.max(done);
            script.push(done, Action::ChildDone { id: child.id.clone(), output: child.output.clone() });
        }
        at += Duration::from_millis(300);
        script.push(at, Action::ToolStart { call_id: "call-hub".into(), name: "hub".into(), args: r#"{"op":"wait","ids":["scout-callers","scout-tests"]}"#.into(), intent: "Wait for both scouts".into() });
        at = settled + Duration::from_millis(100);
        script.push(at, Action::ToolFinish { call_id: "call-hub".into(), name: "hub".into(), text: "scout-callers: completed · scout-tests: completed".into(), is_error: false });
        at += Duration::from_millis(200);
        at = think(
            &mut script,
            at,
            "**Planning the change**\n\nOnly `print_wrapped` prints replies. A `measure` module with a width-aware `wrap` keeps `chat.rs` small and gives the test a home.",
        );
        at += Duration::from_millis(150);
        at = script.tool(at, "call-2", "write", r#"{"path":"src/measure.rs"}"#, "Add width-aware wrapping", Duration::from_millis(250), None, "Wrote 38 lines to src/measure.rs");
        at += Duration::from_millis(120);
        at = script.tool(at, "call-3", "edit", r#"{"input":"[src/chat.rs#print_wrapped]"}"#, "Print the wrapped lines", Duration::from_millis(250), None, "Edited src/chat.rs (+4 −3)");
        at += Duration::from_millis(120);
        at = script.tool(at, "call-4", "edit", r#"{"input":"[src/session.rs#recent]"}"#, "Add Session::recent", Duration::from_millis(250), None, "Edited src/session.rs (+5)");
        at += Duration::from_millis(150);
        at = script.tool_failed(at, "call-5", "bash", r#"{"command":"cargo test --quiet"}"#, "Run the tests", Duration::from_millis(1_100), "error[E0432]: unresolved import `crate::measure`\n --> src/chat.rs:5:5\n  |\n5 | use crate::measure;\n  |     ^^^^^^^^^^^^^^ no `measure` in the root\n\nerror: could not compile `harbor` (bin \"harbor\" test) due to 1 previous error\n");
        at += Duration::from_millis(150);
        at = script.tool(at, "call-6", "edit", r#"{"input":"[src/main.rs#mod]"}"#, "Declare the module", Duration::from_millis(200), None, "Edited src/main.rs (+1)");
        at += Duration::from_millis(150);
        at = script.tool(at, "call-7", "bash", r#"{"command":"cargo test --quiet"}"#, "Run the tests again", Duration::from_millis(1_600), Some("   Compiling harbor v0.3.0 (/project)\n"), "running 1 test\n.\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n");
        at += Duration::from_millis(250);
        at = think(&mut script, at, "**Composing the answer**\n\nTests pass; summarizing the change and the one behaviour worth a decision.");
        at += Duration::from_millis(200);
        let last = script.stream(self, at, TOUR_CHANGE, Action::Answer);
        script.push_receipt(self, first_token, last, thinking_chars);
        script.push(last + Duration::from_millis(40), Action::Finish(Outcome::Completed));
        script
    }

    /// The answer: a lead that speaks to the prompt, then the template to
    /// make up `size` characters.
    fn answer(&self, keyword: &str, tools: bool) -> String {
        let lead = if tools {
            format!(
                "Here is what I found for **{}**.\n\n- `Cargo.toml` names the crate `omperor`, edition 2024.\n- `{keyword}` is handled in three places under `src/`.\n- `cargo check` passes.\n\n",
                self.excerpt()
            )
        } else {
            format!("On **{}**, briefly:\n\n", self.excerpt())
        };
        let lead_chars = lead.chars().count();
        if self.size <= lead_chars {
            return lead.chars().take(self.size).collect();
        }
        lead + &self
            .source()
            .chars()
            .cycle()
            .take(self.size - lead_chars)
            .collect::<String>()
    }
}

/// A sub-agent the script spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Child {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) task: String,
    pub(crate) runs_for: Duration,
    pub(crate) output: String,
}

/// How a scripted turn ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Completed,
    Errored(String),
}

/// The figures the receipt carries, as a real provider would count them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub(crate) tokens_in: u64,
    pub(crate) tokens_out: u64,
    pub(crate) tokens_reasoning: u64,
    pub(crate) ttft_ms: u64,
    pub(crate) last_token_ms: u64,
    pub(crate) duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Started,
    Thinking(String),
    ToolStart {
        call_id: String,
        name: String,
        args: String,
        intent: String,
    },
    ToolUpdate {
        call_id: String,
        content: String,
    },
    ToolFinish {
        call_id: String,
        name: String,
        text: String,
        is_error: bool,
    },
    Spawn(Vec<Child>),
    ChildDone {
        id: String,
        output: String,
    },
    Answer(String),
    Receipt(Receipt),
    Finish(Outcome),
    AuthFailure(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) at: Duration,
    pub(crate) action: Action,
}

/// A turn's events in order. Steps are kept sorted by offset as they are
/// pushed, so a child settling before a later tool starts lands where it
/// happened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Script {
    pub(crate) steps: Vec<Step>,
}

impl Script {
    fn push(&mut self, at: Duration, action: Action) {
        let position = self
            .steps
            .iter()
            .rposition(|step| step.at <= at)
            .map_or(0, |index| index + 1);
        self.steps.insert(position, Step { at, action });
    }

    /// Streams `text` from `from` at the request's rate, one step a chunk.
    /// A chunk lands when its first character is due — the first one at
    /// `from` itself, as a first token arrives at the ttft — and the return
    /// is when the last character is due, which is when the text is over.
    fn stream(
        &mut self,
        request: &Request,
        from: Duration,
        text: &str,
        wrap: fn(String) -> Action,
    ) -> Duration {
        let chunk = request.chunk_chars().max(1);
        let total = text.chars().count();
        let mut delivered = 0;
        let mut buffer = String::new();
        for (index, character) in text.chars().enumerate() {
            buffer.push(character);
            let flush = buffer.chars().count() == chunk || index + 1 == total;
            if flush {
                self.push(from + request.due(delivered), wrap(std::mem::take(&mut buffer)));
                delivered = index + 1;
            }
        }
        from + request.due(total)
    }

    /// One tool call: it starts, may report partial output, and finishes
    /// after `runs_for`. Returns when it finished.
    #[allow(clippy::too_many_arguments)]
    fn tool(
        &mut self,
        from: Duration,
        call_id: &str,
        name: &str,
        args: &str,
        intent: &str,
        runs_for: Duration,
        update: Option<&str>,
        result: &str,
    ) -> Duration {
        self.push(
            from,
            Action::ToolStart {
                call_id: call_id.into(),
                name: name.into(),
                args: args.into(),
                intent: intent.into(),
            },
        );
        if let Some(content) = update {
            self.push(
                from + runs_for.mul_f64(0.4),
                Action::ToolUpdate { call_id: call_id.into(), content: content.into() },
            );
        }
        let done = from + runs_for;
        self.push(
            done,
            Action::ToolFinish {
                call_id: call_id.into(),
                name: name.into(),
                text: result.into(),
                is_error: false,
            },
        );
        done
    }

    /// A tool call that ends in an error.
    #[allow(clippy::too_many_arguments)]
    fn tool_failed(
        &mut self,
        from: Duration,
        call_id: &str,
        name: &str,
        args: &str,
        intent: &str,
        runs_for: Duration,
        result: &str,
    ) -> Duration {
        self.push(
            from,
            Action::ToolStart { call_id: call_id.into(), name: name.into(), args: args.into(), intent: intent.into() },
        );
        let done = from + runs_for;
        self.push(
            done,
            Action::ToolFinish { call_id: call_id.into(), name: name.into(), text: result.into(), is_error: true },
        );
        done
    }

    fn push_receipt(&mut self, request: &Request, first_token: Duration, last: Duration, thinking_chars: usize) {
        let answer_chars = self
            .steps
            .iter()
            .filter_map(|step| match &step.action {
                Action::Answer(text) => Some(text.chars().count()),
                _ => None,
            })
            .sum::<usize>();
        let tokens = |chars: usize| chars.div_ceil(CHARS_PER_TOKEN) as u64;
        let last_ms = last.as_millis() as u64;
        self.push(
            last + Duration::from_millis(20),
            Action::Receipt(Receipt {
                tokens_in: CONTEXT_TOKENS + tokens(request.prompt.chars().count()),
                tokens_out: tokens(answer_chars + thinking_chars),
                tokens_reasoning: tokens(thinking_chars),
                ttft_ms: first_token.as_millis() as u64,
                last_token_ms: last_ms,
                duration_ms: last_ms + 30,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(script: &Script) -> Vec<&Action> {
        script.steps.iter().map(|step| &step.action).collect()
    }

    fn kinds(script: &Script) -> Vec<&'static str> {
        script
            .steps
            .iter()
            .map(|step| match &step.action {
                Action::Started => "started",
                Action::Thinking(_) => "thinking",
                Action::ToolStart { .. } => "tool_start",
                Action::ToolUpdate { .. } => "tool_update",
                Action::ToolFinish { .. } => "tool_finish",
                Action::Spawn(_) => "spawn",
                Action::ChildDone { .. } => "child_done",
                Action::Answer(_) => "answer",
                Action::Receipt(_) => "receipt",
                Action::Finish(_) => "finish",
                Action::AuthFailure(_) => "auth_failure",
            })
            .collect()
    }

    fn dedup(kinds: &[&'static str]) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for kind in kinds {
            if out.last() != Some(kind) {
                out.push(kind);
            }
        }
        out
    }

    /// Every formula in the stress document copies as TeX that is the math
    /// it typesets and parses back as the same formula (Upleft's
    /// `math_copy_conformance`).
    #[test]
    fn the_stress_documents_formulas_copy_as_the_math_they_typeset() {
        use upleft_render::view::math_copy_conformance::{Report, check_document};
        let mut report = Report::default();
        check_document(STRESS_DOCUMENT, "stress", &mut report);
        let checked: usize = report.counts.iter().filter(|((property, _), _)| property == "typeset").map(|(_, (passed, failed))| passed + failed).sum();
        assert!(checked > 0, "the stress document has formulas");
        assert_eq!(report.failed(), 0, "{}\n{:#?}", report.summary(), report.failures);
    }

    #[test]
    fn a_plain_prompt_is_a_request_and_its_words_choose_the_scenario() {
        let plain = Request::parse("Tell me about the sidebar").unwrap();
        assert_eq!(plain.scenario, Scenario::Agent);
        assert_eq!(plain.prompt, "Tell me about the sidebar");
        assert_eq!((plain.rate, plain.size, plain.ttft_ms), (RATE, AGENT_SIZE, TTFT_MS));
        assert_eq!(Request::parse("Spawn two sub-agents").unwrap().scenario, Scenario::Subagents);
        assert_eq!(Request::parse("make it fail").unwrap().scenario, Scenario::Error);
        assert_eq!(Request::parse("the token was revoked").unwrap().scenario, Scenario::Auth);
        assert_eq!(Request::parse("be slow about it").unwrap().scenario, Scenario::Slow);
        assert_eq!(Request::parse("why is the sky blue").unwrap().scenario, Scenario::Answer);
        assert_eq!(Request::parse("be slow about it").unwrap().ttft_ms, SLOW_TTFT_MS);
        assert!(Request::parse("   ").is_err());
        // The search names the prompt's subject, not its verb.
        assert_eq!(Request::parse("Tell me about the sidebar").unwrap().keyword(), "sidebar");
        assert_eq!(Request::parse("Fix the speedometer, please").unwrap().keyword(), "speedometer");
        assert_eq!(Request::parse("hi").unwrap().keyword(), "session");
        assert_eq!(Request::parse("please tell me").unwrap().keyword(), "please", "filler only: the longest word");
    }

    #[test]
    fn a_directive_sets_the_scenario_and_knobs_and_refuses_what_it_does_not_know() {
        let request = Request::parse(EXAMPLE).unwrap();
        assert_eq!(request.scenario, Scenario::Subagents);
        assert_eq!(request.ttft_ms, 800);
        let stress = Request::parse("/mock scenario=stress template=code rate=0 size=12").unwrap();
        assert_eq!((stress.rate, stress.size, stress.ttft_ms), (0, 12, 0));
        assert_eq!(stress.output().chars().count(), 12);
        // A bare /mock is the agent scenario at the defaults.
        assert_eq!(Request::parse("/mock").unwrap().scenario, Scenario::Agent);
        for input in [
            "/mock rate=-1",
            "/mock rate=NaN",
            "/mock size=1000001",
            "/mock rate=1000001",
            "/mock template=other",
            "/mock scenario=other",
            "/mock size=2 size=3",
            "/mock delay=2",
            "/mock ttft=60001",
            "/mock rate=2.5",
            "/mock file=",
            "/mock file=a.md file=b.md",
        ] {
            assert!(Request::parse(input).is_err(), "{input}");
        }
        // "/mocked" is a prompt, not a directive.
        assert_eq!(Request::parse("/mocked up").unwrap().prompt, "/mocked up");
    }

    #[test]
    fn output_size_is_exact_and_independent_of_rate() {
        for template in [
            Template::Text,
            Template::Markdown,
            Template::Code,
            Template::Table,
            Template::Unicode,
            Template::Stress,
        ] {
            for size in [0, 1, 17, 4097, MAX_SIZE] {
                let slow = Request {
                    scenario: Scenario::Stress,
                    template,
                    rate: 1,
                    size,
                    ttft_ms: 0,
                    prompt: "mock stress".into(),
                    file: None,
                    document: None,
                    size_given: true,
                };
                let fast = Request { rate: 0, ..slow.clone() };
                assert_eq!(slow.output().chars().count(), size);
                assert_eq!(slow.output(), fast.output());
            }
        }
    }

    #[test]
    fn pacing_uses_cumulative_deadlines_and_bounded_batches() {
        let mut request = Request::parse("/mock scenario=stress").unwrap();
        assert_eq!(request.chunk_chars(), 12);
        assert_eq!(request.due(240), Duration::from_secs(1));
        request.rate = 1;
        assert_eq!(request.chunk_chars(), 1);
        assert_eq!(request.due(1), Duration::from_secs(1));
        request.rate = MAX_RATE;
        assert_eq!(request.chunk_chars(), 4000);
        request.rate = 0;
        assert_eq!(request.due(MAX_SIZE), Duration::ZERO);
    }

    #[test]
    fn the_agent_script_reasons_works_answers_and_accounts_for_itself() {
        let request = Request::parse("Tell me about the sidebar").unwrap();
        let script = request.script();
        assert_eq!(
            dedup(&kinds(&script)),
            [
                "started", "thinking", "tool_start", "tool_finish", "tool_start", "tool_finish",
                "tool_start", "tool_update", "tool_finish", "thinking", "answer", "receipt", "finish",
            ]
        );
        // Offsets never go backwards.
        assert!(script.steps.windows(2).all(|pair| pair[0].at <= pair[1].at));
        // The first token lands at the ttft; the receipt says so and counts
        // every streamed character, reasoning included, at four a token.
        let first_thinking = script.steps.iter().find(|step| matches!(step.action, Action::Thinking(_))).unwrap();
        assert_eq!(first_thinking.at, Duration::from_millis(TTFT_MS));
        let receipt = actions(&script)
            .into_iter()
            .find_map(|action| match action {
                Action::Receipt(receipt) => Some(receipt.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(receipt.ttft_ms, TTFT_MS);
        // The text after a stream begins once the stream is over, not while
        // its last chunk is still due.
        let opening_chars = match &first_thinking.action {
            Action::Thinking(text) => text.chars().count(),
            _ => unreachable!(),
        };
        let first_tool = script.steps.iter().find(|step| matches!(step.action, Action::ToolStart { .. })).unwrap();
        assert!(first_tool.at >= first_thinking.at + request.due(opening_chars), "{:?} < {:?}", first_tool.at, first_thinking.at + request.due(opening_chars));
        let answer_chars: usize = actions(&script)
            .into_iter()
            .filter_map(|action| match action {
                Action::Answer(text) => Some(text.chars().count()),
                _ => None,
            })
            .sum();
        assert_eq!(answer_chars, AGENT_SIZE, "the answer body is exactly `size`");
        let thinking_chars: usize = actions(&script)
            .into_iter()
            .filter_map(|action| match action {
                Action::Thinking(text) => Some(text.chars().count()),
                _ => None,
            })
            .sum();
        assert_eq!(receipt.tokens_reasoning, thinking_chars.div_ceil(4) as u64);
        assert_eq!(receipt.tokens_out, (answer_chars + thinking_chars).div_ceil(4) as u64);
        assert!(receipt.last_token_ms > receipt.ttft_ms);
        assert_eq!(receipt.duration_ms, receipt.last_token_ms + 30);
        // The search names a word from the prompt.
        assert!(actions(&script).iter().any(|action| matches!(action, Action::ToolStart { name, args, .. } if name == "grep" && args.contains("sidebar"))));
        assert!(matches!(script.steps.last().unwrap().action, Action::Finish(Outcome::Completed)));
    }

    #[test]
    fn sub_agents_settle_before_the_wait_ends_and_a_stopped_turn_has_none_of_it() {
        let script = Request::parse("/mock scenario=subagents").unwrap().script();
        let kinds = kinds(&script);
        let spawn = kinds.iter().position(|kind| *kind == "spawn").unwrap();
        let hub_finish = script
            .steps
            .iter()
            .position(|step| matches!(&step.action, Action::ToolFinish { name, .. } if name == "hub"))
            .unwrap();
        let children_done: Vec<usize> = kinds.iter().enumerate().filter(|(_, kind)| **kind == "child_done").map(|(index, _)| index).collect();
        assert_eq!(children_done.len(), 2);
        assert!(children_done.iter().all(|index| *index > spawn && *index < hub_finish));
        // The task call returns at once with job ids; the wait spans the children.
        let task_finish = script.steps.iter().find(|step| matches!(&step.action, Action::ToolFinish { name, .. } if name == "task")).unwrap();
        assert!(task_finish.at < script.steps[children_done[0]].at);
        assert!(script.steps[hub_finish].at > script.steps[children_done[1]].at);
    }

    #[test]
    fn error_and_auth_scripts_end_the_way_a_provider_would() {
        let error = Request::parse("/mock scenario=error").unwrap().script();
        assert!(matches!(&error.steps.last().unwrap().action, Action::Finish(Outcome::Errored(reason)) if reason.contains("429")));
        assert!(!kinds(&error).contains(&"answer"));
        assert!(!kinds(&error).contains(&"receipt"));
        assert!(kinds(&error).contains(&"tool_finish"), "the error comes after real work");
        let auth = Request::parse("/mock scenario=auth").unwrap().script();
        assert!(matches!(&auth.steps.last().unwrap().action, Action::AuthFailure(reason) if reason.contains("token_revoked")));
        assert!(!kinds(&auth).contains(&"tool_start"));
        assert!(!kinds(&auth).contains(&"finish"), "a refused sign-in ends the turn by itself");
        let stress = Request::parse("/mock scenario=stress size=100 rate=0").unwrap().script();
        assert_eq!(dedup(&kinds(&stress)), ["started", "answer", "receipt", "finish"]);
    }

    fn answer_text(script: &Script) -> String {
        script
            .steps
            .iter()
            .filter_map(|step| match &step.action {
                Action::Answer(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn markdown_prompts_and_directives_choose_the_stress_document() {
        let plain = Request::parse("Show me the markdown renderer").unwrap();
        assert_eq!(plain.scenario, Scenario::Markdown);
        assert_eq!(plain.template, Template::Stress);
        assert_eq!(plain.size, STRESS_DOCUMENT.chars().count());
        assert_eq!(plain.ttft_ms, MARKDOWN_TTFT_MS);
        assert_eq!(Request::parse("run the stress test").unwrap().scenario, Scenario::Markdown);
        // The word alone is not the scenario: "stress" names no test.
        assert_eq!(Request::parse("does stress help").unwrap().scenario, Scenario::Agent);
        for input in ["/mock scenario=markdown", "/mock scenario=markdown-stress"] {
            let request = Request::parse(input).unwrap();
            assert_eq!(request.scenario, Scenario::Markdown, "{input}");
            assert_eq!(request.template, Template::Stress);
            assert_eq!(request.size, STRESS_DOCUMENT.chars().count());
        }
        // An explicit template is the document instead, once by default.
        let code = Request::parse("/mock scenario=markdown template=code").unwrap();
        assert_eq!(code.output(), Template::Code.text());
        // The stress template serves the other scenarios too.
        let stress = Request::parse("/mock scenario=stress template=stress size=64").unwrap();
        assert_eq!(stress.output(), STRESS_DOCUMENT.chars().take(64).collect::<String>());
        // The document covers what the renderers are compared on.
        for construct in [
            "---\ntitle:", "# Markdown", "***both***", "~~struck~~", "[^timing]", "[[Design notes]]",
            "<kbd>", "<details>", "\\\n", "- [x]", "3. ", "> > ", "> [!NOTE]", "```rust", "```diff",
            "```json", "\n    An indented", "| :--- | :---: | ---: |", "\n---\n", "$$", "```math",
            "flowchart", "sequenceDiagram", "classDiagram", "stateDiagram", "erDiagram", "xychart",
            "![The crowned gate](markdown-stress.png)",
        ] {
            assert!(STRESS_DOCUMENT.contains(construct), "{construct:?}");
        }
    }

    #[test]
    fn the_markdown_turn_streams_the_document_exactly_with_no_lead_or_tools() {
        let request = Request::parse("/mock scenario=markdown rate=240").unwrap();
        let script = request.script();
        assert_eq!(dedup(&kinds(&script)), ["started", "thinking", "answer", "receipt", "finish"]);
        assert!(script.steps.windows(2).all(|pair| pair[0].at <= pair[1].at));
        assert!(script.steps.iter().filter(|step| matches!(step.action, Action::Answer(_))).count() > 1, "streamed in chunks");
        let answer = answer_text(&script);
        assert_eq!(answer, STRESS_DOCUMENT, "the chunks reassemble to the document");
        assert!(answer.starts_with("---\n"), "front matter comes first");
        // A larger size cycles the document.
        let heavy = Request::parse("/mock scenario=markdown template=stress size=20000 rate=4000").unwrap();
        let answer = answer_text(&heavy.script());
        assert_eq!(answer.chars().count(), 20_000);
        assert!(answer.starts_with(STRESS_DOCUMENT));
        assert!(answer[STRESS_DOCUMENT.len()..].starts_with(STRESS_DOCUMENT));
    }

    #[test]
    fn a_file_replaces_the_template_and_sets_the_size() {
        let project = std::env::temp_dir().join(format!("omperor-mock-file-{}", std::process::id()));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("notes.md"), "# Notes\n\nCafé e\u{301}.\n").unwrap();
        let mut request = Request::parse("/mock scenario=markdown file=notes.md rate=0").unwrap();
        assert_eq!(request.file.as_deref(), Some("notes.md"));
        request.load_file(&project).unwrap();
        assert_eq!(request.size, 18);
        assert_eq!(answer_text(&request.script()), "# Notes\n\nCafé e\u{301}.\n");
        let mut cut = Request::parse("/mock scenario=markdown file=notes.md size=7").unwrap();
        cut.load_file(&project).unwrap();
        assert_eq!(cut.output(), "# Notes");
        let mut missing = Request::parse("/mock scenario=markdown file=absent.md").unwrap();
        assert!(missing.load_file(&project).unwrap_err().contains("absent.md"));
        // No file: nothing to read, nothing changes.
        let mut plain = Request::parse("/mock scenario=markdown").unwrap();
        plain.load_file(&project).unwrap();
        assert_eq!(plain.output(), STRESS_DOCUMENT);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn the_tour_makes_a_change_with_scouts_and_answers_a_question_from_one_read() {
        let change = Request::parse("/mock scenario=tour").unwrap().script();
        let names = |script: &Script| {
            script
                .steps
                .iter()
                .filter_map(|step| match &step.action {
                    Action::ToolStart { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&change), ["read", "task", "hub", "write", "edit", "edit", "bash", "edit", "bash"]);
        assert!(actions(&change).iter().any(|action| matches!(action, Action::Spawn(children) if children.len() == 2)));
        assert_eq!(
            actions(&change).iter().filter(|action| matches!(action, Action::ToolFinish { is_error: true, .. })).count(),
            1,
            "one failing test run, then its fix"
        );
        assert!(matches!(change.steps.last().unwrap().action, Action::Finish(Outcome::Completed)));

        let mut question = Request::parse("/mock scenario=tour").unwrap();
        question.prompt = "How does it scale?".into();
        let question = question.script();
        assert_eq!(names(&question), ["read"]);
        let answer: String = actions(&question)
            .iter()
            .filter_map(|action| match action {
                Action::Answer(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(answer, TOUR_EXPLAIN);
    }
}
