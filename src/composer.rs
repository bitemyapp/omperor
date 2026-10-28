//! What a session's composer holds besides its text: the files and images
//! attached to the next prompt, each prepared on a worker
//! ([`crate::attachments::prepare`]) before the prompt can be sent.

use std::path::PathBuf;
use std::sync::Arc;

use crate::attachments::{self, AttachmentSource, PreparedAttachment};

#[derive(Debug, Default)]
pub(crate) struct PendingAttachments {
    next_id: u64,
    pub(crate) items: Vec<PendingAttachment>,
}

#[derive(Debug)]
pub(crate) struct PendingAttachment {
    pub(crate) id: u64,
    pub(crate) name: String,
    /// `None` while it is being prepared.
    pub(crate) result: Option<Result<PreparedAttachment, String>>,
    source_path: Option<PathBuf>,
    /// Its thumbnail, made once from the preview the worker decoded.
    pub(crate) thumbnail: Option<Arc<aizome_kit::Image>>,
}

impl PendingAttachments {
    pub(crate) fn insert(&mut self, name: String) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(PendingAttachment { id, name, result: None, source_path: None, thumbnail: None });
        id
    }

    /// The worker finished `id`: whether it is still attached.
    pub(crate) fn complete(&mut self, id: u64, result: Result<PreparedAttachment, String>) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        item.result = Some(result);
        if let Some(Ok(prepared)) = &mut item.result {
            if prepared.name != item.name {
                prepared.name.clone_from(&item.name);
            }
            item.source_path = Some(prepared.path.clone());
            item.thumbnail = prepared.preview.as_ref().and_then(|preview| {
                aizome_kit::Image::from_rgba(preview.width, preview.height, &preview.rgba).map(Arc::new)
            });
        }
        true
    }

    pub(crate) fn remove(&mut self, id: u64) {
        self.items.retain(|item| item.id != id);
    }

    /// Whether anything attached is not ready to send: still preparing,
    /// or failed.
    pub(crate) fn blocked(&self) -> bool {
        self.items.iter().any(|item| !matches!(item.result, Some(Ok(_))))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    /// What was prepared, in the order attached.
    pub(crate) fn prepared(&self) -> Vec<PreparedAttachment> {
        self.items.iter().filter_map(|item| item.result.as_ref()?.as_ref().ok().cloned()).collect()
    }

    /// Put back a draft's attachment: its source to prepare again, or the
    /// reason it can't be.
    pub(crate) fn restore(&mut self, saved: &attachments::SavedAttachment) -> (u64, Option<AttachmentSource>) {
        let id = self.insert(saved.name.clone());
        match &saved.path {
            Some(path) => {
                if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                    item.source_path = Some(path.clone());
                }
                (id, Some(AttachmentSource::File(path.clone())))
            }
            None => {
                self.complete(
                    id,
                    Err(saved.error.clone().unwrap_or_else(|| "Attachment source is unavailable; attach it again".into())),
                );
                (id, None)
            }
        }
    }

    /// What a saved draft keeps of them: names and where they came from.
    pub(crate) fn saved(&self) -> Vec<attachments::SavedAttachment> {
        self.items
            .iter()
            .map(|item| attachments::SavedAttachment {
                name: item.name.clone(),
                path: item.source_path.clone(),
                error: match &item.result {
                    Some(Err(error)) => Some(error.clone()),
                    None if item.source_path.is_none() => {
                        Some("Attachment preparation did not finish; attach it again".into())
                    }
                    _ => None,
                },
            })
            .collect()
    }
}
