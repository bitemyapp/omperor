//! Waking the interface from other threads. The runtime's threads, the
//! file watcher and discovery deliver their results on channels; when one
//! has something, it calls [`notify`], which sends the program one
//! [`crate::app::Msg::Wake`] however many arrive before it is handled.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

type Waker = Box<dyn Fn() + Send + Sync>;

static WAKER: RwLock<Option<Waker>> = RwLock::new(None);
static QUEUED: AtomicBool = AtomicBool::new(false);

/// Install what wakes the interface (when the app starts; a test's app
/// replaces the one before it).
pub(crate) fn install(waker: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut slot) = WAKER.write() {
        *slot = Some(Box::new(waker));
    }
    QUEUED.store(false, Ordering::Release);
}

/// Ask for a wake-up. Coalesced: until the interface takes the one queued
/// ([`taken`]), more asks send nothing.
pub(crate) fn notify() {
    if QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    match WAKER.read().ok().as_ref().and_then(|slot| slot.as_ref()) {
        Some(waker) => waker(),
        // Nothing to wake (headless runs): let the next ask try again.
        None => QUEUED.store(false, Ordering::Release),
    }
}

/// The interface took the wake-up: clear it before draining, so what
/// arrives during the drain asks again.
pub(crate) fn taken() {
    QUEUED.store(false, Ordering::Release);
}
