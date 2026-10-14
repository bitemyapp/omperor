//! Versioned session reading metadata. Transcript bodies stay in OMP journals.
use crate::agents::ViewState;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

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
        Self {
            version: 2,
            sessions: Vec::new(),
        }
    }
}

impl SavedViews {
    pub(crate) fn capture(states: &HashMap<Option<PathBuf>, ViewState>) -> Self {
        let mut sessions: Vec<_> = states
            .iter()
            .map(|(path, view)| {
                let draft_key = path.as_deref().and_then(draft_key);
                SavedView {
                    path: if draft_key.is_some() {
                        None
                    } else {
                        path.clone()
                    },
                    draft_key,
                    view: view.clone(),
                }
            })
            .collect();
        sessions.sort_by(|a, b| (&a.path, a.draft_key).cmp(&(&b.path, b.draft_key)));
        Self {
            version: 2,
            sessions,
        }
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
            let desk = &mut item.view.desk;
            let mut ids = HashSet::new();
            desk.pins
                .retain(|pin| !pin.id.is_empty() && ids.insert(pin.id.clone()));
            // The fixed native pane pool reserves one slot for the preview.
            let mut kept = 0;
            let mut preview = false;
            desk.pins.retain(|pin| {
                if pin.kept {
                    kept += 1;
                    kept <= crate::workspace_model::MAX_PINS
                } else if preview {
                    false
                } else {
                    preview = true;
                    true
                }
            });
            if !desk
                .pins
                .iter()
                .any(|pin| Some(&pin.id) == desk.selected_pin.as_ref())
            {
                desk.selected_pin = desk.pins.first().map(|pin| pin.id.clone());
            }
            if !desk
                .pins
                .iter()
                .any(|pin| Some(&pin.id) == desk.focused_pin.as_ref())
            {
                desk.focused_pin = None;
            }
            desk.panel = desk.panel.clamp(0, 1);
            // Negative positions can occur after overscroll, but are not useful anchors.
            desk.conversation_y = desk.conversation_y.max(0.0);
            desk.change_scroll_y = desk.change_scroll_y.max(0.0);
            desk.diff_scroll_y = desk.diff_scroll_y.max(0.0);
            for pin in &mut desk.pins {
                pin.scroll_y = pin.scroll_y.max(0.0);
            }
        }
        saved.version = 2;
        Ok(saved)
    }

    pub(crate) fn into_states(self) -> impl Iterator<Item = (Option<PathBuf>, ViewState)> {
        self.sessions
            .into_iter()
            .map(|saved| (saved.draft_key.map(draft_path).or(saved.path), saved.view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restores_drafts_tabs_and_positions_without_copying_transcript_bodies() {
        let mut view = ViewState::default();
        view.desk.draft = "Unsent: 日本語 🦀".into();
        view.query = "review".into();
        view.desk.inspect("agent-a");
        view.desk.keep(0);
        view.desk.pins[0].expanded_tools.insert(3);
        view.desk.pins[0].scroll_y = 42.5;
        view.desk.pins[0].snapshot.body = "DO NOT COPY TRANSCRIPT".into();
        view.desk.inspect("agent-b");
        let path = PathBuf::from("/sessions/one.oms");
        let saved = SavedViews::capture(&HashMap::from([(Some(path.clone()), view)]));
        let json = serde_json::to_string(&saved).unwrap();
        assert!(!json.contains("DO NOT COPY TRANSCRIPT"));
        let states: HashMap<_, _> = SavedViews::decode(&json).unwrap().into_states().collect();
        assert!(!states.contains_key(&None));
        let restored = &states[&Some(path)];
        assert_eq!(restored.desk.draft, "Unsent: 日本語 🦀");
        assert_eq!(restored.query, "review");
        assert!(restored.desk.pins[0].kept);
        assert_eq!(restored.desk.pins[0].scroll_y, 42.5);
        assert!(restored.desk.pins[0].expanded_tools.contains(&3));
        assert!(!restored.desk.pins[1].kept);
    }
    #[test]
    fn rejects_unknown_versions_and_duplicate_session_keys() {
        assert!(SavedViews::decode(r#"{"version":3,"sessions":[]}"#).is_err());
        assert!(
            SavedViews::decode(
                r#"{"version":1,"sessions":[{"path":null,"view":{}},{"path":null,"view":{}}]}"#
            )
            .is_err()
        );
        assert!(SavedViews::decode("broken").is_err());
        assert!(
            SavedViews::decode(r#"{"version":1,"sessions":[{"path":null,"view":{}}]}"#).is_ok()
        );
    }

    #[test]
    fn distinct_drafts_and_attachment_descriptors_survive_round_trip() {
        let mut first = ViewState::default();
        first.desk.draft = "first unsent prompt".into();
        first
            .desk
            .attachments
            .push(crate::attachments::SavedAttachment {
                name: "image.png".into(),
                path: Some("/attachments/image.png".into()),
                error: None,
            });
        let mut second = ViewState::default();
        second.desk.draft = "second unsent prompt".into();
        let states = HashMap::from([
            (Some(draft_path(12)), first),
            (Some(draft_path(13)), second),
        ]);
        let json = serde_json::to_string(&SavedViews::capture(&states)).unwrap();
        assert!(!json.contains("@omperor-draft"));
        let restored: HashMap<_, _> = SavedViews::decode(&json).unwrap().into_states().collect();
        assert_eq!(
            restored[&Some(draft_path(12))].desk.draft,
            "first unsent prompt"
        );
        assert_eq!(
            restored[&Some(draft_path(13))].desk.draft,
            "second unsent prompt"
        );
        assert_eq!(restored[&Some(draft_path(12))].desk.attachments.len(), 1);
    }

    #[test]
    fn rejects_ambiguous_or_repeated_draft_identities() {
        assert!(
            SavedViews::decode(
                r#"{"version":2,"sessions":[{"path":"/one.oms","draft_key":12,"view":{}}]}"#
            )
            .is_err()
        );
        assert!(SavedViews::decode(r#"{"version":2,"sessions":[{"draft_key":12,"view":{}},{"draft_key":12,"view":{}}]}"#).is_err());
        assert_eq!(
            draft_key(std::path::Path::new("/real/@omperor-draft/12")),
            None
        );
        assert_eq!(
            draft_key(std::path::Path::new("@omperor-draft/12/extra")),
            None
        );
    }

    #[test]
    fn rejects_reserved_path_alias_and_exhausted_draft_key() {
        assert!(
            SavedViews::decode(
                r#"{"version":2,"sessions":[{"path":"@omperor-draft/12","view":{}}]}"#
            )
            .is_err()
        );
        assert!(
            SavedViews::decode(
                r#"{"version":2,"sessions":[{"draft_key":18446744073709551615,"view":{}}]}"#
            )
            .is_err()
        );
    }
}
