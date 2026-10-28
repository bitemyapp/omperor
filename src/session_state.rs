//! Versioned session reading metadata: what each session's window held
//! (an unsent draft and its attachments, the panel beside the
//! conversation, the change in view, the agents' filter). Transcript
//! bodies stay in OMP's journals. Fields of earlier versions that are gone
//! (the earlier interface's pins and scroll positions) are ignored when
//! read, so drafts written by older builds still come back.
use crate::agents::ViewState;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

/// Launch preferences needed to restore a draft without starting inference.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DraftContext {
    pub(crate) project: PathBuf,
    pub(crate) project_id: Option<String>,
    pub(crate) model: String,
    pub(crate) account: Option<String>,
    pub(crate) thinking_level: String,
    pub(crate) fast_mode: bool,
}

/// One session's working arrangement.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Desk {
    pub(crate) draft: String,
    pub(crate) attachments: Vec<crate::attachments::SavedAttachment>,
    pub(crate) draft_context: Option<DraftContext>,
    pub(crate) selected_change: Option<PathBuf>,
    /// Which panel shows beside the conversation: 0 Changes, 1 Agents.
    pub(crate) panel: isize,
    /// Whether that panel shows at all.
    pub(crate) workspace_inspector: bool,
}

/// Draft keys occupy a relative namespace that is never passed to OMP or read as a journal.
pub(crate) fn draft_path(key: u64) -> PathBuf {
    PathBuf::from("@omperor-draft").join(key.to_string())
}

pub(crate) fn draft_key(path: &std::path::Path) -> Option<u64> {
    let mut parts = path.components();
    if parts.next()?.as_os_str() != "@omperor-draft" {
        return None;
    }
    let key = parts.next()?.as_os_str().to_str()?.parse().ok()?;
    parts.next().is_none().then_some(key)
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SavedViews {
    version: u32,
    sessions: Vec<SavedView>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedView {
    path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draft_key: Option<u64>,
    view: ViewState,
}

impl Default for SavedViews {
    fn default() -> Self {
        Self { version: 2, sessions: Vec::new() }
    }
}

impl SavedViews {
    pub(crate) fn capture(states: &HashMap<Option<PathBuf>, ViewState>) -> Self {
        let mut sessions: Vec<_> = states
            .iter()
            .map(|(path, view)| {
                let draft_key = path.as_deref().and_then(draft_key);
                SavedView { path: if draft_key.is_some() { None } else { path.clone() }, draft_key, view: view.clone() }
            })
            .collect();
        sessions.sort_by(|a, b| (&a.path, a.draft_key).cmp(&(&b.path, b.draft_key)));
        Self { version: 2, sessions }
    }

    pub(crate) fn decode(json: &str) -> Result<Self, String> {
        let mut saved: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if !matches!(saved.version, 1 | 2) {
            return Err(format!("unsupported version {}", saved.version));
        }
        let mut paths = HashSet::new();
        for item in &mut saved.sessions {
            if saved.version == 1 && item.draft_key.is_some() {
                return Err("version 1 cannot contain draft identities".into());
            }
            if item.path.is_some() && item.draft_key.is_some() {
                return Err("session cannot be both a draft and a journal".into());
            }
            if item.path.as_deref().and_then(draft_key).is_some() {
                return Err("draft identity must use draft_key, not a journal path".into());
            }
            if item.draft_key == Some(u64::MAX) {
                return Err("draft identity exhausts the session allocator".into());
            }
            if !paths.insert((item.path.clone(), item.draft_key)) {
                return Err("duplicate session identity".into());
            }
            item.view.desk.panel = item.view.desk.panel.clamp(0, 1);
        }
        saved.version = 2;
        Ok(saved)
    }

    pub(crate) fn into_states(self) -> impl Iterator<Item = (Option<PathBuf>, ViewState)> {
        self.sessions.into_iter().map(|saved| (saved.draft_key.map(draft_path).or(saved.path), saved.view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_drafts_and_filters_without_copying_transcript_bodies() {
        let mut view = ViewState::default();
        view.desk.draft = "Unsent: 日本語 🦀".into();
        view.desk.panel = 1;
        view.query = "review".into();
        let path = PathBuf::from("/sessions/one.oms");
        let saved = SavedViews::capture(&HashMap::from([(Some(path.clone()), view)]));
        let json = serde_json::to_string(&saved).unwrap();
        let states: HashMap<_, _> = SavedViews::decode(&json).unwrap().into_states().collect();
        assert!(!states.contains_key(&None));
        let restored = &states[&Some(path)];
        assert_eq!(restored.desk.draft, "Unsent: 日本語 🦀");
        assert_eq!(restored.desk.panel, 1);
        assert_eq!(restored.query, "review");
    }

    #[test]
    fn an_older_builds_desk_still_gives_back_its_draft() {
        let json = r#"{"version":2,"sessions":[{"path":"/s/a.oms","view":{"desk":{"draft":"kept",
            "pins":[{"id":"x","kept":true}],"conversation_y":120.5,"panel":7},"query":"q","page":3}}]}"#;
        let states: HashMap<_, _> = SavedViews::decode(json).unwrap().into_states().collect();
        let view = &states[&Some(PathBuf::from("/s/a.oms"))];
        assert_eq!(view.desk.draft, "kept");
        assert_eq!(view.desk.panel, 1, "clamped to a panel that exists");
        assert_eq!(view.query, "q");
    }

    #[test]
    fn rejects_unknown_versions_and_duplicate_session_keys() {
        assert!(SavedViews::decode(r#"{"version":3,"sessions":[]}"#).is_err());
        assert!(SavedViews::decode(r#"{"version":1,"sessions":[{"path":null,"view":{}},{"path":null,"view":{}}]}"#).is_err());
        assert!(SavedViews::decode("broken").is_err());
        assert!(SavedViews::decode(r#"{"version":1,"sessions":[{"path":null,"view":{}}]}"#).is_ok());
    }

    #[test]
    fn distinct_drafts_and_attachment_descriptors_survive_round_trip() {
        let mut first = ViewState::default();
        first.desk.draft = "first unsent prompt".into();
        first.desk.attachments.push(crate::attachments::SavedAttachment {
            name: "image.png".into(),
            path: Some("/attachments/image.png".into()),
            error: None,
        });
        let mut second = ViewState::default();
        second.desk.draft = "second unsent prompt".into();
        let states = HashMap::from([(Some(draft_path(12)), first), (Some(draft_path(13)), second)]);
        let json = serde_json::to_string(&SavedViews::capture(&states)).unwrap();
        assert!(!json.contains("@omperor-draft"));
        let restored: HashMap<_, _> = SavedViews::decode(&json).unwrap().into_states().collect();
        assert_eq!(restored[&Some(draft_path(12))].desk.draft, "first unsent prompt");
        assert_eq!(restored[&Some(draft_path(13))].desk.draft, "second unsent prompt");
        assert_eq!(restored[&Some(draft_path(12))].desk.attachments.len(), 1);
    }

    #[test]
    fn rejects_ambiguous_or_repeated_draft_identities() {
        assert!(SavedViews::decode(r#"{"version":2,"sessions":[{"path":"/one.oms","draft_key":12,"view":{}}]}"#).is_err());
        assert!(
            SavedViews::decode(r#"{"version":2,"sessions":[{"draft_key":12,"view":{}},{"draft_key":12,"view":{}}]}"#)
                .is_err()
        );
        assert_eq!(draft_key(std::path::Path::new("/real/@omperor-draft/12")), None);
        assert_eq!(draft_key(std::path::Path::new("@omperor-draft/12/extra")), None);
    }

    #[test]
    fn rejects_reserved_path_alias_and_exhausted_draft_key() {
        assert!(SavedViews::decode(r#"{"version":2,"sessions":[{"path":"@omperor-draft/12","view":{}}]}"#).is_err());
        assert!(
            SavedViews::decode(r#"{"version":2,"sessions":[{"draft_key":18446744073709551615,"view":{}}]}"#).is_err()
        );
    }
}
