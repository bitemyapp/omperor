//! Coalesced worker completion delivery. An idle app needs no polling timer.
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2_core_foundation::{CFRunLoop, kCFRunLoopCommonModes};
use objc2_foundation::{NSNotificationCenter, NSString, ns_string};

static QUEUED: AtomicBool = AtomicBool::new(false);

pub(crate) fn notification() -> &'static NSString {
    ns_string!("OmperorBackgroundJobsReady")
}

pub(crate) fn notify() {
    if QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    let run_loop = CFRunLoop::main().expect("AppKit has a main run loop");
    let block = RcBlock::new(|| {
        // Clear before draining: a completion arriving during the drain must
        // schedule another delivery. The event is queued before notify runs.
        QUEUED.store(false, Ordering::Release);
        // SAFETY: this block runs only on the main run loop. The name is a
        // process-lifetime NSString and observers belong to the AppDelegate.
        unsafe {
            NSNotificationCenter::defaultCenter().postNotificationName_object(notification(), None);
        }
    });
    // SAFETY: CFRunLoopPerformBlock copies the block and supports scheduling
    // from workers. Common modes also deliver while a menu or termination's
    // nested loop is active. No AppKit object is captured across threads.
    unsafe {
        run_loop.perform_block(
            kCFRunLoopCommonModes.map(|mode| mode.as_ref()),
            Some(&block),
        );
    }
    run_loop.wake_up();
}
