//! The sheets that ask for a word or a choice: a project's new name, a
//! fork's branch, an account's label, which folder or which account.

use aizome_kit::prelude::*;

use super::{App, ChoiceAction, Msg, PromptAction, Sheet};
use crate::workspace;

impl App {
    /// The sheet over the window with the keyboard (there is at most one
    /// asking at a time).
    fn open_sheet(&mut self) -> Option<(WindowKey, &mut Sheet)> {
        self.sheets.iter_mut().find(|(_, sheet)| !matches!(sheet, Sheet::SignIn)).map(|(key, sheet)| (*key, sheet))
    }

    pub(super) fn sheet_value(&mut self, value: String) {
        if let Some((_, Sheet::Prompt(prompt))) = self.open_sheet() {
            prompt.value = value;
            prompt.error = None;
        }
    }

    pub(super) fn sheet_choose(&mut self, index: usize) {
        if let Some((_, Sheet::Choice(choice))) = self.open_sheet() {
            choice.selected = index.min(choice.options.len().saturating_sub(1));
        }
    }

    pub(super) fn sheet_cancel(&mut self, cx: &mut Cx<Msg>) {
        if let Some((key, _)) = self.open_sheet() {
            self.sheets.remove(&key);
            cx.close_window(key);
        }
    }

    pub(super) fn sheet_confirm(&mut self, cx: &mut Cx<Msg>) {
        let Some((key, sheet)) = self.open_sheet() else { return };
        let sheet = sheet.clone();
        match sheet {
            Sheet::Prompt(prompt) => {
                let value = prompt.value.trim().to_owned();
                // A value that won't do keeps the sheet up, saying why.
                let error = match &prompt.action {
                    _ if value.is_empty() => Some("Type a name".to_owned()),
                    PromptAction::ForkBranch { .. } if !workspace::branch_name_is_valid(&value) => {
                        Some("That isn't a branch name git accepts".to_owned())
                    }
                    PromptAction::LabelAccount(id) => self.label_error(id, &value),
                    _ => None,
                };
                if let Some(error) = error {
                    if let Some(Sheet::Prompt(prompt)) = self.sheets.get_mut(&key) {
                        prompt.error = Some(error);
                    }
                    return;
                }
                self.sheets.remove(&key);
                cx.close_window(key);
                match prompt.action {
                    PromptAction::RenameProject(id) => self.rename_project(id, value, cx),
                    PromptAction::LabelAccount(id) => self.label_account(id, value, cx),
                    PromptAction::ForkBranch { summary, directory, project_id, point } => {
                        self.add_worktree(&summary, directory, project_id, point, value, cx)
                    }
                }
            }
            Sheet::Choice(choice) => {
                self.sheets.remove(&key);
                cx.close_window(key);
                match choice.action {
                    ChoiceAction::Folder { project_id, folders } => {
                        if let Some(folder) = folders.get(choice.selected).cloned() {
                            self.create_session(&project_id, folder, None, cx);
                        }
                    }
                    ChoiceAction::Account { provider, model, accounts } => {
                        let Some(account) = accounts.get(choice.selected).cloned() else { return };
                        let session = self.session();
                        self.remember_account(&session, provider, &account, cx);
                        match model {
                            Some(model) => self.start_with_model(model, Some(account), cx),
                            None => self.request_account(account, cx),
                        }
                    }
                }
            }
            Sheet::SignIn => {}
        }
    }
}
