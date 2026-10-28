//! The Agents panel's evidence: each sub-agent's recorded transcript, read
//! from its journal on a worker while it runs (every 400 ms) and now and
//! then after, and the one selected laid out as transcript rows.

use std::sync::Arc;

use aizome_kit::prelude::*;
use aizome_kit::Selection;

use super::{App, Msg};
use crate::chat::Conversation;
use crate::live_sessions::{LiveSession, SessionKey};
use crate::{agents, markdown, transcripts};

/// How much of an agent's last answer "Discuss" quotes into the draft.
const DISCUSS_EXCERPT: usize = 1600;

impl App {
    /// Read the sub-agents' journals that changed, if a read is due (or
    /// `force`d) and none is under way.
    pub(crate) fn request_transcripts(&mut self, session: &LiveSession, force: bool, cx: &mut Cx<Msg>) {
        if session.subagent_sync_inflight.get() {
            return;
        }
        let subagents = session.subagents.borrow();
        if subagents.is_empty() {
            session.subagent_transcripts.borrow_mut().clear();
            return;
        }
        let busy = subagents.iter().any(|info| agents::AgentState::from_status(&info.status).is_busy());
        if !force && session.subagent_sync_at.get().is_some_and(|last| last.elapsed() < transcripts::interval(busy, false)) {
            return;
        }
        let selected = self.views.get(&session.key).and_then(|v| v.view.selected.clone());
        let requests = {
            let mut cache = session.subagent_transcripts.borrow_mut();
            transcripts::prune(&mut cache, &subagents);
            let mut requests = transcripts::plan(&subagents, &cache);
            if let Some(id) = selected {
                match requests.iter_mut().find(|r| r.id == id) {
                    Some(request) => request.inspected = true,
                    None => {
                        if let Some(info) = subagents.iter().find(|info| info.id == id)
                            && let Some(path) = &info.session_file
                        {
                            requests.push(transcripts::Request {
                                id: info.id.clone(),
                                path: path.clone(),
                                known: cache.get(&info.id).map(|e| e.stamp).unwrap_or_default(),
                                inspected: true,
                            });
                        }
                    }
                }
            }
            requests
        };
        drop(subagents);
        if requests.is_empty() {
            return;
        }
        session.subagent_sync_at.set(Some(std::time::Instant::now()));
        session.subagent_sync_inflight.set(true);
        let key = session.key;
        cx.spawn(move || Msg::TranscriptsSynced(key, Arc::new(transcripts::sync(requests))));
    }

    /// Every frame: the sessions whose sub-agents are due a read.
    pub(crate) fn sync_subagents(&mut self, cx: &mut Cx<Msg>) {
        for session in self.live.entries() {
            if !session.subagents.borrow().is_empty() {
                self.request_transcripts(&session, false, cx);
            }
        }
    }

    pub(super) fn transcripts_synced(&mut self, key: SessionKey, result: &transcripts::SyncResult, cx: &mut Cx<Msg>) {
        let Some(session) = self.live.get(key) else { return };
        session.subagent_sync_inflight.set(false);
        let changed = transcripts::apply(&mut session.subagent_transcripts.borrow_mut(), result.updates.clone());
        for (id, error) in &result.failures {
            session.workspace_read_failures.borrow_mut().insert(id.clone(), error.clone());
        }
        let selected = self.views.get(&key).and_then(|v| v.view.selected.clone());
        if let Some(id) = selected
            && changed.contains(&id)
        {
            self.build_agent_rows(&session, &id, cx);
        }
    }

    pub(super) fn select_agent(&mut self, id: Option<String>, cx: &mut Cx<Msg>) {
        let session = self.session();
        let key = session.key;
        self.view_mut(key).view.selected = id.clone();
        if let Some(id) = id {
            if session.subagent_transcripts.borrow().contains_key(&id) {
                self.build_agent_rows(&session, &id, cx);
            }
            self.request_transcripts(&session, true, cx);
        }
        self.schedule_save_views(cx);
    }

    /// The agent's recorded messages as transcript rows, their bodies sent
    /// to be parsed.
    fn build_agent_rows(&mut self, session: &LiveSession, id: &str, cx: &mut Cx<Msg>) {
        let messages = match session.subagent_transcripts.borrow().get(id) {
            Some(entry) => entry.messages.clone(),
            None => return,
        };
        let mut conversation = Conversation::default();
        conversation.load_history(&messages);
        let rows: Vec<_> = (0..conversation.len()).filter_map(|i| conversation.message(i)).map(super::live::agent_row).collect();
        let key = session.key;
        let view = self.view_mut(key);
        let previous = view.agent_rows.remove(id).unwrap_or_default();
        let mut rows = rows;
        // Keep what was parsed of rows that didn't change.
        for (row, old) in rows.iter_mut().zip(previous) {
            if old.hash == row.hash {
                row.doc = old.doc;
            }
        }
        let mut wanted = Vec::new();
        for (index, row) in rows.iter_mut().enumerate() {
            let parsed = matches!(
                row.message.kind,
                crate::chat::TranscriptMessageKind::User | crate::chat::TranscriptMessageKind::Assistant
            );
            if parsed && row.doc.is_none() {
                row.parsing = true;
                wanted.push((index, row.hash, row.message.body.clone()));
            }
        }
        view.agent_rows.insert(id.to_owned(), rows);
        for (index, hash, body) in wanted {
            let agent = Some(id.to_owned());
            cx.spawn(move || Msg::Parsed(key, agent, index, hash, Arc::new(markdown::parse(&body))));
        }
    }

    /// Bring an agent's last answer into the draft, quoted with its name,
    /// for the lead agent to discuss. Nothing is sent.
    pub(super) fn discuss_agent(&mut self, id: &str) {
        let session = self.session();
        let Some(info) = session.subagents.borrow().iter().find(|a| a.id == id).cloned() else { return };
        let answer = session
            .subagent_transcripts
            .borrow()
            .get(id)
            .and_then(|entry| {
                entry
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.speaker == crate::session::Speaker::Assistant)
                    .map(|m| m.text.clone())
            })
            .unwrap_or_else(|| info.error.clone().or(info.warning.clone()).unwrap_or_default());
        let mut excerpt: String = answer.chars().take(DISCUSS_EXCERPT).collect();
        if answer.chars().count() > DISCUSS_EXCERPT {
            excerpt.push('…');
        }
        let quoted: String = excerpt.lines().map(|l| format!("> {l}\n")).collect();
        let label = if info.label.is_empty() { info.id.clone() } else { info.label.clone() };
        let block = format!("From {} “{}” ({}):\n{}\n", info.agent, label, info.id, quoted);
        let key = session.key;
        let view = self.view_mut(key);
        let mut text = view.editor.text();
        if !text.is_empty() && !text.ends_with("\n\n") {
            text.push_str(if text.ends_with('\n') { "\n" } else { "\n\n" });
        }
        text.push_str(&block);
        view.editor.set_text(&text);
        let end = view.editor.len_chars();
        view.editor.set_selection(Selection::caret(end));
    }
}
