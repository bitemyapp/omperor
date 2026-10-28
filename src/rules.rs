//! The app's rules that need no interface: which accounts serve a
//! provider, how the sidebar groups sessions, what a fork's branch is
//! called, which models the picker offers, how an account's plan and
//! limits read. Pure, so each is tested on its own.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::mock_inference;
use crate::omp_runtime::{self, AccountSummary, AuthProvider};
use crate::preferences::{Project, SessionRecord, StoredAccount};
use crate::session::SessionSummary;

/// The most models the composer's picker lists.
pub(crate) const MAX_MODEL_PICKER_ITEMS: usize = 128;

pub(crate) fn wall_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// The wall clock at an `Instant` that has already passed.
pub(crate) fn wall_clock_ms_at(at: std::time::Instant) -> u64 {
    wall_clock_ms().saturating_sub(u64::try_from(at.elapsed().as_millis()).unwrap_or(0))
}

/// The last component of a path, for a status line.
pub(crate) fn path_leaf(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// The local time `unix_ms` names, broken down.
fn local_time(unix_ms: i64) -> Option<libc::tm> {
    let seconds = libc::time_t::try_from(unix_ms.div_euclid(1000)).ok()?;
    // SAFETY: `localtime_r` writes only the `tm` it is given and reads the
    // time it is pointed at; both live on this stack frame.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        (!libc::localtime_r(&seconds, &mut tm).is_null()).then_some(tm)
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "27 Oct 2026, 20:05": a moment, as the account list and tooltips give it.
pub(crate) fn format_timestamp(unix_ms: i64) -> String {
    match local_time(unix_ms) {
        Some(tm) => format!(
            "{} {} {}, {:02}:{:02}",
            tm.tm_mday,
            MONTHS[tm.tm_mon.clamp(0, 11) as usize],
            tm.tm_year + 1900,
            tm.tm_hour,
            tm.tm_min
        ),
        None => "an unknown time".to_owned(),
    }
}

/// "Resets 03:14" within the day, otherwise with the date as well.
pub(crate) fn reset_text(at_ms: u64) -> String {
    let now = wall_clock_ms();
    let within_day = at_ms > now && at_ms - now < 24 * 3600 * 1000;
    let Some(tm) = local_time(i64::try_from(at_ms).unwrap_or(i64::MAX)) else {
        return "Resets later".to_owned();
    };
    if within_day {
        format!("Resets {:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        format!("Resets {} {}, {:02}:{:02}", tm.tm_mday, MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_hour, tm.tm_min)
    }
}

/// "3 min ago", "yesterday": how long since `at_ms`, for the sidebar.
pub(crate) fn ago(at_ms: i64, now_ms: i64) -> String {
    let seconds = (now_ms - at_ms).max(0) / 1000;
    match seconds {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        86_400..604_800 => format!("{}d", seconds / 86_400),
        _ => format!("{}w", seconds / 604_800),
    }
}

/// Whether a finished tool could have changed the working tree. Unknown
/// tools count as mutating so the Changes panel errs toward refreshing.
pub(crate) fn tool_may_change_files(name: Option<&str>) -> bool {
    !matches!(
        name,
        Some(
            "read"
                | "grep"
                | "glob"
                | "think"
                | "debug"
                | "todo"
                | "ask"
                | "web_search"
                | "recall"
                | "reflect"
                | "goal"
                | "report_issue"
        )
    )
}

/// One account OMP stores, joined with the metadata Omperor keeps for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountEntry {
    pub(crate) summary: AccountSummary,
    pub(crate) label: Option<String>,
    pub(crate) authenticated_at_ms: i64,
    pub(crate) is_default: bool,
}

impl AccountEntry {
    pub(crate) fn joined(summary: AccountSummary, stored: &[StoredAccount]) -> Self {
        let metadata = stored.iter().find(|account| account.account_id == summary.id);
        Self {
            label: metadata.and_then(|account| account.label.clone()),
            authenticated_at_ms: metadata.map_or(0, |account| account.authenticated_at_ms),
            is_default: metadata.is_some_and(|account| account.is_default),
            summary,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.summary.id
    }

    pub(crate) fn provider(&self) -> AuthProvider {
        self.summary.provider
    }

    pub(crate) fn is_usable(&self) -> bool {
        self.summary.state.is_usable()
    }

    /// The name shown wherever the account is chosen: the user's label when
    /// one exists, otherwise the provider-side identity.
    pub(crate) fn display_name(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.identity())
    }

    /// Provider-side identity: the subscriber email when the provider reports
    /// one, otherwise when the account was authenticated through Omperor.
    pub(crate) fn identity(&self) -> String {
        self.summary.email().map_or_else(
            || format!("Authenticated {}", format_timestamp(self.authenticated_at_ms)),
            str::to_owned,
        )
    }
}

/// Outcome of choosing which stored account should serve a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountResolution {
    /// The provider has no usable account.
    Unavailable,
    Chosen(String),
    /// Several usable accounts and nothing that singles one out.
    NeedsChoice,
}

/// Picks the account a provider should serve from, in priority order: the
/// workspace's remembered choice, the account already pinned on the session,
/// the only usable account, then the provider default. Anything else needs
/// the user to choose.
pub(crate) fn resolve_provider_account(
    accounts: &[AccountEntry],
    provider: AuthProvider,
    project_choice: Option<&str>,
    current: Option<&str>,
) -> AccountResolution {
    let usable = accounts.iter().filter(|entry| entry.provider() == provider && entry.is_usable()).collect::<Vec<_>>();
    if usable.is_empty() {
        return AccountResolution::Unavailable;
    }
    let is_usable = |id: &&str| usable.iter().any(|entry| entry.id() == *id);
    if let Some(id) = project_choice.filter(is_usable).or_else(|| current.filter(is_usable)) {
        return AccountResolution::Chosen(id.to_owned());
    }
    if let [only] = usable.as_slice() {
        return AccountResolution::Chosen(only.id().to_owned());
    }
    match usable.iter().find(|entry| entry.is_default) {
        Some(default) => AccountResolution::Chosen(default.id().to_owned()),
        None => AccountResolution::NeedsChoice,
    }
}

/// Labels are obligatory and must disambiguate: every new label is non-empty
/// and no two labels for a provider match, ignoring case.
pub(crate) fn labels_are_valid(new_labels: &[String], existing_labels: &[String]) -> bool {
    let mut seen = existing_labels.iter().map(|label| label.trim().to_lowercase()).collect::<HashSet<_>>();
    new_labels.iter().all(|label| {
        let label = label.trim();
        !label.is_empty() && seen.insert(label.to_lowercase())
    })
}

/// Replaces OMP's opaque account identifiers in a message with the names the
/// user knows the accounts by, so "no credential is stored for openai-codex
/// account openai-codex:83dd…" reads as "… account “Work”". An account the
/// user has neither labelled nor whose provider reports an email keeps its
/// identifier, which is still what the account menu shows.
pub(crate) fn name_accounts_in(text: &str, accounts: &[AccountEntry]) -> String {
    let mut named = text.to_owned();
    for entry in accounts {
        let Some(name) = entry.label.clone().or_else(|| entry.summary.email().map(str::to_owned)) else {
            continue;
        };
        if named.contains(entry.id()) {
            named = named.replace(entry.id(), &format!("“{name}”"));
        }
    }
    named
}

pub(crate) fn model_change_requires_new_session(current: &str, next: &str) -> bool {
    let current = AuthProvider::for_model(current);
    let next = AuthProvider::for_model(next);
    current != next && (current.is_some() || next.is_some())
}

/// The models the composer offers: the supported subscription models of
/// providers with an account signed in, the active one first.
pub(crate) fn model_picker_options<'a>(
    models: &'a [String],
    active: &'a str,
    authenticated_providers: &HashSet<AuthProvider>,
) -> Vec<&'a str> {
    if active == mock_inference::MODEL && models.iter().any(|model| model == active) {
        return vec![active];
    }
    let mut options = Vec::with_capacity(MAX_MODEL_PICKER_ITEMS.min(models.len() + 1));
    let mut seen = HashSet::with_capacity(options.capacity());
    let mut add = |model: &'a str| {
        if options.len() < MAX_MODEL_PICKER_ITEMS && !model.trim().is_empty() && seen.insert(model) {
            options.push(model);
        }
    };
    if models.iter().any(|model| model == active)
        && AuthProvider::for_model(active).is_some_and(|provider| authenticated_providers.contains(&provider))
        && omp_runtime::is_supported_subscription_model(active)
    {
        add(active);
    }
    for provider in AuthProvider::ALL {
        if !authenticated_providers.contains(&provider) {
            continue;
        }
        for model in models.iter().map(String::as_str).filter(|model| {
            AuthProvider::for_model(model) == Some(provider) && omp_runtime::is_supported_subscription_model(model)
        }) {
            add(model);
        }
    }
    options
}

/// Suggests a branch name for a fork: a slug of the session title plus a
/// short suffix from the session id so sibling forks do not collide.
pub(crate) fn default_fork_branch_name(title: &str, session_id: &str) -> String {
    const MAX_SLUG: usize = 32;
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
        } else {
            pending_dash = true;
        }
        if slug.len() > MAX_SLUG {
            break;
        }
    }
    // Cut an overlong slug at a word boundary rather than mid-word.
    if slug.len() > MAX_SLUG {
        let cut = slug[..MAX_SLUG].rfind('-').unwrap_or(MAX_SLUG);
        slug.truncate(cut);
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "session" } else { slug };
    let suffix =
        session_id.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>().to_lowercase();
    format!("omperor/{slug}-{suffix}")
}

/// Names a new project after its folder, disambiguating against existing
/// project names with the parent folder and then a counter.
pub(crate) fn unique_project_name(directory: &Path, projects: &[Project]) -> String {
    let taken = |candidate: &str| projects.iter().any(|project| project.name.eq_ignore_ascii_case(candidate));
    let base = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| directory.display().to_string());
    if !taken(&base) {
        return base;
    }
    if let Some(parent) = directory.parent().and_then(Path::file_name).map(|name| name.to_string_lossy()) {
        let qualified = format!("{base} ({parent})");
        if !taken(&qualified) {
            return qualified;
        }
    }
    (2..).map(|counter| format!("{base} {counter}")).find(|candidate| !taken(candidate)).expect("a free name")
}

/// The creation time a ULID session id carries in its first ten characters.
pub(crate) fn ulid_timestamp_ms(id: &str) -> Option<i64> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if id.len() != 26 {
        return None;
    }
    id.bytes().take(10).try_fold(0i64, |time, byte| {
        let digit = ALPHABET.iter().position(|&c| c == byte.to_ascii_uppercase())?;
        time.checked_mul(32)?.checked_add(digit as i64)
    })
}

/// Which sessions the sidebar lists: Omperor's own, or the ones OMP's
/// terminal interface made (read-only here).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum SessionSourceFilter {
    #[default]
    Omperor,
    Omp,
}

impl SessionSourceFilter {
    pub(crate) fn from_segment(segment: usize) -> Self {
        match segment {
            1 => Self::Omp,
            _ => Self::Omperor,
        }
    }

    pub(crate) fn includes(self, omperor_owned: bool) -> bool {
        match self {
            Self::Omperor => omperor_owned,
            Self::Omp => !omperor_owned,
        }
    }
}

/// One row of the sidebar's session list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarRow {
    /// A project heading; toggles collapse when clicked.
    Project { project_id: String },
    /// Heading for sessions whose directory belongs to no project.
    Unfiled,
    ShowMore { project_id: String, hidden: usize, expanded: bool },
    /// A session, indexed into the app's session list. `project_id` is set
    /// when the session sits under a project and `None` for the flat
    /// read-only list and unfiled sessions.
    Session { index: usize, project_id: Option<String> },
}

/// Which project a session lists under: the one its record names, else the
/// first whose folders, or whose sessions' folders and worktrees, include
/// the session's folder. `None` files it under Other folders.
pub(crate) fn session_project_position(
    summary: &SessionSummary,
    projects: &[Project],
    records: &HashMap<String, SessionRecord>,
) -> Option<usize> {
    if let Some(position) = records
        .get(&summary.id)
        .and_then(|record| projects.iter().position(|project| project.id == record.project_id))
    {
        return Some(position);
    }
    let cwd = summary.cwd.as_deref()?;
    projects.iter().position(|project| {
        project.directories.iter().any(|directory| directory == cwd)
            || records.values().any(|record| {
                record.project_id == project.id
                    && (record.directory == cwd || record.worktree_path.as_deref() == Some(cwd))
            })
    })
}

/// Groups the sidebar list. Omperor sessions sit under their project
/// (forks are not nested; lineage lives in the tooltip), and sessions no
/// project owns under "Other folders"; the read-only OMP list stays flat. A
/// search hides projects without a match and ignores collapse state.
pub(crate) fn sidebar_rows_for(
    projects: &[Project],
    sessions: &[SessionSummary],
    records: &HashMap<String, SessionRecord>,
    session_positions: &HashMap<String, i64>,
    filter: SessionSourceFilter,
    query: &str,
) -> Vec<SidebarRow> {
    let matches = |summary: &SessionSummary| {
        query.is_empty() || summary.title.to_lowercase().contains(query) || summary.project.to_lowercase().contains(query)
    };
    let mut rows = Vec::new();
    if filter == SessionSourceFilter::Omp {
        rows.extend(
            sessions
                .iter()
                .enumerate()
                .filter(|(_, summary)| filter.includes(summary.is_omperor_owned()) && matches(summary))
                .map(|(index, _)| SidebarRow::Session { index, project_id: None }),
        );
        return rows;
    }
    let mut members = vec![Vec::new(); projects.len()];
    let mut unfiled = Vec::new();
    for (index, summary) in sessions.iter().enumerate() {
        if !summary.is_omperor_owned() || !matches(summary) {
            continue;
        }
        match session_project_position(summary, projects, records) {
            Some(position) => members[position].push(index),
            None => unfiled.push(index),
        }
    }
    for (position, project) in projects.iter().enumerate() {
        let mut sessions_here = std::mem::take(&mut members[position]);
        if !query.is_empty() && sessions_here.is_empty() {
            continue;
        }
        // Sessions the user has arranged hold that order. The rest sit
        // above them, newest created first: ordering by creation rather
        // than by last activity means a session finishing a turn never
        // jumps rows. Sessions with no record keep the list's order.
        let created = |index: usize| {
            records
                .get(&sessions[index].id)
                .map(|record| record.created_at_ms)
                .or_else(|| ulid_timestamp_ms(&sessions[index].id))
        };
        sessions_here.sort_by(|&left, &right| {
            use std::cmp::Ordering;
            match (session_positions.get(&sessions[left].id), session_positions.get(&sessions[right].id)) {
                (None, None) => match (created(left), created(right)) {
                    (Some(left), Some(right)) => right.cmp(&left),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                },
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(left), Some(right)) => left.cmp(right),
            }
        });
        rows.push(SidebarRow::Project { project_id: project.id.clone() });
        if project.collapsed && query.is_empty() {
            continue;
        }
        rows.extend(
            sessions_here.into_iter().map(|index| SidebarRow::Session { index, project_id: Some(project.id.clone()) }),
        );
    }
    if !unfiled.is_empty() {
        rows.push(SidebarRow::Unfiled);
        rows.extend(unfiled.into_iter().map(|index| SidebarRow::Session { index, project_id: None }));
    }
    rows
}

/// How many sessions a project lists before "Show N more".
pub(crate) const PROJECT_SESSION_LIMIT: usize = 5;

/// Keep the first five sessions of each project in their order, plus the
/// selected and running ones. Search results bypass folding at the caller;
/// no session is removed from the registry or its stored order.
pub(crate) fn fold_rows(
    rows: Vec<SidebarRow>,
    expanded: &HashSet<String>,
    keep_visible: &HashSet<usize>,
) -> Vec<SidebarRow> {
    let mut result = Vec::with_capacity(rows.len());
    let mut rows = rows.into_iter().peekable();
    while let Some(row) = rows.next() {
        let SidebarRow::Project { project_id } = &row else {
            result.push(row);
            continue;
        };
        let project_id = project_id.clone();
        result.push(row);
        let mut members = Vec::new();
        while matches!(rows.peek(), Some(SidebarRow::Session { project_id: Some(id), .. }) if id == &project_id) {
            members.push(rows.next().expect("peeked"));
        }
        let is_expanded = expanded.contains(&project_id);
        let pinned = |row: &SidebarRow| matches!(row, SidebarRow::Session { index, .. } if keep_visible.contains(index));
        let hidden = members
            .iter()
            .enumerate()
            .filter(|(position, row)| *position >= PROJECT_SESSION_LIMIT && !pinned(row))
            .count();
        result.extend(
            members
                .into_iter()
                .enumerate()
                .filter_map(|(position, row)| (is_expanded || position < PROJECT_SESSION_LIMIT || pinned(&row)).then_some(row)),
        );
        if hidden > 0 {
            result.push(SidebarRow::ShowMore { project_id, hidden, expanded: is_expanded });
        }
    }
    result
}

pub(crate) fn project_tooltip(project: &Project) -> String {
    if project.directories.is_empty() {
        return format!("{} — no folders", project.name);
    }
    let folders = project.directories.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join("\n");
    format!("{}\n{folders}", project.name)
}

pub(crate) fn session_tooltip(summary: &SessionSummary, record: Option<&SessionRecord>) -> String {
    let mut tooltip = summary.menu_label();
    if let Some(model) = summary.model.as_deref() {
        tooltip.push_str(&format!("\nmodel {}", crate::agents::short_model(model)));
    }
    if let Some(record) = record {
        if let Some(branch) = &record.branch {
            tooltip.push_str(&format!("\nbranch {branch}"));
            if let Some(base) = &record.base_branch {
                tooltip.push_str(&format!(" forked from {base}"));
            }
        }
        if let Some(worktree) = &record.worktree_path {
            tooltip.push_str(&format!("\nworktree {}", worktree.display()));
        }
    }
    tooltip
}

// Account cards.

pub(crate) const GENERAL_USAGE_TITLE: &str = "General usage limits";
pub(crate) const EXTRA_USAGE_TITLE: &str = "Extra usage";

/// How an account card's plan line reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanTone {
    Known,
    Pending,
    Warning,
}

/// What an account card shows: its plan, a note, and its limit windows
/// grouped by what they meter.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AccountCardPlan {
    pub(crate) plan: String,
    pub(crate) tone: PlanTone,
    pub(crate) note: Option<String>,
    pub(crate) note_is_error: bool,
    pub(crate) groups: Vec<UsageGroupPlan>,
    /// The window closest to running out, shown while the card is folded.
    pub(crate) summary: Option<UsageGroupPlan>,
    pub(crate) expanded: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageGroupPlan {
    pub(crate) title: String,
    pub(crate) rows: Vec<UsageRowPlan>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageRowPlan {
    pub(crate) title: String,
    pub(crate) detail: Option<String>,
    pub(crate) left_percent: Option<u8>,
    pub(crate) figure: Option<String>,
}

impl AccountCardPlan {
    /// Whether folding would hide anything.
    pub(crate) fn foldable(&self) -> bool {
        self.groups.iter().map(|group| group.rows.len()).sum::<usize>() > 1
    }

    /// The groups the card shows now.
    pub(crate) fn shown_groups(&self) -> Vec<&UsageGroupPlan> {
        match (&self.summary, self.expanded || !self.foldable()) {
            (Some(summary), false) => vec![summary],
            _ => self.groups.iter().collect(),
        }
    }
}

pub(crate) fn account_card_plan(
    provider: AuthProvider,
    usage: Option<&omp_runtime::AccountUsage>,
    loading: bool,
    usable: bool,
    expanded: bool,
) -> AccountCardPlan {
    let Some(usage) = usage else {
        return AccountCardPlan {
            plan: if !usable {
                "Unknown until the account is signed in".to_owned()
            } else if loading {
                "Checking with the provider…".to_owned()
            } else {
                "Not checked yet".to_owned()
            },
            tone: PlanTone::Pending,
            note: None,
            note_is_error: false,
            groups: Vec::new(),
            summary: None,
            expanded,
        };
    };
    if let Some(error) = &usage.error {
        // The provider's usage endpoint needs a credential OMP does not hold
        // for this sign-in (an API key, say); the account still works.
        let benign = error.contains("credential_unavailable");
        let note = if benign {
            "This sign-in has no credential OMP can ask the provider's usage service with.".to_owned()
        } else {
            error.clone()
        };
        return AccountCardPlan {
            plan: "Unavailable".to_owned(),
            tone: if benign { PlanTone::Pending } else { PlanTone::Warning },
            note: Some(note),
            note_is_error: !benign,
            groups: Vec::new(),
            summary: None,
            expanded,
        };
    }
    let mut groups: Vec<UsageGroupPlan> = Vec::new();
    let mut worst_used = 0u8;
    // The rate window closest to running out, which is what a folded card
    // shows; money is a cap, not a window, so it never stands in for one.
    let mut tightest: Option<(u8, UsageRowPlan)> = None;
    for window in &usage.windows {
        let title = usage_group_title(window.scope.as_deref(), &window.label);
        let row = match window.spend {
            Some(spend) => spend_row(spend, window.used_percent),
            None => UsageRowPlan {
                title: usage_row_title(&window.label),
                detail: window.resets_at_ms.map(reset_text),
                left_percent: window.used_percent.map(|used| 100u8.saturating_sub(used)),
                figure: None,
            },
        };
        if let Some(used) = window.used_percent {
            worst_used = worst_used.max(used);
            if window.spend.is_none() && tightest.as_ref().is_none_or(|(worst, _)| used > *worst) {
                let mut summary = row.clone();
                if title != GENERAL_USAGE_TITLE {
                    // Which feature's window it is, since the group heading
                    // that would say so is folded away.
                    let feature = title.trim_end_matches(" usage limits");
                    summary.title = format!("{} ({feature})", summary.title);
                }
                tightest = Some((used, summary));
            }
        }
        match groups.iter_mut().find(|group| group.title == title) {
            Some(group) => group.rows.push(row),
            None => groups.push(UsageGroupPlan { title, rows: vec![row] }),
        }
    }
    // The account as a whole comes before any one feature's limits.
    groups.sort_by_key(|group| group.title != GENERAL_USAGE_TITLE);
    let summary = tightest.map(|(_, row)| UsageGroupPlan { title: "Usage".to_owned(), rows: vec![row] });
    AccountCardPlan {
        plan: plan_display_name(provider, usage.plan.as_deref()),
        tone: if worst_used >= 90 { PlanTone::Warning } else { PlanTone::Known },
        note: None,
        note_is_error: false,
        groups,
        summary,
        expanded,
    }
}

/// "General usage limits" for the account's own windows; a metered feature
/// is named after the parenthetical its windows carry ("Spark usage limits",
/// "Fable usage limits"); pay-as-you-go spend past the plan is "Extra usage".
pub(crate) fn usage_group_title(scope: Option<&str>, label: &str) -> String {
    match scope {
        None | Some("shared") => GENERAL_USAGE_TITLE.to_owned(),
        Some("extra") => EXTRA_USAGE_TITLE.to_owned(),
        Some(scope) => match parenthetical(label) {
            Some(feature) => format!("{feature} usage limits"),
            None => format!("{} usage limits", capitalized(scope)),
        },
    }
}

/// "5 hours (Spark)" reads as "5-hour limit"; a week as "Weekly limit". A
/// provider's own name in front of the span ("Claude 7 Day") is dropped.
pub(crate) fn usage_row_title(label: &str) -> String {
    let span = match label.find(" (") {
        Some(end) => &label[..end],
        None => label,
    };
    let span = span.trim();
    let Some(start) = span.find(|c: char| c.is_ascii_digit()) else {
        return format!("{span} limit");
    };
    let span = span[start..].to_ascii_lowercase();
    match span.split_once(' ') {
        Some((count, unit)) if count.chars().all(|c| c.is_ascii_digit()) => {
            let unit = unit.trim().trim_end_matches('s');
            match (count, unit) {
                ("1", "day") => "Daily limit".to_owned(),
                ("7", "day") => "Weekly limit".to_owned(),
                ("30", "day") => "Monthly limit".to_owned(),
                _ => format!("{count}-{unit} limit"),
            }
        }
        _ => format!("{span} limit"),
    }
}

/// A billing window as money: what was spent against the cap, and what is
/// left of it. Without a cap there is nothing to show as remaining.
fn spend_row(spend: omp_runtime::Spend, used_percent: Option<u8>) -> UsageRowPlan {
    match spend.limit_cents {
        Some(limit) => UsageRowPlan {
            title: "Monthly spending limit".to_owned(),
            detail: Some(format!("{} of {} spent", dollars(spend.used_cents), dollars(limit))),
            left_percent: used_percent.map(|used| 100u8.saturating_sub(used)),
            figure: Some(format!("{} left", dollars(limit.saturating_sub(spend.used_cents)))),
        },
        None => UsageRowPlan {
            title: "Spending, uncapped".to_owned(),
            detail: Some(format!("{} spent", dollars(spend.used_cents))),
            left_percent: None,
            figure: None,
        },
    }
}

/// "$1,234.56".
pub(crate) fn dollars(cents: u64) -> String {
    let whole = (cents / 100).to_string();
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("${grouped}.{:02}", cents % 100)
}

pub(crate) fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub(crate) fn parenthetical(label: &str) -> Option<&str> {
    let start = label.find('(')? + 1;
    let end = label[start..].find(')')? + start;
    let inner = label[start..end].trim();
    (!inner.is_empty()).then_some(inner)
}

/// The plan as a person names it. OpenAI reports ChatGPT plan slugs; OMP
/// reads Claude's from the account profile (`max_20x`, `pro`, `team`…).
pub(crate) fn plan_display_name(provider: AuthProvider, plan: Option<&str>) -> String {
    let plan = plan.map(str::to_ascii_lowercase);
    match provider {
        AuthProvider::OpenAi => match plan.as_deref() {
            Some("pro") => "ChatGPT Pro".to_owned(),
            Some("plus") => "ChatGPT Plus".to_owned(),
            Some("team") => "ChatGPT Team".to_owned(),
            Some("business") => "ChatGPT Business".to_owned(),
            Some("enterprise") => "ChatGPT Enterprise".to_owned(),
            Some("edu") => "ChatGPT Edu".to_owned(),
            Some("free") => "ChatGPT Free".to_owned(),
            Some(other) if !other.is_empty() => format!("ChatGPT {}", capitalized(other)),
            _ => "ChatGPT plan not reported".to_owned(),
        },
        AuthProvider::OpenAiApi => "API key, billed per token".to_owned(),
        AuthProvider::Anthropic => match plan.as_deref() {
            Some("max_20x") => "Claude Max 20×".to_owned(),
            Some("max_5x") => "Claude Max 5×".to_owned(),
            Some("max") => "Claude Max".to_owned(),
            Some("pro") => "Claude Pro".to_owned(),
            Some("team") => "Claude Team".to_owned(),
            Some("enterprise") => "Claude Enterprise".to_owned(),
            Some("free") => "Claude Free".to_owned(),
            Some(other) if !other.is_empty() => format!("Claude {}", capitalized(other)),
            _ => "Claude plan not reported".to_owned(),
        },
    }
}

pub(crate) const fn provider_account_detail(provider: AuthProvider) -> &'static str {
    match provider {
        AuthProvider::OpenAi => "Your ChatGPT subscription: sign in in your browser, or paste an access token.",
        AuthProvider::OpenAiApi => "An OpenAI API key, billed per token.",
        AuthProvider::Anthropic => "Your Claude subscription: sign in in your browser, or paste an API key.",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::omp_runtime::{AccountState, AccountUsage, UsageWindow};

    fn project(id: &str, name: &str, directories: &[&str], collapsed: bool) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            directories: directories.iter().map(Into::into).collect(),
            last_opened_at_ms: 0,
            collapsed,
        }
    }

    fn record(session_id: &str, project_id: &str, directory: &str, parent: Option<&str>) -> SessionRecord {
        SessionRecord {
            session_id: session_id.into(),
            project_id: project_id.into(),
            directory: directory.into(),
            parent_session_id: parent.map(str::to_owned),
            branch: parent.map(|_| "omperor/fork".to_owned()),
            base_branch: parent.map(|_| "main".to_owned()),
            worktree_path: parent.map(|_| directory.into()),
            created_at_ms: 0,
        }
    }

    #[test]
    fn ulid_session_ids_carry_their_creation_time() {
        assert_eq!(ulid_timestamp_ms("01M4A5FVJ4B2B29K6GJS3WZKAP"), Some(1_791_342_669_380));
        assert!(ulid_timestamp_ms("01M4A5FVJ4B2B29K6GJS3WZKAP") > ulid_timestamp_ms("01M30XF8KTV881T8J22B2W0K3B"));
        assert_eq!(ulid_timestamp_ms("research"), None);
    }

    #[test]
    fn sidebar_groups_sessions_under_projects_without_nesting_forks() {
        let projects = vec![project("p-app", "app", &["/work/app"], false), project("p-lib", "lib", &["/work/lib"], true)];
        let sessions = vec![
            SessionSummary::for_test("fork", "Fork task", Some("/wt/app/fork"), true),
            SessionSummary::for_test("root", "Root task", Some("/work/app"), true),
            SessionSummary::for_test("grandchild", "Deeper", Some("/wt/app/deeper"), true),
            SessionSummary::for_test("lib-session", "Lib task", Some("/work/lib"), true),
            SessionSummary::for_test("stray", "Elsewhere", Some("/elsewhere"), true),
            SessionSummary::for_test("tui", "TUI chat", Some("/work/app"), false),
        ];
        let created = |mut record: SessionRecord, at| {
            record.created_at_ms = at;
            record
        };
        let records = [
            created(record("root", "p-app", "/work/app", None), 10),
            created(record("fork", "p-app", "/wt/app/fork", Some("root")), 20),
            created(record("grandchild", "p-app", "/wt/app/deeper", Some("fork")), 30),
        ]
        .into_iter()
        .map(|record| (record.session_id.clone(), record))
        .collect::<HashMap<_, _>>();
        let positions = HashMap::new();
        let session =
            |index: usize, project: Option<&str>| SidebarRow::Session { index, project_id: project.map(str::to_owned) };
        let rows = sidebar_rows_for(&projects, &sessions, &records, &positions, SessionSourceFilter::Omperor, "");
        assert_eq!(
            rows,
            vec![
                SidebarRow::Project { project_id: "p-app".into() },
                session(2, Some("p-app")),
                session(0, Some("p-app")),
                session(1, Some("p-app")),
                SidebarRow::Project { project_id: "p-lib".into() },
                SidebarRow::Unfiled,
                session(4, None),
            ]
        );
        let ordered = HashMap::from([("grandchild".to_owned(), 0), ("fork".to_owned(), 1)]);
        let rows = sidebar_rows_for(&projects, &sessions, &records, &ordered, SessionSourceFilter::Omperor, "");
        let app_sessions: Vec<_> = rows
            .iter()
            .skip_while(|row| !matches!(row, SidebarRow::Session { .. }))
            .take_while(|row| matches!(row, SidebarRow::Session { .. }))
            .cloned()
            .collect();
        assert_eq!(app_sessions, vec![session(1, Some("p-app")), session(2, Some("p-app")), session(0, Some("p-app"))]);
        let rows = sidebar_rows_for(&projects, &sessions, &records, &positions, SessionSourceFilter::Omperor, "lib");
        assert_eq!(rows, vec![SidebarRow::Project { project_id: "p-lib".into() }, session(3, Some("p-lib"))]);
        let rows = sidebar_rows_for(&projects, &sessions, &records, &positions, SessionSourceFilter::Omp, "");
        assert_eq!(rows, vec![session(5, None)]);
    }

    #[test]
    fn long_projects_fold_to_five_sessions_keeping_the_selected_one() {
        let mut rows = vec![SidebarRow::Project { project_id: "p".into() }];
        rows.extend((0..8).map(|index| SidebarRow::Session { index, project_id: Some("p".into()) }));
        let folded = fold_rows(rows.clone(), &HashSet::new(), &HashSet::from([7]));
        assert_eq!(folded.len(), 1 + 5 + 1 + 1);
        assert_eq!(folded.last(), Some(&SidebarRow::ShowMore { project_id: "p".into(), hidden: 2, expanded: false }));
        let open = fold_rows(rows, &HashSet::from(["p".to_owned()]), &HashSet::new());
        assert_eq!(open.len(), 1 + 8 + 1);
    }

    #[test]
    fn fork_branch_names_are_slugs_with_a_session_suffix() {
        assert_eq!(
            default_fork_branch_name("Reply with exactly: pong!", "01M1MY90K25SRDQ29ZFCN2DGE9"),
            "omperor/reply-with-exactly-pong-dge9"
        );
        assert_eq!(
            default_fork_branch_name("Reply with exactly: account pin verified", "01M1MY90K25SRDQ29ZFCN2DGE9"),
            "omperor/reply-with-exactly-account-pin-dge9"
        );
        assert_eq!(default_fork_branch_name("   ", "ABCD"), "omperor/session-abcd");
        let long = default_fork_branch_name(&"word ".repeat(20), "XYZ1");
        assert!(long.len() <= "omperor/".len() + 32 + 5, "{long}");
        assert!(!long.contains("--") && !long.ends_with('-'));
    }

    #[test]
    fn project_names_come_from_folders_and_stay_unique() {
        let existing =
            vec![project("a", "omperor", &["/work/omperor"], false), project("b", "omperor (other)", &["/other/omperor"], false)];
        assert_eq!(unique_project_name(Path::new("/tmp/fresh"), &existing), "fresh");
        assert_eq!(unique_project_name(Path::new("/third/omperor"), &existing), "omperor (third)");
        assert_eq!(unique_project_name(Path::new("/other/omperor"), &existing), "omperor 2");
        assert_eq!(unique_project_name(Path::new("/"), &existing), "/");
    }

    fn account(id: &str, provider: AuthProvider, label: Option<&str>, is_default: bool, state: AccountState) -> AccountEntry {
        AccountEntry {
            summary: AccountSummary { id: id.into(), provider, principal: None, state },
            label: label.map(str::to_owned),
            authenticated_at_ms: 0,
            is_default,
        }
    }

    #[test]
    fn account_resolution_prefers_workspace_memory_then_session_pin_then_default() {
        let accounts = vec![
            account("anthropic:a", AuthProvider::Anthropic, Some("Personal"), false, AccountState::Active),
            account("anthropic:b", AuthProvider::Anthropic, Some("Work"), true, AccountState::Active),
            account("openai-codex:1", AuthProvider::OpenAi, None, false, AccountState::Active),
        ];
        let resolve = |project: Option<&str>, current: Option<&str>| {
            resolve_provider_account(&accounts, AuthProvider::Anthropic, project, current)
        };
        assert_eq!(resolve(Some("anthropic:a"), Some("anthropic:b")), AccountResolution::Chosen("anthropic:a".into()));
        assert_eq!(resolve(None, Some("anthropic:a")), AccountResolution::Chosen("anthropic:a".into()));
        assert_eq!(resolve(None, None), AccountResolution::Chosen("anthropic:b".into()));
        assert_eq!(resolve(Some("anthropic:missing"), Some("anthropic:a")), AccountResolution::Chosen("anthropic:a".into()));
        assert_eq!(
            resolve_provider_account(&accounts, AuthProvider::OpenAi, None, None),
            AccountResolution::Chosen("openai-codex:1".into())
        );
    }

    #[test]
    fn account_resolution_asks_when_nothing_singles_out_an_account() {
        let accounts = vec![
            account("anthropic:a", AuthProvider::Anthropic, Some("Personal"), false, AccountState::Active),
            account("anthropic:b", AuthProvider::Anthropic, Some("Work"), false, AccountState::RefreshRequired),
            account("anthropic:c", AuthProvider::Anthropic, Some("Old"), true, AccountState::Disabled),
        ];
        assert_eq!(resolve_provider_account(&accounts, AuthProvider::Anthropic, None, None), AccountResolution::NeedsChoice);
        assert_eq!(
            resolve_provider_account(&accounts, AuthProvider::Anthropic, Some("anthropic:c"), None),
            AccountResolution::NeedsChoice
        );
        assert_eq!(resolve_provider_account(&accounts, AuthProvider::OpenAi, None, None), AccountResolution::Unavailable);
    }

    #[test]
    fn an_account_card_names_its_plan_and_groups_its_windows() {
        let window = |label: &str, scope: &str, used: u8| UsageWindow {
            label: label.into(),
            scope: Some(scope.into()),
            used_percent: Some(used),
            resets_at_ms: None,
            spend: None,
        };
        let usage = AccountUsage {
            account_id: "openai-codex:a".into(),
            plan: Some("pro".into()),
            windows: vec![window("5 hours (Spark)", "spark", 0), window("7 days", "shared", 10), window("7 days (Spark)", "spark", 93)],
            error: None,
        };
        let card = account_card_plan(AuthProvider::OpenAi, Some(&usage), false, true, true);
        assert_eq!(card.plan, "ChatGPT Pro");
        assert_eq!(card.tone, PlanTone::Warning, "a nearly spent window warns");
        let titles = card.groups.iter().map(|group| group.title.as_str()).collect::<Vec<_>>();
        assert_eq!(titles, ["General usage limits", "Spark usage limits"]);
        assert_eq!(card.groups[0].rows[0].title, "Weekly limit");
        assert_eq!(card.groups[0].rows[0].left_percent, Some(90));
        assert_eq!(card.groups[1].rows[0].title, "5-hour limit");
        assert_eq!(card.groups[1].rows[1].left_percent, Some(7));
        assert_eq!(card.shown_groups().len(), 2, "unfolded, every group shows");
        let folded = account_card_plan(AuthProvider::OpenAi, Some(&usage), false, true, false);
        let shown = folded.shown_groups();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title, "Usage");
        assert_eq!(shown[0].rows[0].title, "Weekly limit (Spark)");
        assert_eq!(shown[0].rows[0].left_percent, Some(7));
        let single = AccountUsage {
            account_id: "openai-codex:s".into(),
            plan: Some("free".into()),
            windows: vec![window("30 days", "shared", 1)],
            error: None,
        };
        let card = account_card_plan(AuthProvider::OpenAi, Some(&single), false, true, false);
        assert!(!card.foldable());
        assert_eq!(card.shown_groups()[0].title, "General usage limits");
        assert_eq!(card.shown_groups()[0].rows[0].title, "Monthly limit");
        let pending = account_card_plan(AuthProvider::OpenAi, None, true, true, false);
        assert_eq!(pending.plan, "Checking with the provider…");
        assert!(pending.groups.is_empty() && pending.summary.is_none());
        let signed_out = account_card_plan(AuthProvider::OpenAi, None, false, false, false);
        assert_eq!(signed_out.plan, "Unknown until the account is signed in");
        let failed = AccountUsage {
            account_id: "anthropic:b".into(),
            plan: None,
            windows: Vec::new(),
            error: Some("protocol violation (console_usage_credential_unavailable)".into()),
        };
        let card = account_card_plan(AuthProvider::Anthropic, Some(&failed), false, true, false);
        assert_eq!(card.plan, "Unavailable");
        assert!(card.note.as_deref().is_some_and(|note| note.contains("no credential")));
        assert_eq!(usage_row_title("30 days"), "Monthly limit");
        assert_eq!(usage_row_title("1 day"), "Daily limit");
        assert_eq!(usage_row_title("12 hours (Spark)"), "12-hour limit");
        assert_eq!(usage_group_title(None, "7 days"), "General usage limits");
        assert_eq!(usage_group_title(Some("spark"), "7 days (Spark)"), "Spark usage limits");
        assert_eq!(plan_display_name(AuthProvider::OpenAi, Some("team")), "ChatGPT Team");
        assert_eq!(plan_display_name(AuthProvider::OpenAi, Some("startup")), "ChatGPT Startup");
        assert_eq!(plan_display_name(AuthProvider::OpenAi, None), "ChatGPT plan not reported");
        let claude = AccountUsage {
            account_id: "anthropic:c".into(),
            plan: Some("max_20x".into()),
            windows: vec![
                window("Claude 7 Day (Fable)", "fable", 0),
                window("Claude 5 Hour", "shared", 1),
                window("Claude 7 Day", "shared", 3),
                UsageWindow {
                    label: "Claude Extra Usage".into(),
                    scope: Some("extra".into()),
                    used_percent: Some(25),
                    resets_at_ms: None,
                    spend: Some(crate::omp_runtime::Spend { used_cents: 10_025, limit_cents: Some(40_000) }),
                },
            ],
            error: None,
        };
        let card = account_card_plan(AuthProvider::Anthropic, Some(&claude), false, true, true);
        assert_eq!(card.plan, "Claude Max 20×");
        let titles = card.groups.iter().map(|group| group.title.as_str()).collect::<Vec<_>>();
        assert_eq!(titles, ["General usage limits", "Fable usage limits", "Extra usage"]);
        assert_eq!(card.groups[0].rows[0].title, "5-hour limit");
        assert_eq!(card.groups[0].rows[1].title, "Weekly limit");
        let spend = &card.groups[2].rows[0];
        assert_eq!(spend.title, "Monthly spending limit");
        assert_eq!(spend.detail.as_deref(), Some("$100.25 of $400.00 spent"));
        assert_eq!(spend.figure.as_deref(), Some("$299.75 left"));
        assert_eq!(spend.left_percent, Some(75));
        assert_eq!(dollars(1_234_567), "$12,345.67");
        let folded = account_card_plan(AuthProvider::Anthropic, Some(&claude), false, true, false);
        let summary = folded.summary.as_ref().expect("a summary");
        assert_eq!(summary.rows[0].title, "Weekly limit");
        assert_eq!(summary.rows[0].left_percent, Some(97));
    }

    #[test]
    fn turn_error_reasons_name_accounts_the_user_knows() {
        let entry = |id: &str, label: Option<&str>, principal: Option<&str>| AccountEntry {
            label: label.map(str::to_owned),
            authenticated_at_ms: 0,
            is_default: false,
            summary: AccountSummary {
                id: id.to_owned(),
                provider: AuthProvider::OpenAi,
                principal: principal.map(str::to_owned),
                state: AccountState::Active,
            },
        };
        let accounts = vec![
            entry("openai-codex:work", Some("Work account"), None),
            entry("anthropic:cma@example.test", None, Some("cma@example.test")),
            entry("openai-codex:opaque", None, Some("0a1b2c3d")),
        ];
        assert_eq!(
            name_accounts_in("no credential is stored for openai-codex account openai-codex:work; sign in again", &accounts),
            "no credential is stored for openai-codex account “Work account”; sign in again"
        );
        assert_eq!(name_accounts_in("anthropic:cma@example.test is rate limited", &accounts), "“cma@example.test” is rate limited");
        assert_eq!(name_accounts_in("openai-codex:opaque failed", &accounts), "openai-codex:opaque failed");
    }

    #[test]
    fn account_labels_must_be_present_and_distinct_ignoring_case() {
        let existing = vec!["Work".to_owned()];
        assert!(labels_are_valid(&["Personal".into()], &existing));
        assert!(!labels_are_valid(&["".into()], &existing));
        assert!(!labels_are_valid(&["   ".into()], &existing));
        assert!(!labels_are_valid(&["work".into()], &existing));
        assert!(!labels_are_valid(&["Home".into(), "home ".into()], &[]));
        assert!(labels_are_valid(&["Home".into(), "Office".into()], &existing));
    }

    #[test]
    fn model_picker_only_includes_supported_models_for_connected_accounts() {
        let mut models = (0..300)
            .map(|index| if index == 250 { "openai-codex/gpt-current".to_owned() } else { format!("provider/model-{index}") })
            .collect::<Vec<_>>();
        models.extend(["anthropic/claude-opus-5".to_owned(), "anthropic/claude-fable-5-1".to_owned()]);
        let authenticated = HashSet::from([AuthProvider::OpenAi, AuthProvider::Anthropic]);
        let options = model_picker_options(&models, "provider/model-42", &authenticated);
        assert_eq!(options, ["openai-codex/gpt-current", "anthropic/claude-opus-5", "anthropic/claude-fable-5-1"]);
    }

    #[test]
    fn model_picker_uses_only_authenticated_runtime_rows() {
        let models = [
            "openai-codex/gpt-5.6-luna",
            "anthropic/claude-fable-5",
            "anthropic/claude-fable-5-1",
            "anthropic/claude-opus-5",
            "cursor/default",
            "aiand/deepseek-ai/deepseek-v4-pro",
        ]
        .map(str::to_owned);
        let authenticated = HashSet::from([AuthProvider::Anthropic]);
        let options = model_picker_options(&models, "anthropic/claude-opus-5", &authenticated);
        assert_eq!(options, ["anthropic/claude-opus-5", "anthropic/claude-fable-5", "anthropic/claude-fable-5-1"]);
    }

    #[test]
    fn active_model_is_hidden_while_its_account_is_disconnected() {
        let models = ["openai-codex/gpt-5.6-luna".to_owned(), "anthropic/claude-sonnet-4-6".to_owned()];
        assert!(model_picker_options(&models, "openai-codex/gpt-5.6-luna", &HashSet::new()).is_empty());
    }

    #[test]
    fn changing_subscription_provider_starts_a_fresh_backend_session() {
        assert!(model_change_requires_new_session("openai-codex/gpt-5.6-luna", "anthropic/claude-opus-4-8"));
        assert!(model_change_requires_new_session("anthropic/claude-opus-4-8", "openrouter/anthropic/claude-opus-4-8"));
        assert!(!model_change_requires_new_session("anthropic/claude-opus-4-8", "anthropic/claude-sonnet-4-6"));
        assert!(!model_change_requires_new_session("openai-codex/gpt-5.6-luna", "openai-codex/gpt-5.6-sol"));
    }

    #[test]
    fn session_source_tabs_partition_native_and_external_sessions() {
        assert!(SessionSourceFilter::Omperor.includes(true));
        assert!(!SessionSourceFilter::Omperor.includes(false));
        assert!(!SessionSourceFilter::Omp.includes(true));
        assert!(SessionSourceFilter::Omp.includes(false));
        assert_eq!(SessionSourceFilter::from_segment(0), SessionSourceFilter::Omperor);
        assert_eq!(SessionSourceFilter::from_segment(1), SessionSourceFilter::Omp);
        assert_eq!(SessionSourceFilter::from_segment(99), SessionSourceFilter::Omperor);
    }

    #[test]
    fn file_changing_tools_refresh_the_changes_but_read_only_tools_do_not() {
        for tool in ["write", "edit", "ast_edit", "bash", "task", "checkpoint", "rewind"] {
            assert!(tool_may_change_files(Some(tool)), "{tool} can change files");
        }
        for tool in ["read", "grep", "glob", "think", "todo", "web_search"] {
            assert!(!tool_may_change_files(Some(tool)), "{tool} is read-only");
        }
        assert!(tool_may_change_files(None));
        assert!(tool_may_change_files(Some("some_future_tool")));
    }

    #[test]
    fn timestamps_read_as_a_day_and_a_time() {
        let text = format_timestamp(1_791_342_669_380);
        assert!(text.contains("2026"), "{text}");
        assert!(text.contains(':'), "{text}");
        assert_eq!(ago(0, 30_000), "now");
        assert_eq!(ago(0, 5 * 60_000), "5m");
        assert_eq!(ago(0, 3 * 3_600_000), "3h");
        assert_eq!(ago(0, 2 * 86_400_000), "2d");
    }
}
