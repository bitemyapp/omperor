//! Every event AppKit dispatches is a named span on the main thread's
//! ledger, so a pass over the frame budget can say it was a scroll, a key,
//! or a click that ran long — not only what the app's own timers did.
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSEvent, NSEventType};

use crate::perf;

static DISPATCHED: AtomicU64 = AtomicU64::new(0);
static SCROLLS: AtomicU64 = AtomicU64::new(0);

/// Events dispatched so far, and how many were scroll wheels.
pub(crate) fn dispatched() -> (u64, u64) {
    (
        DISPATCHED.load(Ordering::Relaxed),
        SCROLLS.load(Ordering::Relaxed),
    )
}

define_class!(
    // SAFETY:
    // - NSApplication has no subclassing requirements beyond being the class
    //   `sharedApplication` is first sent to, which `shared_application` is.
    // - MainThreadOnly matches AppKit's threading model.
    // - OmperorApplication does not implement Drop.
    #[unsafe(super = NSApplication)]
    #[thread_kind = MainThreadOnly]
    #[name = "OmperorApplication"]
    #[derive(Debug)]
    struct OmperorApplication;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for OmperorApplication {}

    impl OmperorApplication {
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            let kind = event.r#type();
            DISPATCHED.fetch_add(1, Ordering::Relaxed);
            if kind == NSEventType::ScrollWheel {
                SCROLLS.fetch_add(1, Ordering::Relaxed);
            }
            let _span = perf::span(event_name(kind));
            // SAFETY: NSApplication's own dispatch, with the event it was given.
            unsafe { msg_send![super(self), sendEvent: event] }
        }
    }
);

/// The shared application, created as the subclass that times events. Must
/// be the first `sharedApplication` sent in the process.
pub(crate) fn shared_application(mtm: MainThreadMarker) -> Retained<NSApplication> {
    let _ = mtm;
    // SAFETY: `sharedApplication` on an NSApplication subclass returns the
    // singleton, creating it as that subclass the first time.
    unsafe { msg_send![OmperorApplication::class(), sharedApplication] }
}

fn event_name(kind: NSEventType) -> &'static str {
    match kind {
        NSEventType::LeftMouseDown => "event.left_mouse_down",
        NSEventType::LeftMouseUp => "event.left_mouse_up",
        NSEventType::RightMouseDown => "event.right_mouse_down",
        NSEventType::RightMouseUp => "event.right_mouse_up",
        NSEventType::MouseMoved => "event.mouse_moved",
        NSEventType::LeftMouseDragged => "event.left_mouse_dragged",
        NSEventType::RightMouseDragged => "event.right_mouse_dragged",
        NSEventType::MouseEntered => "event.mouse_entered",
        NSEventType::MouseExited => "event.mouse_exited",
        NSEventType::KeyDown => "event.key_down",
        NSEventType::KeyUp => "event.key_up",
        NSEventType::FlagsChanged => "event.flags_changed",
        NSEventType::AppKitDefined => "event.appkit_defined",
        NSEventType::SystemDefined => "event.system_defined",
        NSEventType::ApplicationDefined => "event.application_defined",
        NSEventType::Periodic => "event.periodic",
        NSEventType::CursorUpdate => "event.cursor_update",
        NSEventType::ScrollWheel => "event.scroll_wheel",
        NSEventType::TabletPoint => "event.tablet_point",
        NSEventType::TabletProximity => "event.tablet_proximity",
        NSEventType::OtherMouseDown => "event.other_mouse_down",
        NSEventType::OtherMouseUp => "event.other_mouse_up",
        NSEventType::OtherMouseDragged => "event.other_mouse_dragged",
        NSEventType::Gesture => "event.gesture",
        NSEventType::Magnify => "event.magnify",
        NSEventType::Swipe => "event.swipe",
        NSEventType::Rotate => "event.rotate",
        NSEventType::BeginGesture => "event.begin_gesture",
        NSEventType::EndGesture => "event.end_gesture",
        NSEventType::SmartMagnify => "event.smart_magnify",
        NSEventType::QuickLook => "event.quick_look",
        NSEventType::Pressure => "event.pressure",
        NSEventType::DirectTouch => "event.direct_touch",
        NSEventType::ChangeMode => "event.change_mode",
        _ => "event.other",
    }
}
