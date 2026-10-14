//! Native attachment ingress; no file access or image decoding occurs here.
//!
//! Requires objc2-app-kit's `NSDragging` feature in addition to `NSApplication`,
//! `NSPasteboard`, `NSResponder`, `NSText`, `NSTextView`, and `NSView`; foundation
//! needs `NSArray`, `NSString`, `NSObject`, and `NSGeometry`.
//!
//! Return `field_editor` from windowWillReturnFieldEditor:toObject: only for the
//! composer. Use `drop_view` as the chat pane's container, not a hit-test overlay.
//! The application delegate must implement `receive_attachments(&NSPasteboard)
//! -> bool`, extracting file URLs first (all file kinds), otherwise PNG/TIFF.
//! It must snapshot needed pasteboard data before returning, then schedule file
//! I/O and image encoding off the main thread. `true` means ingress was handled.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSMenu, NSMenuItem,
    NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeTIFF, NSTextView,
    NSView,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSObjectProtocol, NSRect, NSString};

fn attachment_types() -> Retained<NSArray<NSString>> {
    // SAFETY: AppKit's pasteboard type constants live for the process lifetime.
    unsafe {
        NSArray::from_slice(&[
            NSPasteboardTypeFileURL,
            NSPasteboardTypePNG,
            NSPasteboardTypeTIFF,
        ])
    }
}

pub(crate) fn has_attachments(pasteboard: &NSPasteboard) -> bool {
    pasteboard
        .availableTypeFromArray(&attachment_types())
        .is_some()
}

fn receive(pasteboard: &NSPasteboard, mtm: MainThreadMarker) -> bool {
    let application = NSApplication::sharedApplication(mtm);
    let Some(delegate) = application.delegate() else {
        return false;
    };
    delegate
        .downcast::<crate::AppDelegate>()
        .is_ok_and(|delegate| delegate.receive_attachments(pasteboard))
}

fn attachment_operation(sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
    if sender
        .draggingSourceOperationMask()
        .contains(NSDragOperation::Copy)
        && has_attachments(&sender.draggingPasteboard())
    {
        NSDragOperation::Copy
    } else {
        NSDragOperation::None
    }
}

define_class!(
    // SAFETY: initialized through NSTextView's designated frame initializer;
    // main-thread-only, with no additional ivars or Drop implementation.
    #[unsafe(super = NSTextView)]
    #[thread_kind = MainThreadOnly]
    #[derive(Debug)]
    struct AttachmentFieldEditor;

    unsafe impl NSObjectProtocol for AttachmentFieldEditor {}

    impl AttachmentFieldEditor {
        // Plain-text NSTextView validation otherwise disables Paste for PNG/TIFF,
        // before AppKit ever dispatches our paste: override.
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> objc2::runtime::Bool {
            if item.action() == Some(sel!(paste:))
                && has_attachments(&NSPasteboard::generalPasteboard())
            {
                return objc2::runtime::Bool::YES;
            }
            unsafe { msg_send![super(self), validateMenuItem: item] }
        }
        #[unsafe(method(paste:))]
        fn paste(&self, sender: Option<&AnyObject>) {
            let pasteboard = NSPasteboard::generalPasteboard();
            if has_attachments(&pasteboard) && receive(&pasteboard, self.mtm()) {
                return;
            }
            // SAFETY: preserve NSTextView's standard text-paste behavior.
            unsafe { let _: () = msg_send![super(self), paste: sender]; }
        }
    }

    // SAFETY: selectors and argument/return types match NSDraggingDestination.
    unsafe impl NSDraggingDestination for AttachmentFieldEditor {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if has_attachments(&sender.draggingPasteboard()) {
                attachment_operation(sender)
            } else {
                unsafe { msg_send![super(self), draggingEntered: sender] }
            }
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if has_attachments(&sender.draggingPasteboard()) {
                attachment_operation(sender)
            } else {
                unsafe { msg_send![super(self), draggingUpdated: sender] }
            }
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            if has_attachments(&sender.draggingPasteboard()) {
                attachment_operation(sender) == NSDragOperation::Copy
            } else {
                unsafe { msg_send![super(self), prepareForDragOperation: sender] }
            }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let pasteboard = sender.draggingPasteboard();
            if has_attachments(&pasteboard) {
                attachment_operation(sender) == NSDragOperation::Copy
                    && receive(&pasteboard, self.mtm())
            } else {
                unsafe { msg_send![super(self), performDragOperation: sender] }
            }
        }

        #[unsafe(method(concludeDragOperation:))]
        fn conclude_drag(&self, sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            if sender.is_some_and(|sender| has_attachments(&sender.draggingPasteboard())) {
                return;
            }
            unsafe { let _: () = msg_send![super(self), concludeDragOperation: sender]; }
        }
    }
);

define_class!(
    // SAFETY: initialized through NSView's designated frame initializer;
    // main-thread-only, with no additional ivars or Drop implementation.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[derive(Debug)]
    struct AttachmentDropView;

    unsafe impl NSObjectProtocol for AttachmentDropView {}

    // SAFETY: selectors and argument/return types match NSDraggingDestination.
    unsafe impl NSDraggingDestination for AttachmentDropView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            attachment_operation(sender)
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            attachment_operation(sender)
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            attachment_operation(sender) == NSDragOperation::Copy
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            attachment_operation(sender) == NSDragOperation::Copy
                && receive(&sender.draggingPasteboard(), self.mtm())
        }
    }
);

/// A dedicated shared field editor for the composer, not other text fields.
pub(crate) fn field_editor(mtm: MainThreadMarker) -> Retained<NSTextView> {
    // SAFETY: NSTextView's designated initializer; the allocation is on main.
    let editor: Retained<AttachmentFieldEditor> =
        unsafe { msg_send![AttachmentFieldEditor::alloc(mtm), initWithFrame: NSRect::ZERO] };
    editor.setFieldEditor(true);
    editor.setRichText(false);
    editor.setImportsGraphics(false);
    // Prompts are often code, commands, and flags: macOS's typing
    // substitutions would turn `--flag` into an em dash and "x" into curly
    // quotes in what the agent receives.
    editor.setAutomaticQuoteSubstitutionEnabled(false);
    editor.setAutomaticDashSubstitutionEnabled(false);
    editor.setAutomaticTextReplacementEnabled(false);
    editor.setAutomaticSpellingCorrectionEnabled(false);
    // Registration adds to NSTextView's existing text drag types.
    editor.registerForDraggedTypes(&attachment_types());
    Retained::into_super(editor)
}

/// A chat pane container that accepts file URLs and pasted image representations.
pub(crate) fn drop_view(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    // SAFETY: NSView's designated initializer; the allocation is on main.
    let view: Retained<AttachmentDropView> =
        unsafe { msg_send![AttachmentDropView::alloc(mtm), initWithFrame: frame] };
    view.registerForDraggedTypes(&attachment_types());
    Retained::into_super(view)
}

define_class!(
    // SAFETY: NSMenu has no additional subclassing invariants.
    #[unsafe(super = NSMenu)]
    #[thread_kind = MainThreadOnly]
    #[derive(Debug)]
    struct AttachmentEditMenu;
    unsafe impl NSObjectProtocol for AttachmentEditMenu {}
    impl AttachmentEditMenu {

        #[unsafe(method(update))]
        fn update(&self) {
            let app = NSApplication::sharedApplication(self.mtm());
            if let Some(delegate) = app.delegate().and_then(|value| value.downcast::<crate::AppDelegate>().ok()) {
                if let Some(item) = self.itemWithTitle(&NSString::from_str("Paste")) {
                    // A nil target preserves AppKit's editing transaction. An
                    // explicit non-responder target can end field editing first.
                    let attachment = delegate.can_paste_attachment();
                    unsafe {
                        item.setAction(Some(if attachment { sel!(pasteAttachment:) } else { sel!(paste:) }));
                        item.setTarget(if attachment { Some(&*delegate as &AnyObject) } else { None });
                    }
                }
            }
            unsafe { let _: () = msg_send![super(self), update]; }
        }
    }
);

pub(crate) fn edit_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu: Retained<AttachmentEditMenu> = unsafe {
        msg_send![AttachmentEditMenu::alloc(mtm), initWithTitle: &*NSString::from_str("Edit")]
    };
    Retained::into_super(menu)
}

define_class!(
    // SAFETY: NSMenu has no additional subclassing invariants.
    #[unsafe(super = NSMenu)]
    #[thread_kind = MainThreadOnly]
    #[derive(Debug)]
    struct AttachmentMainMenu;
    unsafe impl NSObjectProtocol for AttachmentMainMenu {}
    impl AttachmentMainMenu {
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &objc2_app_kit::NSEvent) -> bool {
            // AppKit searches submenus internally; it need not invoke their
            // performKeyEquivalent: overrides. Refresh Paste at the entry point.
            let items = self.itemArray();
            for index in 0..items.count() {
                let item = items.objectAtIndex(index);
                if let Some(menu) = item.submenu() {
                    if menu.downcast_ref::<AttachmentEditMenu>().is_some() {
                        menu.update();
                    }
                }
            }
            unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }
    }
);

pub(crate) fn main_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu: Retained<AttachmentMainMenu> = unsafe {
        msg_send![AttachmentMainMenu::alloc(mtm), initWithTitle: &*NSString::from_str("")]
    };
    Retained::into_super(menu)
}

/// Runs the real window, field editor, menu validation, and Command-V routing.
/// Opt-in only: this replaces the general pasteboard with fixture data.
pub(crate) fn run_paste_smoke(
    app: &NSApplication,
    delegate: &crate::AppDelegate,
    _mtm: MainThreadMarker,
) {
    use objc2::DefinedClass;
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSPasteboardTypeString};
    use objc2_foundation::{NSData, NSPoint, NSRange, ns_string};
    let session = delegate.session();
    // Keep the diagnostic composer editable through its normal text-change callback.
    session
        .current_model
        .replace(crate::mock_inference::MODEL.into());
    session.runtime_ready.set(true);
    let window = delegate.ivars().window.get().unwrap();
    let input = delegate.ivars().input.get().unwrap();
    let menu = app.mainMenu().unwrap();
    let edit = menu.itemAtIndex(2).unwrap().submenu().unwrap();
    let paste = edit.itemWithTitle(ns_string!("Paste")).unwrap();
    let paste_index = edit.indexOfItem(&paste);
    let board = NSPasteboard::generalPasteboard();
    let fixture = std::thread::spawn(|| {
        let path = if let Some(path) = std::env::var_os("OMPEROR_ATTACHMENT_SMOKE_IMAGE") {
            std::path::PathBuf::from(path)
        } else {
            let path = std::env::temp_dir().join(format!(
                "omperor-preview-fixture-{}.png",
                std::process::id()
            ));
            let image = image::RgbImage::from_fn(640, 480, |x, y| {
                image::Rgb([(40 + x / 4) as u8, (60 + y / 4) as u8, 145])
            });
            image.save(&path).expect("write smoke image fixture");
            path
        };
        let path = std::fs::canonicalize(path).expect("resolve smoke fixture");
        let bytes = std::fs::read(&path).expect("read smoke fixture");
        (path, bytes)
    });
    while !fixture.is_finished() {
        smoke_run_loop_tick();
    }
    let (fixture_path, png) = fixture.join().expect("fixture worker panicked");
    window.makeKeyAndOrderFront(None);
    assert!(window.isKeyWindow());
    for (editable, focused) in [(true, true), (true, false), (false, false)] {
        for keyboard in [false, true] {
            input.setEditable(editable);
            input.setStringValue(ns_string!("Keep this prompt"));
            if focused {
                assert!(window.makeFirstResponder(Some(input)));
                unsafe {
                    input.selectText(None);
                }
                let responder = window.firstResponder().unwrap();
                assert!(
                    responder.downcast_ref::<AttachmentFieldEditor>().is_some(),
                    "composer must use its attachment field editor, got {responder:?}"
                );
            } else {
                assert!(window.makeFirstResponder(Some(&**window)));
            }
            // Command-V must discover a newly copied image even if the last
            // menu validation saw an empty clipboard and a different target.
            if keyboard {
                board.clearContents();
                edit.update();
            }
            board.clearContents();
            board.setData_forType(Some(&NSData::with_bytes(&png)), unsafe {
                NSPasteboardTypePNG
            });
            if !keyboard {
                edit.update();
                assert!(paste.isEnabled(), "image Paste disabled focused={focused}");
            }
            let before = session.pending_attachments.borrow().items.len();
            if keyboard {
                let event: Retained<NSEvent> = unsafe {
                    msg_send![objc2::class!(NSEvent),
                        keyEventWithType: NSEventType::KeyDown, location: NSPoint::ZERO,
                        modifierFlags: NSEventModifierFlags::Command, timestamp: 0.0f64,
                        windowNumber: window.windowNumber(), context: None::<&AnyObject>,
                        characters: ns_string!("v"), charactersIgnoringModifiers: ns_string!("v"),
                        isARepeat: false, keyCode: 9u16
                    ]
                };
                assert!(
                    menu.performKeyEquivalent(&event),
                    "main menu must handle Command-V"
                );
            } else {
                edit.performActionForItemAtIndex(paste_index);
            }
            assert_eq!(session.pending_attachments.borrow().items.len(), before + 1);
            assert_eq!(input.stringValue().to_string(), "Keep this prompt");
            eprintln!(
                "PASS native PNG paste editable={editable} focused={focused} keyboard={keyboard}"
            );
        }
    }
    input.setEditable(true);
    assert!(window.makeFirstResponder(Some(input)));
    unsafe {
        input.selectText(None);
    }
    let editor = window.firstResponder().unwrap();
    let editor = editor.downcast_ref::<NSTextView>().unwrap();
    board.clearContents();
    board.setString_forType(ns_string!("Text replacement"), unsafe {
        NSPasteboardTypeString
    });
    edit.update();
    assert!(paste.isEnabled());
    assert!(unsafe { app.sendAction_to_from(sel!(paste:), None, Some(&*paste)) });
    assert_eq!(editor.string().to_string(), "Text replacement");
    editor.setSelectedRange(NSRange::new(0, 0));
    eprintln!("PASS native text paste replaces selected prompt");
    board.clearContents();
    board.setData_forType(Some(&NSData::with_bytes(&png)), unsafe {
        NSPasteboardTypePNG
    });
    let before = session.pending_attachments.borrow().items.len();
    let validates: bool = unsafe { msg_send![editor, validateMenuItem: &*paste] };
    assert!(validates, "field editor must accept image-only Paste");
    assert!(unsafe { app.sendAction_to_from(sel!(paste:), None, Some(&*paste)) });
    assert_eq!(session.pending_attachments.borrow().items.len(), before + 1);
    eprintln!("PASS field editor native image Paste validation and routing");
    assert!(window.makeFirstResponder(Some(&**window)));
    session.read_only_session.set(true);
    edit.update();
    assert!(
        !paste.isEnabled(),
        "read-only conversation must reject image Paste"
    );
    session.read_only_session.set(false);
    eprintln!("PASS read-only conversation rejects image Paste");
    board.clearContents();
    edit.update();
    assert!(!paste.isEnabled(), "empty clipboard must disable Paste");
    eprintln!("PASS empty clipboard disables Paste");
    smoke_wait_for_previews(delegate);
    eprintln!("PASS real background PNG preparation and RGBA previews");

    // Exercise Finder-style file URL ingress through the actual Paste menu.
    let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(
        &fixture_path.to_string_lossy(),
    ));
    board.clearContents();
    assert!(
        board.setString_forType(&url.absoluteString().unwrap(), unsafe {
            NSPasteboardTypeFileURL
        })
    );
    let before = session.pending_attachments.borrow().items.len();
    edit.update();
    assert!(paste.isEnabled(), "file URL Paste must be enabled");
    edit.performActionForItemAtIndex(paste_index);
    assert_eq!(session.pending_attachments.borrow().items.len(), before + 1);
    smoke_wait_for_previews(delegate);
    {
        let pending = session.pending_attachments.borrow();
        let file = pending
            .items
            .last()
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap();
        assert_eq!(file.path, fixture_path);
    }
    eprintln!("PASS native file URL Paste and worker filesystem preparation");
    smoke_verify_attachment_layout(delegate);

    // Retain the actual remove control, releasing the borrow before its action.
    let (removed, button) = {
        let pending = session.pending_attachments.borrow();
        (pending.items[1].id, pending.buttons[1].clone())
    };
    unsafe {
        button.performClick(None);
    }
    {
        let pending = session.pending_attachments.borrow();
        assert_eq!(pending.items.len(), before);
        assert!(pending.items.iter().all(|item| item.id != removed));
        assert!(pending.tiles.iter().all(|(id, _)| *id != removed));
    }
    smoke_verify_attachment_layout(delegate);
    eprintln!("PASS actual thumbnail remove button preserves remaining images");
    smoke_capture_window(window, "/tmp/omperor-attachment-preview.png");
    delegate.clear_pending_attachments();
    {
        let pending = session.pending_attachments.borrow();
        assert!(pending.items.is_empty());
        assert!(pending.tiles.is_empty());
        assert!(pending.buttons.is_empty());
        assert!(pending.strip.as_ref().is_none_or(|strip| strip.isHidden()));
    }
    eprintln!("PASS attachment paste responder smoke");
}

/// Exercise typed input and submission without loading sessions or starting
/// an inference child. The normal action still prepares the prompt on a worker.
pub(crate) fn run_composer_smoke(app: &NSApplication, delegate: &crate::AppDelegate) {
    use objc2::DefinedClass;
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSTextInputClient};
    use objc2_foundation::{NSNotFound, NSPoint, NSRange, ns_string};

    let session = delegate.session();
    session.backend.set(crate::mock_inference::Backend::Mock);
    session
        .current_model
        .replace(crate::mock_inference::MODEL.into());
    session.runtime_ready.set(true);
    let window = delegate.ivars().window.get().unwrap();
    let input = delegate.ivars().input.get().unwrap();
    delegate.update_chat_controls();
    window.makeKeyAndOrderFront(None);
    assert!(window.makeFirstResponder(Some(input)));
    unsafe { input.selectText(None) };
    let editor = input
        .currentEditor()
        .unwrap()
        .downcast::<NSTextView>()
        .unwrap();
    assert!(editor.downcast_ref::<AttachmentFieldEditor>().is_some());
    let prompt = "Composer submission smoke";
    unsafe {
        NSTextInputClient::insertText_replacementRange(
            &*editor,
            &NSString::from_str(prompt),
            NSRange::new(NSNotFound as usize, 0),
        );
    }
    assert_eq!(
        input.stringValue().to_string(),
        prompt,
        "composer reads live typed text"
    );
    smoke_capture_window(window, "/tmp/omperor-composer-input.png");

    let route =
        std::env::var("OMPEROR_COMPOSER_SMOKE_SUBMIT").unwrap_or_else(|_| "keyboard".into());
    match route.as_str() {
        "keyboard" => {
            let event: Retained<NSEvent> = unsafe {
                msg_send![objc2::class!(NSEvent),
                    keyEventWithType: NSEventType::KeyDown, location: NSPoint::ZERO,
                    modifierFlags: NSEventModifierFlags::empty(), timestamp: 0.0f64,
                    windowNumber: window.windowNumber(), context: None::<&AnyObject>,
                    characters: ns_string!("\r"), charactersIgnoringModifiers: ns_string!("\r"),
                    isARepeat: false, keyCode: 36u16
                ]
            };
            app.sendEvent(&event);
        }
        "editor" => unsafe {
            NSTextInputClient::doCommandBySelector(&*editor, sel!(insertNewline:));
        },
        "button" => unsafe {
            delegate
                .ivars()
                .send_button
                .get()
                .unwrap()
                .performClick(None);
        },
        _ => panic!("unknown composer smoke submission route: {route}"),
    }
    {
        let conversation = session.conversation.borrow();
        let message = conversation
            .message(conversation.len() - 1)
            .expect("submitted message");
        assert_eq!(
            message.body, prompt,
            "submitted prompt appears in transcript"
        );
    }
    assert!(
        input.stringValue().is_empty(),
        "submission clears the composer"
    );
    assert!(session.chat_busy.get(), "submission starts work");
    assert!(
        session.attachment_submission.get(),
        "prompt preparation is queued on the worker"
    );
    // The next focus starts from the cleared value, not the old editor text.
    // Normally the worker's completion enables the input again.
    input.setEditable(true);
    assert!(window.makeFirstResponder(Some(input)));
    unsafe { input.selectText(None) };
    assert!(input.currentEditor().unwrap().string().is_empty());
    eprintln!("PASS composer typed input, {route} submission, and cleared editor");
}

/// Yield to native display/timers rather than sleeping on the AppKit thread.
fn smoke_run_loop_tick() {
    use objc2_foundation::{NSDate, NSRunLoop, ns_string};
    unsafe {
        let _: bool = msg_send![&*NSRunLoop::currentRunLoop(),
            runMode: ns_string!("kCFRunLoopDefaultMode"),
            beforeDate: &*NSDate::dateWithTimeIntervalSinceNow(0.01)];
    }
}

fn smoke_wait_for_previews(delegate: &crate::AppDelegate) {
    let session = delegate.session();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        delegate.poll_background_jobs();
        let ready = {
            let pending = session.pending_attachments.borrow();
            assert!(!pending.items.is_empty());
            pending.items.iter().all(|item| match &item.result {
                None => false,
                Some(Err(error)) => panic!("attachment {} failed: {error}", item.name),
                Some(Ok(file)) => {
                    let preview = file
                        .preview
                        .as_ref()
                        .expect("image needs a ready thumbnail");
                    assert!(preview.width > 0 && preview.height > 0);
                    assert_eq!(
                        preview.rgba.len(),
                        preview.width as usize * preview.height as usize * 4
                    );
                    assert!(preview.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
                    true
                }
            })
        };
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for attachment workers"
        );
        smoke_run_loop_tick();
    }
}

fn smoke_verify_attachment_layout(delegate: &crate::AppDelegate) {
    use objc2::DefinedClass;
    let session = delegate.session();
    let pending = session.pending_attachments.borrow();
    assert!(pending.items.len() > 1, "multi-image strip required");
    assert_eq!(pending.tiles.len(), pending.items.len());
    assert_eq!(pending.buttons.len(), pending.items.len());
    let strip = pending.strip.as_ref().expect("native thumbnail strip");
    let surface = delegate.ivars().composer_surface.get().unwrap();
    let parent = unsafe { strip.superview() }.expect("strip parent");
    assert_eq!(surface.contentView().as_deref(), Some(&*parent));
    assert!(!strip.isHidden());
    assert_eq!(strip.frame().size.height, 132.0);
    let input = delegate.ivars().input.get().unwrap();
    let input_rect = input.convertRect_toView(input.bounds(), Some(&parent));
    assert!(
        strip.frame().origin.y >= input_rect.origin.y + input_rect.size.height,
        "thumbnail strip must sit above input: {:?} vs {:?}",
        strip.frame(),
        input_rect
    );
    let mut right = 0.0;
    for (item, (id, tile)) in pending.items.iter().zip(&pending.tiles) {
        assert_eq!(item.id, *id);
        let frame = tile.frame();
        assert_eq!(frame.size.width, 160.0);
        assert_eq!(frame.size.height, 120.0);
        assert!(
            frame.origin.x >= right,
            "tiles must not overlap horizontally"
        );
        right = frame.origin.x + frame.size.width;
    }
    assert!(strip.documentView().unwrap().frame().size.width >= right);
    eprintln!(
        "PASS {} ready thumbnails in horizontal 160x120 tiles above composer",
        pending.items.len()
    );
}

pub(crate) fn smoke_capture_window(window: &objc2_app_kit::NSWindow, path: &str) {
    let path = path.to_owned();
    use objc2_foundation::{NSData, NSDictionary};
    let content = window.contentView().unwrap();
    // Include native titlebar and window chrome, not merely the composer view.
    let root = unsafe { content.superview() }.unwrap_or(content);
    unsafe {
        let _: () = msg_send![&*root, layoutSubtreeIfNeeded];
        let _: () = msg_send![&*root, displayIfNeeded];
        let bitmap: Retained<AnyObject> =
            msg_send![&*root, bitmapImageRepForCachingDisplayInRect: root.bounds()];
        let _: () =
            msg_send![&*root, cacheDisplayInRect: root.bounds(), toBitmapImageRep: &*bitmap];
        // AppKit capture stays on main; only an owned pixel snapshot crosses threads.
        let planar: bool = msg_send![&*bitmap, isPlanar];
        assert!(!planar, "window capture must be interleaved");
        let width: isize = msg_send![&*bitmap, pixelsWide];
        let height: isize = msg_send![&*bitmap, pixelsHigh];
        let bits: isize = msg_send![&*bitmap, bitsPerSample];
        let samples: isize = msg_send![&*bitmap, samplesPerPixel];
        let alpha: bool = msg_send![&*bitmap, hasAlpha];
        let format: usize = msg_send![&*bitmap, bitmapFormat];
        let stride: isize = msg_send![&*bitmap, bytesPerRow];
        let pixel_bits: isize = msg_send![&*bitmap, bitsPerPixel];
        let color_space: Retained<NSString> = msg_send![&*bitmap, colorSpaceName];
        let color_space = color_space.to_string();
        let len = usize::try_from(stride)
            .unwrap()
            .checked_mul(usize::try_from(height).unwrap())
            .unwrap();
        let pixels: *const u8 = msg_send![&*bitmap, bitmapData];
        assert!(!pixels.is_null());
        let pixels = std::slice::from_raw_parts(pixels, len).to_vec();
        let output = path.clone();
        let writer = std::thread::spawn(move || {
            objc2::rc::autoreleasepool(|_| {
                let rep: objc2::rc::Allocated<AnyObject> =
                    msg_send![objc2::class!(NSBitmapImageRep), alloc];
                let rep: Retained<AnyObject> = msg_send![rep,
                    initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>(),
                    pixelsWide: width, pixelsHigh: height,
                    bitsPerSample: bits, samplesPerPixel: samples, hasAlpha: alpha,
                    isPlanar: false, colorSpaceName: &*NSString::from_str(&color_space),
                    bitmapFormat: format, bytesPerRow: stride, bitsPerPixel: pixel_bits];
                let destination: *mut u8 = msg_send![&*rep, bitmapData];
                assert!(!destination.is_null());
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), destination, pixels.len());
                let properties = NSDictionary::<NSString, AnyObject>::new();
                let png: Retained<NSData> =
                    msg_send![&*rep, representationUsingType: 4usize, properties: &*properties];
                let bytes = png.to_vec();
                assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
                std::fs::write(output, bytes).expect("write native snapshot");
            });
        });
        while !writer.is_finished() {
            smoke_run_loop_tick();
        }
        writer.join().expect("snapshot writer panicked");
    }
    eprintln!("PASS native bitmap snapshot {path}");
}
