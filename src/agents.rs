//! The Agents panel: the agent tree of the session in view. The root is the
//! session's own agent, the OMP child serving it; beneath it are the
//! sub-agents that agent spawned through the `task` tool, the ones still
//! running first, in spawn order, then the finished ones, newest first,
//! which can be narrowed to the ones that succeeded or the ones that did
//! not. Pure data, so the panel's logic is tested without AppKit.

use std::time::Duration;

use crate::omp_runtime::SubagentInfo;
use crate::session::SessionOutcome;

/// What an agent is doing, or how it stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum AgentState {
    /// A turn is running and the model is producing output.
    Working,
    /// A turn is running and the model is reasoning.
    Thinking,
    /// A turn is running a tool.
    RunningTool(String),
    /// The agent cannot proceed without the user: sign-in, a prompt, an
    /// error that needs attention.
    Blocked(String),
    /// Open, no turn running.
    #[default]
    Idle,
    /// The last turn completed.
    Done,
    /// The last turn ended with an error.
    Errored,
    /// The last turn was cut off.
    Interrupted,
    /// No child serves the session: history is shown read-only.
    Detached,
}

/// The dot drawn before an agent, one color per family of states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dot {
    /// Gold, filled: busy.
    Working,
    /// Plum ring: waiting on the user.
    Blocked,
    /// Hollow: open and idle, or detached.
    Idle,
    /// Green: finished well.
    Done,
    /// Red: finished badly.
    Errored,
}

impl AgentState {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Working => "working".into(),
            Self::Thinking => "thinking".into(),
            Self::RunningTool(tool) => format!("running {tool}"),
            Self::Blocked(why) => format!("blocked · {why}"),
            Self::Idle => "idle".into(),
            Self::Done => "done".into(),
            Self::Errored => "failed".into(),
            Self::Interrupted => "interrupted".into(),
            Self::Detached => "not attached".into(),
        }
    }

    pub(crate) const fn dot(&self) -> Dot {
        match self {
            Self::Working | Self::Thinking | Self::RunningTool(_) => Dot::Working,
            Self::Blocked(_) => Dot::Blocked,
            Self::Idle | Self::Detached => Dot::Idle,
            Self::Done => Dot::Done,
            Self::Errored | Self::Interrupted => Dot::Errored,
        }
    }

    pub(crate) const fn is_busy(&self) -> bool {
        matches!(self, Self::Working | Self::Thinking | Self::RunningTool(_))
    }

    /// The state a stored session is in when no child serves it, for the
    /// sidebar's dots.
    pub(crate) fn from_outcome(outcome: Option<SessionOutcome>) -> Self {
        match outcome {
            Some(SessionOutcome::Completed) => Self::Done,
            Some(SessionOutcome::Errored) => Self::Errored,
            Some(SessionOutcome::Interrupted | SessionOutcome::InProgress) => Self::Interrupted,
            None => Self::Idle,
        }
    }

    /// A sub-agent's state from the job board's lifecycle status.
    pub(crate) fn from_status(status: &str) -> Self {
        match status {
            "starting" | "running" => Self::Working,
            "completed" => Self::Done,
            "failed" => Self::Errored,
            "cancelled" => Self::Interrupted,
            _ => Self::Idle,
        }
    }
}

/// An in-session lens, not a global novice/expert mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum WorkScope {
    #[default]
    All,
    Active,
    Attention,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ViewState {
    pub(crate) desk: crate::workspace_model::SessionDesk,
    pub(crate) query: String,
    pub(crate) selected: Option<String>,
    pub(crate) scope: WorkScope,
    pub(crate) compact: bool,
    pub(crate) search_open: bool,
    pub(crate) finished_collapsed: bool,
    pub(crate) page: usize,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            desk: Default::default(),
            query: String::new(),
            selected: None,
            scope: WorkScope::All,
            compact: false,
            search_open: false,
            finished_collapsed: false,
            page: 0,
        }
    }
}

pub(crate) fn needs_attention(info: &SubagentInfo) -> bool {
    info.error.is_some()
        || info.warning.is_some()
        || matches!(
            AgentState::from_status(&info.status),
            AgentState::Errored | AgentState::Interrupted
        )
}

pub(crate) struct VisibleWork {
    pub(crate) rows: Vec<AgentRow>,
    pub(crate) matched: usize,
    pub(crate) page: usize,
    pub(crate) pages: usize,
}

/// Shared search and scope semantics for the task tree and Observatory.
pub(crate) fn matching_work<'a>(
    subagents: &'a [SubagentInfo],
    view: &ViewState,
) -> Vec<&'a SubagentInfo> {
    let query = view.query.trim().to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    subagents
        .iter()
        .filter(|info| {
            let admitted = match view.scope {
                WorkScope::All => true,
                WorkScope::Active => AgentState::from_status(&info.status).is_busy(),
                WorkScope::Attention => needs_attention(info),
            };
            if !admitted {
                return false;
            }
            if words.is_empty() {
                return true;
            }
            let haystack = format!(
                "{} {} {} {} {} {}",
                info.label,
                info.agent,
                info.id,
                AgentState::from_status(&info.status).label(),
                info.error.as_deref().unwrap_or(""),
                info.warning.as_deref().unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .collect()
}

/// Keep the root as orientation even under search. Search/attention explicitly
/// reveal matching finished work, so a collapsed group cannot conceal a result.
/// Page the native controls rather than constructing thousands of AppKit views.
pub(crate) fn visible_work(
    root: &RootAgent,
    subagents: &[SubagentInfo],
    now_ms: u64,
    view: &ViewState,
) -> VisibleWork {
    let query = view.query.trim().to_lowercase();
    let candidates: Vec<_> = matching_work(subagents, view)
        .into_iter()
        .cloned()
        .collect();
    let matched = candidates.len();
    let mut rows = agent_rows(root, &candidates, now_ms, FinishedFilter::All);
    let collapse = view.finished_collapsed && query.is_empty() && view.scope == WorkScope::All;
    let available = candidates
        .iter()
        .filter(|info| !collapse || AgentState::from_status(&info.status).is_busy())
        .count();
    let pages = available.div_ceil(40).max(1);
    let page = view.page.min(pages - 1);
    let start = page * 40;
    let mut seen = 0;
    let mut in_finished = false;
    rows.retain(|row| {
        if let AgentRow::Section { section, .. } = row {
            in_finished = *section == Section::Finished;
            return true;
        }
        if matches!(row, AgentRow::Subagent { .. }) {
            if collapse && in_finished {
                return false;
            }
            let show = (start..start + 40).contains(&seen);
            seen += 1;
            return show;
        }
        true
    });
    // Drop headings with no page rows, retaining the collapsed disclosure.
    let mut index = 0;
    while index < rows.len() {
        if let AgentRow::Section { section, .. } = &rows[index] {
            let collapsed_group = collapse && *section == Section::Finished;
            let has_child = matches!(rows.get(index + 1), Some(AgentRow::Subagent { .. }));
            if !collapsed_group && !has_child {
                rows.remove(index);
                continue;
            }
        }
        index += 1;
    }
    VisibleWork {
        rows,
        matched,
        page,
        pages,
    }
}

/// Legacy finished-state preference retained for database compatibility.
/// The activity workspace uses per-session WorkScope instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FinishedFilter {
    #[default]
    All,
    Succeeded,
    Failed,
}

impl FinishedFilter {
    pub(crate) const fn stored(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_stored(value: Option<&str>) -> Self {
        match value {
            Some("succeeded") => Self::Succeeded,
            Some("failed") => Self::Failed,
            _ => Self::All,
        }
    }

    /// Whether a finished sub-agent in `state` is shown under this filter.
    /// "Failed" is anything that did not finish well: an error, or being cut
    /// off.
    fn admits(self, state: &AgentState) -> bool {
        match self {
            Self::All => true,
            Self::Succeeded => *state == AgentState::Done,
            Self::Failed => *state != AgentState::Done,
        }
    }
}

/// The two groups a session's sub-agents fall into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Running,
    Finished,
}

impl Section {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Finished => "Finished",
        }
    }
}

/// The session's own agent, as the panel's root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootAgent {
    /// The session title, or a stand-in for a fresh one.
    pub(crate) title: String,
    pub(crate) state: AgentState,
    /// Time since the current turn started, while one runs.
    pub(crate) elapsed: Option<Duration>,
    pub(crate) model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentRow {
    Root {
        title: String,
        state: AgentState,
        /// "working · gpt-6-astra"; the clock is appended separately.
        subtitle: String,
        elapsed: Option<Duration>,
    },
    /// A heading over a group of sub-agents: how many the group holds, and
    /// how many of them the filter lets through.
    Section {
        section: Section,
        count: usize,
        shown: usize,
    },
    Subagent {
        id: String,
        title: String,
        state: AgentState,
        /// "running · scout", "done · reviewer · 12s".
        subtitle: String,
        /// Time since it started, while it runs; ticks in the panel.
        elapsed: Option<Duration>,
        error: Option<String>,
        /// The task was done, but something about its workspace was not:
        /// its changes were not merged back, say.
        warning: Option<String>,
        session_file: Option<std::path::PathBuf>,
    },
}

impl AgentRow {
    pub(crate) fn state(&self) -> &AgentState {
        match self {
            Self::Root { state, .. } | Self::Subagent { state, .. } => state,
            Self::Section { section, .. } => match section {
                Section::Running => &AgentState::Working,
                Section::Finished => &AgentState::Done,
            },
        }
    }

    pub(crate) fn title(&self) -> &str {
        match self {
            Self::Root { title, .. } | Self::Subagent { title, .. } => title,
            Self::Section { section, .. } => section.title(),
        }
    }

    pub(crate) fn elapsed(&self) -> Option<Duration> {
        match self {
            Self::Root { elapsed, .. } | Self::Subagent { elapsed, .. } => *elapsed,
            Self::Section { .. } => None,
        }
    }

    /// The subtitle without the clock, which the panel appends itself so it
    /// can tick without rebuilding the row.
    pub(crate) fn subtitle(&self) -> &str {
        match self {
            Self::Root { subtitle, .. } | Self::Subagent { subtitle, .. } => subtitle,
            Self::Section { .. } => "",
        }
    }

    /// The subtitle with the running clock appended.
    pub(crate) fn subtitle_line(&self) -> String {
        match self.elapsed() {
            Some(elapsed) => format!("{} · {}", self.subtitle(), elapsed_text(elapsed)),
            None => self.subtitle().to_owned(),
        }
    }
}

/// Lays the tree out: the root; then, under a heading, every sub-agent still
/// running, in spawn order; then, under another, the finished ones, newest
/// first, narrowed by `filter`. A heading counts its whole group and how many
/// the filter lets through.
pub(crate) fn agent_rows(
    root: &RootAgent,
    subagents: &[SubagentInfo],
    now_ms: u64,
    filter: FinishedFilter,
) -> Vec<AgentRow> {
    let mut subtitle = root.state.label();
    let model = short_model(&root.model);
    if !model.is_empty() && root.state != AgentState::Detached {
        subtitle.push_str(" · ");
        subtitle.push_str(&model);
    }
    let mut rows = vec![AgentRow::Root {
        title: root.title.clone(),
        state: root.state.clone(),
        subtitle,
        elapsed: root.elapsed.filter(|_| root.state.is_busy()),
    }];
    let mut running: Vec<&SubagentInfo> = Vec::new();
    let mut finished: Vec<&SubagentInfo> = Vec::new();
    for info in subagents {
        if AgentState::from_status(&info.status).is_busy() {
            running.push(info);
        } else {
            finished.push(info);
        }
    }
    running.sort_by_key(|info| (info.index, info.id.clone()));
    // The most recently finished first: what just happened is what the
    // user is most likely looking for.
    finished.sort_by(|left, right| {
        settled_ms(right)
            .cmp(&settled_ms(left))
            .then_with(|| right.index.cmp(&left.index))
    });
    if !running.is_empty() {
        rows.push(AgentRow::Section {
            section: Section::Running,
            count: running.len(),
            shown: running.len(),
        });
        rows.extend(running.into_iter().map(|info| subagent_row(info, now_ms)));
    }
    if !finished.is_empty() {
        let shown: Vec<&SubagentInfo> = finished
            .iter()
            .copied()
            .filter(|info| filter.admits(&AgentState::from_status(&info.status)))
            .collect();
        rows.push(AgentRow::Section {
            section: Section::Finished,
            count: finished.len(),
            shown: shown.len(),
        });
        rows.extend(shown.into_iter().map(|info| subagent_row(info, now_ms)));
    }
    rows
}

/// When a settled sub-agent settled, as far as the board says.
fn settled_ms(info: &SubagentInfo) -> u64 {
    info.started_ms
        .unwrap_or(0)
        .saturating_add(info.duration_ms.unwrap_or(0))
}

fn subagent_row(info: &SubagentInfo, now_ms: u64) -> AgentRow {
    let state = AgentState::from_status(&info.status);
    let mut subtitle = state.label();
    subtitle.push_str(" · ");
    subtitle.push_str(&info.agent);
    let elapsed = if state.is_busy() {
        info.started_ms
            .map(|started| Duration::from_millis(now_ms.saturating_sub(started)))
    } else {
        if let Some(duration) = info.duration_ms {
            subtitle.push_str(" · ");
            subtitle.push_str(&elapsed_text(Duration::from_millis(duration)));
        }
        None
    };
    if info.warning.is_some() {
        subtitle.push_str(" · needs review");
    }
    AgentRow::Subagent {
        id: info.id.clone(),
        title: info.label.clone(),
        state,
        subtitle,
        elapsed,
        error: info.error.clone(),
        warning: info.warning.clone(),
        session_file: info.session_file.clone(),
    }
}

/// "2 running · 1 done · 1 failed", or nothing when no sub-agent exists.
pub(crate) fn summary_line(subagents: &[SubagentInfo]) -> String {
    let mut running = 0;
    let mut done = 0;
    let mut failed = 0;
    let mut stopped = 0;
    let mut review = 0;
    for info in subagents {
        match AgentState::from_status(&info.status) {
            AgentState::Working => running += 1,
            AgentState::Done if info.warning.is_some() || info.error.is_some() => review += 1,
            AgentState::Done => done += 1,
            AgentState::Errored => failed += 1,
            _ => stopped += 1,
        }
    }
    let mut parts = Vec::new();
    for (count, word) in [
        (review, "needs review"),
        (running, "running"),
        (done, "done"),
        (failed, "failed"),
        (stopped, "stopped"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {word}"));
        }
    }
    parts.join(" · ")
}

/// `openai-codex/gpt-6-astra` → `gpt-6-astra`.
pub(crate) fn short_model(model: &str) -> String {
    model
        .rsplit_once('/')
        .map_or(model, |(_, model)| model)
        .to_owned()
}

/// `2m 14s`, `47s`, `1h 03m`.
pub(crate) fn elapsed_text(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        AgentRow, AgentState, Dot, FinishedFilter, RootAgent, Section, ViewState, WorkScope,
        agent_rows, elapsed_text, summary_line, visible_work,
    };
    use crate::omp_runtime::SubagentInfo;

    fn subagent(id: &str, index: u64, agent: &str, status: &str) -> SubagentInfo {
        SubagentInfo {
            id: id.into(),
            index,
            agent: agent.into(),
            label: id.into(),
            status: status.into(),
            started_ms: Some(1_000_000),
            duration_ms: None,
            error: None,
            warning: None,
            session_file: Some(format!("/sessions/{id}.oms").into()),
        }
    }

    #[test]
    fn the_root_leads_and_sub_agents_follow_in_spawn_order() {
        let root = RootAgent {
            title: "Refactor the parser".into(),
            state: AgentState::RunningTool("task".into()),
            elapsed: Some(Duration::from_secs(134)),
            model: "openai-codex/gpt-6-astra".into(),
        };
        let mut done = subagent("reviewer", 1, "reviewer", "completed");
        done.duration_ms = Some(12_000);
        let mut failed = subagent("scout-2", 2, "scout", "failed");
        failed.error = Some("child exhausted its budget".into());
        failed.duration_ms = Some(30_000);
        let subagents = [done, failed, subagent("scout", 0, "scout", "running")];
        let rows = agent_rows(&root, &subagents, 1_063_000, FinishedFilter::All);
        let lines: Vec<String> = rows
            .iter()
            .map(|row| format!("{} — {}", row.title(), row.subtitle_line()))
            .collect();
        // The one still running comes first, whatever its spawn order; the
        // finished ones follow, newest first.
        assert_eq!(
            lines,
            vec![
                "Refactor the parser — running task · gpt-6-astra · 2m 14s",
                "Running — ",
                "scout — working · scout · 1m 03s",
                "Finished — ",
                "scout-2 — failed · scout · 30s",
                "reviewer — done · reviewer · 12s",
            ]
        );
        assert_eq!(rows[2].state().dot(), Dot::Working);
        assert!(rows[2].elapsed().is_some(), "a running sub-agent ticks");
        assert!(
            rows[5].elapsed().is_none(),
            "a settled one holds its duration"
        );
        let AgentRow::Subagent { error, .. } = &rows[4] else {
            panic!("fifth row is the failed sub-agent");
        };
        assert_eq!(error.as_deref(), Some("child exhausted its budget"));
        assert!(matches!(
            rows[3],
            AgentRow::Section {
                section: Section::Finished,
                count: 2,
                shown: 2
            }
        ));
    }

    #[test]
    fn the_finished_filter_narrows_the_group_but_the_heading_still_counts_it() {
        let root = RootAgent {
            title: "Session".into(),
            state: AgentState::Idle,
            elapsed: None,
            model: String::new(),
        };
        let mut done = subagent("a", 0, "task", "completed");
        done.started_ms = Some(1_000);
        done.duration_ms = Some(500);
        let mut failed = subagent("b", 1, "task", "failed");
        failed.started_ms = Some(2_000);
        failed.duration_ms = Some(500);
        let mut cut_off = subagent("c", 2, "task", "cancelled");
        cut_off.started_ms = Some(3_000);
        cut_off.duration_ms = Some(500);
        let subagents = [done, failed, cut_off];
        let titles = |filter| {
            agent_rows(&root, &subagents, 10_000, filter)
                .iter()
                .skip(2)
                .map(|row| row.title().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(FinishedFilter::All), ["c", "b", "a"]);
        assert_eq!(titles(FinishedFilter::Succeeded), ["a"]);
        assert_eq!(titles(FinishedFilter::Failed), ["c", "b"]);
        let rows = agent_rows(&root, &subagents, 10_000, FinishedFilter::Succeeded);
        assert!(matches!(
            rows[1],
            AgentRow::Section {
                section: Section::Finished,
                count: 3,
                shown: 1
            }
        ));
    }

    #[test]
    fn a_done_sub_agent_whose_changes_were_not_merged_says_so() {
        let root = RootAgent {
            title: "Session".into(),
            state: AgentState::Idle,
            elapsed: None,
            model: String::new(),
        };
        let mut done = subagent("a", 0, "task", "completed");
        done.duration_ms = Some(30_000);
        done.warning = Some("changes were not merged back: document server rejected".into());
        let rows = agent_rows(&root, &[done], 0, FinishedFilter::All);
        assert_eq!(rows[2].subtitle_line(), "done · task · 30s · needs review");
        assert_eq!(rows[2].state().dot(), Dot::Done);
    }

    #[test]
    fn a_detached_root_says_so_without_a_model_or_clock() {
        let root = RootAgent {
            title: "Old session".into(),
            state: AgentState::Detached,
            elapsed: Some(Duration::from_secs(5)),
            model: "openai-codex/gpt-6-astra".into(),
        };
        let rows = agent_rows(&root, &[], 0, FinishedFilter::All);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].subtitle_line(), "not attached");
        assert_eq!(rows[0].state().dot(), Dot::Idle);
    }

    #[test]
    fn the_summary_counts_by_state() {
        let subagents = [
            subagent("a", 0, "scout", "running"),
            subagent("b", 1, "scout", "starting"),
            subagent("c", 2, "reviewer", "completed"),
            subagent("d", 3, "reviewer", "failed"),
            subagent("e", 4, "reviewer", "cancelled"),
        ];
        assert_eq!(
            summary_line(&subagents),
            "2 running · 1 done · 1 failed · 1 stopped"
        );
        assert_eq!(summary_line(&[]), "");
    }

    #[test]
    fn completion_warnings_stay_visible_in_the_summary() {
        let mut review = subagent("review", 0, "reviewer", "completed");
        review.warning = Some("Changes require review".into());
        assert_eq!(
            summary_line(&[review, subagent("done", 1, "scout", "completed")]),
            "1 needs review · 1 done"
        );
    }

    fn test_root() -> RootAgent {
        RootAgent {
            title: "Root".into(),
            state: AgentState::Idle,
            elapsed: None,
            model: "test".into(),
        }
    }

    #[test]
    fn attention_includes_failed_interrupted_and_completed_warnings() {
        let mut review = subagent("review", 0, "reviewer", "completed");
        review.warning = Some("merge needs review".into());
        let mut errored_completion = subagent("error", 1, "task", "completed");
        errored_completion.error = Some("workspace sync failed".into());
        let board = [
            review,
            errored_completion,
            subagent("failed", 2, "task", "failed"),
            subagent("stopped", 3, "task", "cancelled"),
            subagent("fine", 4, "scout", "completed"),
        ];
        let view = ViewState {
            scope: WorkScope::Attention,
            finished_collapsed: true,
            ..ViewState::default()
        };
        let work = visible_work(&test_root(), &board, 0, &view);
        let ids: Vec<_> = work
            .rows
            .iter()
            .filter_map(|row| {
                if let AgentRow::Subagent { id, .. } = row {
                    Some(id.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(work.matched, 4);
        assert!(
            ids.contains(&"review")
                && ids.contains(&"error")
                && ids.contains(&"failed")
                && ids.contains(&"stopped")
        );
        assert!(!ids.contains(&"fine"));
    }

    #[test]
    fn search_reveals_collapsed_results_and_matches_all_words_across_fields() {
        let mut issue = subagent("authentication", 0, "reviewer", "failed");
        issue.error = Some("Token expired".into());
        let board = [issue, subagent("rendering", 1, "reviewer", "running")];
        let view = ViewState {
            query: "REVIEWER token".into(),
            finished_collapsed: true,
            ..ViewState::default()
        };
        let work = visible_work(&test_root(), &board, 0, &view);
        assert_eq!(work.matched, 1);
        assert!(matches!(work.rows.first(), Some(AgentRow::Root { .. })));
        assert!(
            matches!(work.rows.last(), Some(AgentRow::Subagent { id, .. }) if id == "authentication")
        );
        let none = visible_work(
            &test_root(),
            &board,
            0,
            &ViewState {
                query: "nonexistent".into(),
                ..view
            },
        );
        assert_eq!(none.rows.len(), 1);
    }

    #[test]
    fn large_workloads_have_bounded_pages_without_missing_or_duplicate_tasks() {
        let board: Vec<_> = (0..107)
            .map(|i| {
                subagent(
                    &format!("task-{i}"),
                    i,
                    "task",
                    if i < 52 { "running" } else { "completed" },
                )
            })
            .collect();
        let mut ids = std::collections::HashSet::new();
        for page in 0..3 {
            let work = visible_work(
                &test_root(),
                &board,
                0,
                &ViewState {
                    page,
                    ..ViewState::default()
                },
            );
            assert_eq!(work.pages, 3);
            assert_eq!(work.matched, 107);
            let tasks: Vec<_> = work
                .rows
                .iter()
                .filter_map(|row| {
                    if let AgentRow::Subagent { id, .. } = row {
                        Some(id)
                    } else {
                        None
                    }
                })
                .collect();
            assert!(tasks.len() <= 40);
            for id in tasks {
                assert!(ids.insert(id.clone()));
            }
            for (i, row) in work.rows.iter().enumerate() {
                if matches!(row, AgentRow::Section { .. }) {
                    assert!(matches!(
                        work.rows.get(i + 1),
                        Some(AgentRow::Subagent { .. })
                    ));
                }
            }
        }
        assert_eq!(ids.len(), board.len());
        let work = visible_work(
            &test_root(),
            &board[..1],
            0,
            &ViewState {
                page: 90,
                ..ViewState::default()
            },
        );
        assert_eq!(work.page, 0);
    }

    #[test]
    fn collapse_and_active_lenses_keep_the_primary_agent_and_running_tasks() {
        let board = [
            subagent("active", 0, "task", "running"),
            subagent("done", 1, "task", "completed"),
        ];
        let collapsed = visible_work(
            &test_root(),
            &board,
            0,
            &ViewState {
                finished_collapsed: true,
                ..ViewState::default()
            },
        );
        assert_eq!(collapsed.rows.len(), 4);
        assert!(matches!(
            collapsed.rows.last(),
            Some(AgentRow::Section {
                section: Section::Finished,
                ..
            })
        ));
        let active = visible_work(
            &test_root(),
            &board,
            0,
            &ViewState {
                scope: WorkScope::Active,
                ..ViewState::default()
            },
        );
        assert_eq!(active.matched, 1);
        assert_eq!(active.rows.len(), 3);
    }

    #[test]
    fn durations_read_the_way_people_say_them() {
        assert_eq!(elapsed_text(Duration::from_secs(47)), "47s");
        assert_eq!(elapsed_text(Duration::from_secs(134)), "2m 14s");
        assert_eq!(elapsed_text(Duration::from_secs(3780)), "1h 03m");
        assert_eq!(AgentState::from_status("running").label(), "working");
        assert_eq!(AgentState::from_status("cancelled").label(), "interrupted");
    }
}
