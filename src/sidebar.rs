//! Pure presentation rules for abbreviated project lists and partial-list drags.
use crate::SidebarRow;
use std::collections::HashSet;

pub(crate) const PROJECT_SESSION_LIMIT: usize = 5;

/// Keep the first five sessions in their existing order, plus selected/running
/// sessions. Search results bypass folding at the caller; no session is removed
/// from the underlying registry or persisted order.
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
        while matches!(rows.peek(), Some(SidebarRow::Session { project_id: Some(id), .. }) if id == &project_id)
        {
            members.push(rows.next().unwrap());
        }
        let is_expanded = expanded.contains(&project_id);
        let hidden = members.iter().enumerate().filter(|(position, row)| {
            *position >= PROJECT_SESSION_LIMIT && matches!(row, SidebarRow::Session { index, .. } if !keep_visible.contains(index))
        }).count();
        result.extend(members.into_iter().enumerate().filter_map(|(position, row)| {
            let pinned = matches!(&row, SidebarRow::Session { index, .. } if keep_visible.contains(index));
            (is_expanded || position < PROJECT_SESSION_LIMIT || pinned).then_some(row)
        }));
        if hidden > 0 {
            result.push(SidebarRow::ShowMore {
                project_id,
                hidden,
                expanded: is_expanded,
            });
        }
    }
    result
}

/// Move only the dragged session relative to its visible neighbors. Hidden
/// siblings retain their relative order, and all siblings are persisted.
pub(crate) fn reordered_project(
    mut all: Vec<String>,
    visible: &[String],
    dragged: &str,
) -> Vec<String> {
    let Some(position) = visible.iter().position(|id| id == dragged) else {
        return all;
    };
    if !all.iter().any(|id| id == dragged) {
        return all;
    }
    all.retain(|id| id != dragged);
    let insertion = visible
        .get(position + 1)
        .and_then(|next| all.iter().position(|id| id == next))
        .or_else(|| {
            position
                .checked_sub(1)
                .and_then(|i| all.iter().position(|id| id == &visible[i]).map(|i| i + 1))
        })
        .unwrap_or(all.len());
    all.insert(insertion, dragged.into());
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project(id: &str, start: usize, count: usize) -> Vec<SidebarRow> {
        std::iter::once(SidebarRow::Project {
            project_id: id.into(),
        })
        .chain((start..start + count).map(|index| SidebarRow::Session {
            index,
            project_id: Some(id.into()),
        }))
        .collect()
    }
    fn indices(rows: &[SidebarRow]) -> Vec<usize> {
        rows.iter()
            .filter_map(|r| {
                if let SidebarRow::Session { index, .. } = r {
                    Some(*index)
                } else {
                    None
                }
            })
            .collect()
    }
    #[test]
    fn folds_each_project_independently_and_keeps_all_running_and_selected_rows() {
        let rows = [project("a", 0, 10), project("b", 10, 7)].concat();
        let folded = fold_rows(rows.clone(), &HashSet::new(), &HashSet::from([7, 9, 16]));
        assert_eq!(
            indices(&folded),
            [0, 1, 2, 3, 4, 7, 9, 10, 11, 12, 13, 14, 16]
        );
        assert!(folded.contains(&SidebarRow::ShowMore {
            project_id: "a".into(),
            hidden: 3,
            expanded: false
        }));
        let expanded = fold_rows(rows, &HashSet::from(["a".into()]), &HashSet::new());
        assert_eq!(indices(&expanded), (0..15).collect::<Vec<_>>());
        assert!(expanded.contains(&SidebarRow::ShowMore {
            project_id: "a".into(),
            hidden: 5,
            expanded: true
        }));
    }
    #[test]
    fn five_sessions_and_collapsed_project_need_no_more_button() {
        let rows = [project("a", 0, 5), project("closed", 5, 0)].concat();
        assert_eq!(
            fold_rows(rows.clone(), &HashSet::new(), &HashSet::new()),
            rows
        );
    }
    #[test]
    fn dragging_visible_sessions_preserves_hidden_siblings() {
        let all = (0..10).map(|n| n.to_string()).collect::<Vec<_>>();
        let visible = ["9", "0", "1", "2", "3", "4"].map(String::from);
        assert_eq!(
            reordered_project(all.clone(), &visible, "9"),
            ["9", "0", "1", "2", "3", "4", "5", "6", "7", "8"]
        );
        let visible = ["1", "2", "3", "4", "0"].map(String::from);
        assert_eq!(
            reordered_project(all, &visible, "0"),
            ["1", "2", "3", "4", "0", "5", "6", "7", "8", "9"]
        );
    }
}

/// Exercise the real native controls without loading preferences, starting a
/// helper, or spending inference tokens. Leave the fixture open for visual QA.
impl crate::AppDelegate {
    pub(super) fn run_sidebar_activity_smoke(&self) {
        use crate::*;
        let project = Project {
            id: "activity-check".into(),
            name: "Activity check".into(),
            directories: vec!["/tmp/omperor-activity-check".into()],
            last_opened_at_ms: 0,
            collapsed: false,
        };
        self.ivars().projects.replace(vec![project.clone()]);
        let mut entries = Vec::new();
        for index in 0..8 {
            let entry = self.ivars().live_sessions.borrow_mut().create();
            entry.backend.set(mock_inference::Backend::Mock);
            entry
                .current_project
                .replace(project.directories[0].clone());
            entry.active_project_id.replace(Some(project.id.clone()));
            entry.current_model.replace(mock_inference::MODEL.into());
            entry
                .mock_session_title
                .replace(Some(format!("Activity check {}", index + 1)));
            let path =
                std::path::PathBuf::from(format!("/tmp/omperor-activity-check/mock-{index}.oms"));
            self.ivars()
                .live_sessions
                .borrow_mut()
                .bind_journal(entry.key, path)
                .unwrap();
            if [1, 6, 7].contains(&index) {
                entry.reduce(UiEvent::Running, std::time::Instant::now());
            }
            entries.push(entry);
        }
        self.ivars()
            .live_sessions
            .borrow_mut()
            .select(entries[7].key);
        self.render_selected_binding();
        assert_eq!(self.ivars().sidebar_spinners.borrow().len(), 3);
        let spinner = self.ivars().busy_spinner.get().unwrap();
        assert!(!spinner.isHidden());
        assert!(
            spinner.frame().origin.y
                < self
                    .ivars()
                    .composer_surface
                    .get()
                    .unwrap()
                    .frame()
                    .origin
                    .y
        );
        // A different selected session cannot inherit those running indicators.
        self.ivars()
            .live_sessions
            .borrow_mut()
            .select(entries[0].key);
        self.render_selected_binding();
        assert!(spinner.isHidden());
        assert_eq!(self.ivars().sidebar_spinners.borrow().len(), 3);
        self.ivars()
            .live_sessions
            .borrow_mut()
            .select(entries[7].key);
        self.render_selected_binding();
        assert!(!spinner.isHidden());
        self.ivars()
            .window
            .get()
            .unwrap()
            .setTitle(ns_string!("Activity check · No inference"));
        eprintln!(
            "sidebar-activity-smoke PASS simultaneous_sidebar_spinners=3 footer_below_composer=true idle_selection_hides_spinner=true"
        );
    }
}
