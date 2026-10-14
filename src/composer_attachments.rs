//! Pending attachment presentation and asynchronous preparation for the composer.
use crate::{
    AppDelegate, BackgroundEvent,
    attachments::{self, AttachmentSource, PreparedAttachment},
    preferences, rect,
};
use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{
    NSAccessibility, NSButton, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSString, NSURL};

#[derive(Debug, Default)]
pub(crate) struct PendingAttachments {
    next_id: u64,
    pub(crate) items: Vec<PendingAttachment>,
    pub(crate) buttons: Vec<Retained<NSButton>>,
    pub(crate) tiles: Vec<(u64, Retained<objc2_app_kit::NSView>)>,
    pub(crate) strip: Option<Retained<objc2_app_kit::NSScrollView>>,
}
#[derive(Debug)]
pub(crate) struct PendingAttachment {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) result: Option<Result<PreparedAttachment, String>>,
    image: Option<Retained<objc2_app_kit::NSImage>>,
    source_path: Option<std::path::PathBuf>,
}
impl PendingAttachments {
    pub(crate) fn insert(&mut self, name: String) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(PendingAttachment {
            id,
            name,
            result: None,
            image: None,
            source_path: None,
        });
        id
    }
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
        }
        true
    }
    pub(crate) fn remove(&mut self, id: u64) {
        self.items.retain(|item| item.id != id);
    }
    pub(crate) fn blocked(&self) -> bool {
        self.items
            .iter()
            .any(|item| !matches!(item.result, Some(Ok(_))))
    }
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    /// Drop controls when a session leaves the visible composer, retaining its data.
    pub(crate) fn detach_presentation(&mut self) {
        for (_, tile) in self.tiles.drain(..) {
            tile.removeFromSuperview();
        }
        self.buttons.clear();
        if let Some(strip) = self.strip.take() {
            strip.removeFromSuperview();
        }
    }

    pub(crate) fn remember_source(&mut self, id: u64, path: std::path::PathBuf) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.source_path = Some(path);
        }
    }

    fn restore_descriptor(
        &mut self,
        saved: &attachments::SavedAttachment,
    ) -> (u64, Option<AttachmentSource>) {
        let id = self.insert(saved.name.clone());
        if let Some(path) = &saved.path {
            self.remember_source(id, path.clone());
            (id, Some(AttachmentSource::File(path.clone())))
        } else {
            self.complete(
                id,
                Err(saved
                    .error
                    .clone()
                    .unwrap_or_else(|| "Attachment source is unavailable; attach it again".into())),
            );
            (id, None)
        }
    }

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

impl crate::AppDelegateIvars {
    fn attachment_composer_hidden(&self, session: &crate::live_sessions::LiveSession) -> bool {
        self.agent_view_states
            .borrow()
            .get(&*session.selected_session_path.borrow())
            .is_some_and(|view| view.desk.active)
    }

    pub(crate) fn can_receive_attachments(&self, session: &crate::live_sessions::LiveSession) -> bool {
        !self.attachment_composer_hidden(session)
            && !session.read_only_session.get()
            && session.resuming_session_id.borrow().is_none()
            && !self.termination_pending.get()
    }
}

impl AppDelegate {
    pub(crate) fn can_paste_attachment(&self) -> bool {
        use objc2::{DefinedClass, MainThreadOnly};
        let app = objc2_app_kit::NSApplication::sharedApplication(self.mtm());
        let Some(window) = self.ivars().window.get() else {
            return false;
        };
        if app.keyWindow().as_deref() != Some(&**window)
            || window.attachedSheet().is_some()
            || !self.ivars().can_receive_attachments(&self.session())
        {
            return false;
        }
        // Do not turn paste in search, credentials, or another editable text
        // view into a composer attachment. An unfocused transcript is allowed.
        if let Some(responder) = window.firstResponder() {
            if let Some(text) = responder.downcast_ref::<objc2_app_kit::NSTextView>() {
                if text.isEditable()
                    && !self
                        .ivars()
                        .attachment_editor
                        .get()
                        .is_some_and(|editor| std::ptr::eq(&**editor, text))
                {
                    return false;
                }
            }
        }
        crate::attachment_input::has_attachments(&NSPasteboard::generalPasteboard())
    }

    pub(crate) fn receive_attachments(&self, pasteboard: &NSPasteboard) -> bool {
        use objc2::DefinedClass;
        let session = self.session();
        if !self.ivars().can_receive_attachments(&session) {
            self.set_chat_status("Choose an editable conversation before attaching files");
            return true;
        }
        let mut sources = Vec::new();
        if let Some(items) = pasteboard.pasteboardItems() {
            for index in 0..items.count() {
                let item = items.objectAtIndex(index);
                if let Some(value) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) {
                    if let Some(url) = NSURL::URLWithString(&value).filter(|url| url.isFileURL()) {
                        if let Some(path) = url.path() {
                            sources.push(AttachmentSource::File(path.to_string().into()));
                        }
                    }
                }
            }
        }
        if sources.is_empty() {
            for (kind, name) in unsafe {
                [
                    (NSPasteboardTypePNG, "Clipboard.png"),
                    (NSPasteboardTypeTIFF, "Clipboard.tiff"),
                ]
            } {
                if let Some(data) = pasteboard.dataForType(kind) {
                    // The pasteboard's owned snapshot must survive subsequent clipboard changes.
                    sources.push(AttachmentSource::Bytes {
                        name: name.into(),
                        bytes: data.to_vec(),
                    });
                    break;
                }
            }
        }
        if sources.is_empty() {
            return false;
        }
        self.add_attachment_sources(sources);
        true
    }

    pub(crate) fn add_attachment_sources(&self, sources: Vec<AttachmentSource>) {
        use objc2::DefinedClass;
        let session = self.session();
        if !self.ivars().can_receive_attachments(&session) {
            self.set_chat_status("Choose an editable conversation before attaching files");
            return;
        }
        for source in sources {
            if session.pending_attachments.borrow().items.len() >= 16 {
                self.set_chat_status(
                    "Up to 16 attachments per message; remove an attachment before adding more",
                );
                break;
            }
            let name = match &source {
                AttachmentSource::File(path) => path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                AttachmentSource::Bytes { name, .. } => name.clone(),
            };
            let id = session.pending_attachments.borrow_mut().insert(name);
            if let AttachmentSource::File(path) = &source {
                session
                    .pending_attachments
                    .borrow_mut()
                    .remember_source(id, path.clone());
            }
            self.prepare_pending_attachment(id, source);
        }
        self.refresh_pending_attachments();
        self.update_chat_controls();
    }

    fn prepare_pending_attachment(&self, id: u64, source: AttachmentSource) {
        let session = self.session();
        let key = session.key;
        let job = self.begin_job("Preparing attachment…");
        if !self.jobs().submit(move || {
            let result = preferences::omp_data_directory()
                .map_err(|error| error.to_string())
                .and_then(|directory| attachments::prepare(source, &directory.join("attachments")));
            BackgroundEvent::AttachmentPrepared {
                key,
                job,
                id,
                result,
            }
        }) {
            self.finish_job(job);
            session
                .pending_attachments
                .borrow_mut()
                .complete(id, Err("Attachment worker unavailable".into()));
        }
    }

    pub(crate) fn restore_pending_attachments(&self, saved: &[attachments::SavedAttachment]) {
        let session = self.session();
        if session.attachments_restored.replace(true) {
            return;
        }
        // Retained live-session data is authoritative, including in-flight workers.
        if !session.pending_attachments.borrow().items.is_empty() {
            return;
        }
        for descriptor in saved {
            let (id, source) = session
                .pending_attachments
                .borrow_mut()
                .restore_descriptor(descriptor);
            if let Some(source) = source {
                self.prepare_pending_attachment(id, source);
            }
        }
        self.refresh_pending_attachments();
        self.update_chat_controls();
    }

    pub(crate) fn clear_pending_attachments(&self) {
        let session = self.session();
        session
            .attachment_generation
            .set(session.attachment_generation.get().wrapping_add(1));
        session.attachment_submission.set(false);
        session.pending_attachments.borrow_mut().clear();
        self.refresh_pending_attachments();
    }

    pub(crate) fn refresh_pending_attachments(&self) {
        use objc2::{DefinedClass, MainThreadOnly};
        use objc2_app_kit::{NSImageScaling, NSImageView, NSScrollView, NSView};
        let Some(surface) = self.ivars().composer_surface.get() else {
            return;
        };
        let session = self.session();
        let mut pending = session.pending_attachments.borrow_mut();
        if pending.strip.is_none() {
            let strip = NSScrollView::initWithFrame(
                NSScrollView::alloc(self.mtm()),
                rect(0.0, 0.0, 1.0, 132.0),
            );
            strip.setDrawsBackground(false);
            strip.setHasHorizontalScroller(true);
            strip.setHasVerticalScroller(false);
            strip.setAutohidesScrollers(true);
            let document =
                NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0.0, 0.0, 1.0, 120.0));
            strip.setDocumentView(Some(&document));
            // Sidestep doesn't forward NSBox subviews to its content view.
            surface
                .contentView()
                .expect("composer content view")
                .addSubview(&strip);
            pending.strip = Some(strip);
        }
        for (_, tile) in pending.tiles.drain(..) {
            tile.removeFromSuperview();
        }
        pending.buttons.clear();
        let document = pending.strip.as_ref().unwrap().documentView().unwrap();
        let mut tiles = Vec::new();
        let mut buttons = Vec::new();
        for item in &mut pending.items {
            let tile =
                NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0.0, 0.0, 160.0, 120.0));
            let backing = objc2_app_kit::NSBox::initWithFrame(
                objc2_app_kit::NSBox::alloc(self.mtm()),
                tile.bounds(),
            );
            backing.setBoxType(objc2_app_kit::NSBoxType::Custom);
            backing.setTitlePosition(objc2_app_kit::NSTitlePosition::NoTitle);
            backing.setCornerRadius(12.0);
            backing.setBorderWidth(0.0);
            backing.setFillColor(&crate::theme::surface_inset());
            tile.addSubview(&backing);
            if item.image.is_none() {
                if let Some(Ok(file)) = &item.result {
                    if let Some(preview) = &file.preview {
                        item.image = Some(preview_image(preview));
                    }
                }
            }
            if let Some(image) = &item.image {
                let view =
                    NSImageView::initWithFrame(NSImageView::alloc(self.mtm()), tile.bounds());
                view.setImage(Some(image));
                view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                view.setWantsLayer(true);
                // Only compositing happens here: pixels were decoded and resized by prepare.
                unsafe {
                    let layer: *mut objc2::runtime::AnyObject = objc2::msg_send![&*view, layer];
                    let _: () = objc2::msg_send![layer, setCornerRadius: 12.0f64];
                    let _: () = objc2::msg_send![layer, setMasksToBounds: true];
                }
                view.setAccessibilityLabel(Some(&NSString::from_str(&item.name)));
                tile.addSubview(&view);
            } else {
                let detail = match &item.result {
                    None => "Preparing…".to_owned(),
                    Some(Err(_)) => "Couldn't attach".to_owned(),
                    Some(Ok(file)) => format!("{} · {} bytes", file.mime_type, file.byte_len),
                };
                let title = crate::text_label(&item.name, 12.0, true, self.mtm());
                title.setFrame(rect(12.0, 45.0, 136.0, 34.0));
                title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
                tile.addSubview(&title);
                let status = crate::text_label(&detail, 12.0, false, self.mtm());
                status.setTextColor(Some(&crate::theme::secondary_text()));
                status.setFrame(rect(12.0, 18.0, 136.0, 22.0));
                status.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
                tile.addSubview(&status);
            }
            let detail = match &item.result {
                Some(Err(error)) => format!("{} — {error}", item.name),
                _ => item.name.clone(),
            };
            tile.setToolTip(Some(&NSString::from_str(&detail)));
            let button = crate::settings_button(
                "×",
                rect(128.0, 88.0, 26.0, 26.0),
                self,
                sel!(removeAttachment:),
                self.mtm(),
            );
            button.setBezelStyle(objc2_app_kit::NSBezelStyle::Circular);
            button.setTag(item.id as isize);
            button.setToolTip(Some(&NSString::from_str(&format!("Remove {}", item.name))));
            button.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                "Remove attachment {}",
                item.name
            ))));
            tile.addSubview(&button);
            document.addSubview(&tile);
            buttons.push(button);
            tiles.push((item.id, tile));
        }
        pending.buttons = buttons;
        pending.tiles = tiles;
        pending.strip.as_ref().unwrap().setHidden(
            pending.items.is_empty() || self.ivars().attachment_composer_hidden(&session),
        );
        drop(pending);
        self.refit_composer_chips();
    }

    pub(crate) fn set_pending_attachments_hidden(&self, hidden: bool) {
        let session = self.session();
        let pending = session.pending_attachments.borrow();
        if let Some(strip) = &pending.strip {
            strip.setHidden(hidden || pending.items.is_empty());
        }
    }

    pub(crate) fn layout_pending_attachments(&self, x: f64, y: f64, width: f64) {
        let session = self.session();
        let pending = session.pending_attachments.borrow();
        let Some(strip) = &pending.strip else {
            return;
        };
        strip.setFrame(rect(x, y, width.max(1.0), 132.0));
        if let Some(document) = strip.documentView() {
            document.setFrame(rect(
                0.0,
                0.0,
                attachment_strip_width(pending.tiles.len()).max(width),
                120.0,
            ));
        }
        for (index, (_, tile)) in pending.tiles.iter().enumerate() {
            tile.setFrame(rect(index as f64 * 172.0, 0.0, 160.0, 120.0));
        }
    }
}

fn attachment_strip_width(count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        count as f64 * 172.0 - 12.0
    }
}

/// Wrap already prepared RGBA pixels; never ask AppKit to decode the source image.
fn preview_image(preview: &attachments::AttachmentPreview) -> Retained<objc2_app_kit::NSImage> {
    use objc2::{AnyThread, msg_send};
    use objc2_foundation::{NSSize, ns_string};
    unsafe {
        let rep: objc2::rc::Allocated<objc2::runtime::AnyObject> =
            msg_send![objc2::class!(NSBitmapImageRep), alloc];
        let rep: Retained<objc2::runtime::AnyObject> = msg_send![rep,
            initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>(),
            pixelsWide: preview.width as isize, pixelsHigh: preview.height as isize,
            bitsPerSample: 8isize, samplesPerPixel: 4isize, hasAlpha: true,
            isPlanar: false, colorSpaceName: ns_string!("NSDeviceRGBColorSpace"),
            bitmapFormat: 2usize, bytesPerRow: (preview.width * 4) as isize, bitsPerPixel: 32isize];
        let pixels: *mut u8 = msg_send![&*rep, bitmapData];
        std::ptr::copy_nonoverlapping(preview.rgba.as_ptr(), pixels, preview.rgba.len());
        let image = objc2_app_kit::NSImage::initWithSize(
            objc2_app_kit::NSImage::alloc(),
            NSSize::new(preview.width as f64 / 2.0, preview.height as f64 / 2.0),
        );
        let _: () = msg_send![&*image, addRepresentation: &*rep];
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thumbnail_strip_keeps_spacing_without_a_trailing_gap() {
        assert_eq!(attachment_strip_width(0), 0.0);
        assert_eq!(attachment_strip_width(1), 160.0);
        assert_eq!(attachment_strip_width(2), 332.0);
        assert_eq!(attachment_strip_width(16), 2740.0);
    }

    #[test]
    fn failed_attachments_remain_removable_and_block_submission() {
        let mut pending = PendingAttachments::default();
        let id = pending.insert("broken.png".into());
        assert!(pending.complete(id, Err("Image decode failed".into())));
        assert!(pending.blocked());
        assert_eq!(pending.items.len(), 1);
        pending.remove(id);
        assert!(!pending.blocked());
    }

    #[test]
    fn attachments_require_an_editable_conversation_surface() {
        let ivars = crate::AppDelegateIvars::default();
        let session = ivars.live_sessions.borrow().selected();
        let path = session.selected_session_path.borrow().clone();
        assert!(ivars.can_receive_attachments(&session));
        ivars
            .agent_view_states
            .borrow_mut()
            .entry(path.clone())
            .or_default()
            .desk
            .active = true;
        assert!(ivars.attachment_composer_hidden(&session));
        assert!(!ivars.can_receive_attachments(&session));
        ivars
            .agent_view_states
            .borrow_mut()
            .get_mut(&path)
            .unwrap()
            .desk
            .active = false;
        assert!(!ivars.attachment_composer_hidden(&session));
        assert!(ivars.can_receive_attachments(&session));
        session.read_only_session.set(true);
        assert!(!ivars.can_receive_attachments(&session));
        session.read_only_session.set(false);
        session.resuming_session_id.replace(Some("resuming".into()));
        assert!(!ivars.can_receive_attachments(&session));
        session.resuming_session_id.replace(None);
        ivars.termination_pending.set(true);
        assert!(!ivars.can_receive_attachments(&session));
        ivars.termination_pending.set(false);
        assert!(ivars.can_receive_attachments(&session));
    }

    #[test]
    fn removed_and_cleared_items_ignore_late_workers() {
        let mut pending = PendingAttachments::default();
        let first = pending.insert("one".into());
        assert!(pending.blocked());
        pending.remove(first);
        assert!(!pending.complete(first, Err("late".into())));
        let second = pending.insert("two".into());
        assert_ne!(first, second);
        pending.clear();
        assert!(!pending.complete(second, Err("late".into())));
        assert!(!pending.blocked());
    }

    #[test]
    fn saved_descriptors_preserve_sources_names_and_errors() {
        let mut pending = PendingAttachments::default();
        let file = pending.insert("User-facing name".into());
        pending.remember_source(file, "/missing/original.png".into());
        let clipboard = pending.insert("Clipboard.png".into());
        let failed = pending.insert("failed.txt".into());
        pending.complete(failed, Err("Permission denied".into()));
        let saved = pending.saved();
        assert_eq!(saved[0].name, "User-facing name");
        assert_eq!(
            saved[0].path.as_deref(),
            Some(std::path::Path::new("/missing/original.png"))
        );
        assert!(saved[0].error.is_none());
        assert!(saved[1].path.is_none());
        assert!(
            saved[1]
                .error
                .as_deref()
                .unwrap()
                .contains("did not finish")
        );
        assert_eq!(saved[2].error.as_deref(), Some("Permission denied"));
        pending.remove(clipboard);
        assert_eq!(pending.saved().len(), 2);
    }

    #[test]
    fn restored_descriptors_retry_paths_and_surface_unavailable_sources() {
        let mut pending = PendingAttachments::default();
        let saved = attachments::SavedAttachment {
            name: "Original display name".into(),
            path: Some("/missing/file.png".into()),
            error: Some("Previous error".into()),
        };
        let (id, source) = pending.restore_descriptor(&saved);
        assert!(
            matches!(source, Some(AttachmentSource::File(path)) if path == std::path::Path::new("/missing/file.png"))
        );
        assert_eq!(pending.items[0].name, saved.name);
        assert!(pending.items[0].result.is_none());
        assert_eq!(pending.saved()[0].path, saved.path);
        assert!(pending.complete(id, Err("Attachment file no longer exists".into())));
        assert!(pending.blocked());

        let (_, source) = pending.restore_descriptor(&attachments::SavedAttachment {
            name: "Lost clipboard".into(),
            path: None,
            error: None,
        });
        assert!(source.is_none());
        assert!(
            pending.saved()[1]
                .error
                .as_deref()
                .unwrap()
                .contains("source is unavailable")
        );
        let (_, source) = pending.restore_descriptor(&attachments::SavedAttachment {
            name: "Failed clipboard".into(),
            path: None,
            error: Some("Decode failed".into()),
        });
        assert!(source.is_none());
        assert_eq!(pending.saved()[2].error.as_deref(), Some("Decode failed"));
    }
}
