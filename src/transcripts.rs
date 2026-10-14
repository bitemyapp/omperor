//! Keeping the current session's sub-agent transcripts warm.
//!
//! A sub-agent's transcript should appear the instant it is asked for, so the
//! journals behind the Agents panel are folded on a worker before anyone
//! clicks, and refolded while a sub-agent is still writing so an open inspector
//! follows it live. Only the session in view is tracked, only journals that
//! actually changed on disk are re-read, and the whole cache is bounded, so
//! the resident cost stays proportional to one session's sub-agents rather
//! than to everything the user has ever run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::omp_runtime::SubagentInfo;
use crate::session::SessionMessage;

/// The most journals kept warm at once. A session with more sub-agents than
/// this keeps the most recently spawned, which is what the panel shows first.
pub(crate) const MAX_TRACKED: usize = 32;

/// Journals larger than this are left to the read that a click starts, so a
/// runaway child cannot pull megabytes into memory every few seconds.
pub(crate) const MAX_PRELOAD_BYTES: u64 = 8 * 1024 * 1024;

/// How often journals are re-examined while a sub-agent is still working or a
/// transcript inspector is visible: fast enough that an open inspector reads as live.
pub(crate) const ACTIVE_INTERVAL: Duration = Duration::from_millis(400);

/// How often they are re-examined once every sub-agent has settled and no
/// inspector is visible. Nothing is expected to change; this only catches a late
/// write.
pub(crate) const IDLE_INTERVAL: Duration = Duration::from_secs(5);

/// What a journal looked like when it was last folded, which is enough to
/// tell whether re-reading it could produce anything new.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) len: u64,
    pub(crate) modified_ms: u64,
}

impl Stamp {
    /// The stamp of the file at `path`, or `None` when it cannot be stat-ed.
    ///
    /// This touches the filesystem and belongs on a worker thread.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        Some(Self {
            len: metadata.len(),
            modified_ms,
        })
    }
}

/// One sub-agent's folded journal, with the stamp it was folded at.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) messages: Vec<SessionMessage>,
    pub(crate) stamp: Stamp,
    pub(crate) read_started: std::time::Instant,
}

/// Read metadata before folding so a concurrent append is picked up on the
/// next pass rather than incorrectly marked as already displayed.
pub(crate) fn read(path: &Path) -> Result<Entry, String> {
    let read_started = std::time::Instant::now();
    let stamp = Stamp::of(path).ok_or_else(|| "The journal is not accessible.".to_owned())?;
    let messages = crate::journal::read(path)?.messages;
    Ok(Entry {
        messages,
        stamp,
        read_started,
    })
}

/// One journal a sync pass should look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    /// The stamp already folded, so an unchanged journal is not re-read.
    pub(crate) known: Stamp,
    /// Explicitly inspected journals must keep updating beyond the preload limit.
    pub(crate) inspected: bool,
}

/// One journal a sync pass actually re-read.
#[derive(Debug, Clone)]
pub(crate) struct Update {
    pub(crate) id: String,
    pub(crate) entry: Entry,
}

#[derive(Debug, Default)]
pub(crate) struct SyncResult {
    pub(crate) updates: Vec<Update>,
    pub(crate) failures: Vec<(String, String)>,
}

/// The journals worth examining for the sub-agents in view.
///
/// Sub-agents without a journal yet are skipped, and the newest `MAX_TRACKED`
/// win when a session has spawned more than the cache holds.
pub(crate) fn plan(subagents: &[SubagentInfo], cache: &HashMap<String, Entry>) -> Vec<Request> {
    let mut tracked: Vec<&SubagentInfo> = subagents
        .iter()
        .filter(|info| info.session_file.is_some())
        .collect();
    if tracked.len() > MAX_TRACKED {
        tracked.sort_by_key(|info| info.index);
        tracked.drain(..tracked.len() - MAX_TRACKED);
    }
    tracked
        .into_iter()
        .filter_map(|info| {
            Some(Request {
                id: info.id.clone(),
                path: info.session_file.clone()?,
                known: cache
                    .get(&info.id)
                    .map(|entry| entry.stamp)
                    .unwrap_or_default(),
                inspected: false,
            })
        })
        .collect()
}

/// Accept completions in read-start order, even when a slower earlier fold
/// finishes last or the file was replaced with a shorter/older journal.
pub(crate) fn apply(cache: &mut HashMap<String, Entry>, updates: Vec<Update>) -> Vec<String> {
    let mut changed = Vec::new();
    for update in updates {
        if cache
            .get(&update.id)
            .is_some_and(|known| known.read_started > update.entry.read_started)
        {
            continue;
        }
        changed.push(update.id.clone());
        cache.insert(update.id, update.entry);
    }
    changed
}

/// How long to wait before the next sync pass.
pub(crate) const fn interval(any_busy: bool, sheet_open: bool) -> Duration {
    if any_busy || sheet_open {
        ACTIVE_INTERVAL
    } else {
        IDLE_INTERVAL
    }
}

/// Forgets every cached journal whose sub-agent is no longer in the session,
/// which is what bounds the cache when the user switches sessions.
pub(crate) fn prune(cache: &mut HashMap<String, Entry>, subagents: &[SubagentInfo]) {
    if subagents.is_empty() {
        cache.clear();
        return;
    }
    cache.retain(|id, _| subagents.iter().any(|info| &info.id == id));
}

/// Folds every journal in `requests` that changed since its known stamp.
///
/// This reads files and belongs on a worker thread.
pub(crate) fn sync(requests: Vec<Request>) -> SyncResult {
    let mut result = SyncResult::default();
    for request in requests {
        let stamp = Stamp::of(&request.path);
        if stamp.is_some_and(|stamp| {
            stamp == request.known || (!request.inspected && stamp.len > MAX_PRELOAD_BYTES)
        }) {
            continue;
        }
        match read(&request.path) {
            Ok(entry) => result.updates.push(Update {
                id: request.id,
                entry,
            }),
            Err(error) if request.inspected => result.failures.push((request.id, error)),
            Err(_) => {} // Preloading stays silent until the user inspects the journal.
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{ACTIVE_INTERVAL, Entry, IDLE_INTERVAL, MAX_TRACKED, Stamp, interval, plan, prune};
    use crate::omp_runtime::SubagentInfo;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn info(id: &str, index: u64, file: Option<&str>) -> SubagentInfo {
        SubagentInfo {
            id: id.to_owned(),
            index,
            agent: "task".to_owned(),
            label: id.to_owned(),
            status: "running".to_owned(),
            started_ms: None,
            duration_ms: None,
            error: None,
            warning: None,
            session_file: file.map(PathBuf::from),
        }
    }

    fn cached(stamp: Stamp) -> Entry {
        Entry {
            messages: Vec::new(),
            stamp,
            read_started: std::time::Instant::now(),
        }
    }

    #[test]
    fn late_fold_does_not_replace_newer_content_even_after_journal_replacement() {
        let now = std::time::Instant::now();
        let mut old = cached(Stamp {
            len: 500,
            modified_ms: 900,
        });
        old.read_started = now - std::time::Duration::from_secs(1);
        let mut new = cached(Stamp {
            len: 50,
            modified_ms: 100,
        });
        new.read_started = now;
        new.messages.push(crate::session::SessionMessage::spoken(
            crate::session::Speaker::Assistant,
            "new journal",
        ));
        let mut cache = HashMap::new();
        super::apply(
            &mut cache,
            vec![super::Update {
                id: "agent".into(),
                entry: new,
            }],
        );
        assert!(
            super::apply(
                &mut cache,
                vec![super::Update {
                    id: "agent".into(),
                    entry: old
                }]
            )
            .is_empty()
        );
        assert_eq!(cache["agent"].messages[0].text, "new journal");
    }

    #[test]
    fn a_subagent_without_a_journal_is_not_planned() {
        let subagents = vec![
            info("alpha", 0, None),
            info("beta", 1, Some("/tmp/beta.oms")),
        ];
        let planned = plan(&subagents, &HashMap::new());
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].id, "beta");
        assert_eq!(planned[0].known, Stamp::default());
    }

    #[test]
    fn a_planned_journal_carries_the_stamp_already_folded() {
        let subagents = vec![info("alpha", 0, Some("/tmp/alpha.oms"))];
        let stamp = Stamp {
            len: 42,
            modified_ms: 7,
        };
        let cache = HashMap::from([("alpha".to_owned(), cached(stamp))]);
        assert_eq!(plan(&subagents, &cache)[0].known, stamp);
    }

    #[test]
    fn planning_keeps_the_newest_journals_when_a_session_overflows_the_cache() {
        let subagents = (0..MAX_TRACKED as u64 + 4)
            .map(|index| info(&format!("child-{index}"), index, Some("/tmp/child.oms")))
            .collect::<Vec<_>>();
        let planned = plan(&subagents, &HashMap::new());
        assert_eq!(planned.len(), MAX_TRACKED);
        assert_eq!(planned[0].id, "child-4");
    }

    #[test]
    fn pruning_forgets_sub_agents_that_left_with_their_session() {
        let mut cache = HashMap::from([
            ("alpha".to_owned(), cached(Stamp::default())),
            ("beta".to_owned(), cached(Stamp::default())),
        ]);
        prune(&mut cache, &[info("beta", 0, Some("/tmp/beta.oms"))]);
        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key("beta"));
        prune(&mut cache, &[]);
        assert!(cache.is_empty());
    }

    #[test]
    fn the_sync_cadence_follows_what_the_user_is_looking_at() {
        assert_eq!(interval(false, false), IDLE_INTERVAL);
        assert_eq!(interval(true, false), ACTIVE_INTERVAL);
        assert_eq!(interval(false, true), ACTIVE_INTERVAL);
    }
}

#[cfg(test)]
mod live_journal_tests {
    use super::*;
    use omp_journal::{EntryDraft, Journal, Kind, KindName};
    use serde_json::json;

    fn append(
        journal: &mut Journal,
        text: String,
        by: Option<omp_journal::EntryId>,
    ) -> omp_journal::EntryId {
        journal
            .append(EntryDraft {
                kind: Kind::known(KindName::MsgUser),
                by,
                prior: None,
                label: None,
                data: json!({"text":text}).to_string().into(),
            })
            .unwrap()
            .id
    }

    #[test]
    fn inspected_large_journal_refreshes_while_its_writer_is_open_and_reports_loss() {
        let dir = std::env::temp_dir().join(format!("omperor-live-sync-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.oms");
        let mut journal = Journal::create(&path).unwrap();
        let genesis = journal
            .append(EntryDraft {
                kind: Kind::known(KindName::Journal),
                by: None,
                prior: None,
                label: None,
                data: json!({"version":1,"cwd":"/tmp","created":"2026-09-17T00:00:00Z"})
                    .to_string()
                    .into(),
            })
            .unwrap();
        let mut parent = genesis.id;
        for _ in 0..9 {
            parent = append(&mut journal, "x".repeat(1024 * 1024 - 1024), Some(parent));
        }
        let request = Request {
            id: "agent".into(),
            path: path.clone(),
            known: Stamp::default(),
            inspected: false,
        };
        assert!(sync(vec![request.clone()]).updates.is_empty());
        let mut request = Request {
            inspected: true,
            ..request
        };
        let result = sync(vec![request.clone()]);
        assert!(result.failures.is_empty());
        assert_eq!(result.updates.len(), 1);
        assert_eq!(result.updates[0].entry.messages.len(), 9);
        request.known = result.updates[0].entry.stamp;
        assert!(sync(vec![request.clone()]).updates.is_empty());
        append(&mut journal, "new committed data 🦀".into(), Some(parent));
        let result = sync(vec![request.clone()]);
        assert_eq!(result.updates[0].entry.messages.len(), 10);
        assert!(result.updates[0].entry.stamp.len > request.known.len);
        drop(journal);
        std::fs::remove_file(&path).unwrap();
        let result = sync(vec![request]);
        assert!(result.updates.is_empty());
        assert_eq!(result.failures[0].0, "agent");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_preload_stays_silent_but_explicit_inspection_reports_failure() {
        let dir = std::env::temp_dir().join(format!("omperor-broken-sync-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.oms");
        let request = Request {
            id: "broken".into(),
            path,
            known: Stamp::default(),
            inspected: false,
        };
        let result = sync(vec![request.clone()]);
        assert!(result.updates.is_empty() && result.failures.is_empty());
        assert_eq!(
            sync(vec![Request {
                inspected: true,
                ..request
            }])
            .failures
            .len(),
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
