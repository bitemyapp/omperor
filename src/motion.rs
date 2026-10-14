//! How things move.
//!
//! One tempo for every press, fulfilment, and fade in the app, and one way to
//! run a grouped animation, so a row in the Agents panel and a row in the
//! sidebar answer the same gesture the same way.
//!
//! The gesture has three beats. A press tints the target with a share of the
//! gold it would get if chosen and plants a seed of full gold at its edge: the
//! app has heard, and says what is being chosen. Release inside fulfils the
//! press: the seed sweeps across the target and dissolves as the tint rises
//! to the settled selection. Release outside, or a selection that ends,
//! fades without hurry.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_app_kit::{NSAnimationContext, NSView, NSWorkspace};
use objc2_foundation::{NSNumber, NSString};

/// A press is acknowledged within a frame or two, before anything the press
/// starts could have happened.
pub(crate) const PRESS_IN: f64 = 0.09;

/// Fulfilment: the seed sweeps the row and dissolves into the settled pill.
/// About as long as a sheet takes to slide in beside it, so the two read as
/// one motion.
pub(crate) const BLOOM: f64 = 0.28;

/// An abandoned press or an ended selection leaves without hurry.
pub(crate) const FADE_OUT: f64 = 0.2;

/// How much of the selection's gold a press already shows: enough to say what
/// is being chosen, short of saying it is chosen.
pub(crate) const PRESS_TINT: f64 = 0.38;

/// How long a confirmation such as "Copied …" holds the status line before
/// the line goes back to what it was saying.
pub(crate) const FLASH_HOLD: f64 = 1.6;

/// Runs `changes` as one implicit animation group lasting `duration` seconds.
/// Properties set through a view's `animator()` inside `changes` interpolate
/// over the group; properties set on the view itself apply at once.
pub(crate) fn animate(duration: f64, changes: impl Fn()) {
    let block = RcBlock::new(move |context: NonNull<NSAnimationContext>| {
        // SAFETY: AppKit hands the group a live context for the call.
        unsafe { context.as_ref() }.setDuration(duration);
        changes();
    });
    NSAnimationContext::runAnimationGroup(&block);
}

/// As `animate`, then runs `done` once the group has finished, for a motion
/// that has a second half: fade out, change, fade back in.
pub(crate) fn animate_then(duration: f64, changes: impl Fn(), done: impl Fn() + 'static) {
    let block = RcBlock::new(move |context: NonNull<NSAnimationContext>| {
        // SAFETY: AppKit hands the group a live context for the call.
        unsafe { context.as_ref() }.setDuration(duration);
        changes();
    });
    let done = RcBlock::new(done);
    NSAnimationContext::runAnimationGroup_completionHandler(&block, Some(&done));
}

// MARK: Bounce
//
// The transcript's life: what arrives settles in on a spring, work in
// flight turns, and work that lands pops. Every motion here is a Core
// Animation animation on a layer — the render server runs it; no frame
// of it costs the main thread a layout or a draw — and none changes a
// model value, so what the reader's place is measured from never moves.
// Reduce Motion keeps only a short fade, or nothing.


/// A message's arrival: it rises this far into place.
const SETTLE_RISE: f64 = 10.0;
/// The spring every arrival and pop shares: quick, with one small
/// overshoot (a damping ratio of about 0.55), settled in under half a second.
const SPRING_MASS: f64 = 1.0;
const SPRING_STIFFNESS: f64 = 320.0;
const SPRING_DAMPING: f64 = 20.0;
/// A selection sliding sideways is firmer (a damping ratio of about 0.78):
/// across a whole segment, the shared spring's overshoot would carry it a
/// tenth of the way past its place.
const SLIDE_DAMPING: f64 = 28.0;
/// A new message's fade: over before the spring has settled.
const SETTLE_FADE: f64 = 0.22;
/// One turn of a working spinner.
pub(crate) const SPIN_PERIOD: f64 = 0.9;

/// Whether the user asked for less motion (Accessibility › Display).
pub(crate) fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

fn class(name: &std::ffi::CStr) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

fn number(value: f64) -> Retained<NSNumber> {
    NSNumber::numberWithDouble(value)
}

/// A spring animation of `key_path` from `from` to `to`.
fn spring(key_path: &str, from: f64, to: f64) -> Option<Retained<AnyObject>> {
    spring_damped(key_path, from, to, SPRING_DAMPING)
}

fn spring_damped(key_path: &str, from: f64, to: f64, damping: f64) -> Option<Retained<AnyObject>> {
    let class = class(c"CASpringAnimation")?;
    // SAFETY: CASpringAnimation's class method and setters take these types.
    unsafe {
        let animation: Retained<AnyObject> = msg_send![class, animationWithKeyPath: &*NSString::from_str(key_path)];
        let _: () = msg_send![&*animation, setMass: SPRING_MASS];
        let _: () = msg_send![&*animation, setStiffness: SPRING_STIFFNESS];
        let _: () = msg_send![&*animation, setDamping: damping];
        let _: () = msg_send![&*animation, setFromValue: &*number(from)];
        let _: () = msg_send![&*animation, setToValue: &*number(to)];
        let settling: f64 = msg_send![&*animation, settlingDuration];
        let _: () = msg_send![&*animation, setDuration: settling];
        Some(animation)
    }
}

/// A plain animation of `key_path` over `duration`, eased out.
fn basic(key_path: &str, from: f64, to: f64, duration: f64) -> Option<Retained<AnyObject>> {
    let class = class(c"CABasicAnimation")?;
    let timing = class_timing(c"easeOut");
    // SAFETY: CABasicAnimation's class method and setters take these types.
    unsafe {
        let animation: Retained<AnyObject> = msg_send![class, animationWithKeyPath: &*NSString::from_str(key_path)];
        let _: () = msg_send![&*animation, setFromValue: &*number(from)];
        let _: () = msg_send![&*animation, setToValue: &*number(to)];
        let _: () = msg_send![&*animation, setDuration: duration];
        if let Some(timing) = timing {
            let _: () = msg_send![&*animation, setTimingFunction: &*timing];
        }
        Some(animation)
    }
}

fn class_timing(name: &std::ffi::CStr) -> Option<Retained<AnyObject>> {
    let class = class(c"CAMediaTimingFunction")?;
    let name = NSString::from_str(name.to_str().ok()?);
    // SAFETY: the class method takes a timing-function name.
    Some(unsafe { msg_send![class, functionWithName: &*name] })
}

fn add(layer: &AnyObject, animation: Option<Retained<AnyObject>>, key: &str) {
    if let Some(animation) = animation {
        // SAFETY: CALayer's addAnimation:forKey: takes an animation and a key.
        let _: () = unsafe { msg_send![layer, addAnimation: &*animation, forKey: &*NSString::from_str(key)] };
    }
}

fn layer_of(view: &NSView) -> Option<Retained<AnyObject>> {
    // SAFETY: -[NSView layer] returns the view's layer or nil.
    unsafe { msg_send![view, layer] }
}

/// A new message settles into place: it fades in as it rises the last few
/// points on the spring. Only the layer moves; the view's frame, which the
/// reader's place is measured from, is where it will stay.
pub(crate) fn settle_in(view: &NSView) {
    let Some(layer) = layer_of(view) else { return };
    add(&layer, basic("opacity", 0.0, 1.0, if reduce_motion() { 0.12 } else { SETTLE_FADE }), "omperor.settle.fade");
    if reduce_motion() {
        return;
    }
    // Down the screen is +y in a flipped layer tree, −y otherwise.
    // SAFETY: contentsAreFlipped takes no arguments and returns a BOOL.
    let flipped: bool = unsafe { msg_send![&*layer, contentsAreFlipped] };
    let rise = if flipped { SETTLE_RISE } else { -SETTLE_RISE };
    add(&layer, spring("transform.translation.y", rise, 0.0), "omperor.settle.rise");
}

/// A layer that turns for as long as it lives: a working spinner.
pub(crate) fn spin(layer: &AnyObject) {
    if reduce_motion() {
        return;
    }
    let Some(animation) = basic("transform.rotation.z", 0.0, -std::f64::consts::TAU, SPIN_PERIOD) else { return };
    // SAFETY: repeatCount takes a float; linear timing reads as steady work.
    unsafe {
        let _: () = msg_send![&*animation, setRepeatCount: f32::INFINITY];
        if let Some(linear) = class_timing(c"linear") {
            let _: () = msg_send![&*animation, setTimingFunction: &*linear];
        }
    }
    add(layer, Some(animation), "omperor.spin");
}

/// A call that landed: a ring springs out from its mark and fades, the
/// press's gold seed spent in one beat.
pub(crate) fn bloom(layer: &AnyObject) {
    if reduce_motion() {
        return;
    }
    add(layer, spring("transform.scale", 0.45, 1.0), "omperor.bloom");
    add(layer, basic("opacity", 0.9, 0.0, 0.55), "omperor.bloom.fade");
}

/// A press that sends something on its way: the control hops and lands on
/// the spring, a beat of bounce on top of whatever the press starts.
pub(crate) fn hop(view: &NSView) {
    if reduce_motion() {
        return;
    }
    let Some(layer) = layer_of(view) else { return };
    // SAFETY: contentsAreFlipped takes no arguments and returns a BOOL.
    let flipped: bool = unsafe { msg_send![&*layer, contentsAreFlipped] };
    let lift = if flipped { -5.0 } else { 5.0 };
    add(&layer, spring("transform.translation.y", lift, 0.0), "omperor.hop");
}

/// Something in flight that has no end to show yet: it breathes, growing
/// and brightening and back, for as long as it lives.
pub(crate) fn breathe(layer: &AnyObject) {
    if reduce_motion() {
        return;
    }
    for (key, from, to) in [("transform.scale", 0.62, 1.0), ("opacity", 0.45, 1.0)] {
        let Some(animation) = basic(key, from, to, BREATH / 2.0) else { continue };
        // SAFETY: autoreverses takes a BOOL, repeatCount a float.
        unsafe {
            let _: () = msg_send![&*animation, setAutoreverses: true];
            let _: () = msg_send![&*animation, setRepeatCount: f32::INFINITY];
            if let Some(ease) = class_timing(c"easeInEaseOut") {
                let _: () = msg_send![&*animation, setTimingFunction: &*ease];
            }
        }
        add(layer, Some(animation), &format!("omperor.breathe.{key}"));
    }
}

/// One breath of the streaming orb, in and out.
const BREATH: f64 = 1.1;

/// A selection that moved sideways: the view is already where it belongs,
/// and its layer springs the last `from_dx` points there, overshooting a
/// little. Reduce Motion lands it at once.
pub(crate) fn slide_x(view: &NSView, from_dx: f64) {
    if reduce_motion() || from_dx.abs() < 0.5 {
        return;
    }
    let Some(layer) = layer_of(view) else { return };
    add(&layer, spring_damped("transform.translation.x", from_dx, 0.0, SLIDE_DAMPING), "omperor.slide");
}

/// What a view shows is about to change (a label's color): the change
/// cross-fades over `duration` rather than landing at once. Only the layer
/// fades between its old and new drawing; nothing is drawn per frame.
pub(crate) fn crossfade(view: &NSView, duration: f64) {
    let Some(layer) = layer_of(view) else { return };
    let Some(class) = class(c"CATransition") else { return };
    // SAFETY: CATransition's class method and setters take these types; the
    // type name is Core Animation's documented string value.
    unsafe {
        let transition: Retained<AnyObject> = msg_send![class, animation];
        let _: () = msg_send![&*transition, setType: &*NSString::from_str("fade")];
        let _: () = msg_send![&*transition, setDuration: if reduce_motion() { duration.min(0.12) } else { duration }];
        if let Some(timing) = class_timing(c"easeOut") {
            let _: () = msg_send![&*transition, setTimingFunction: &*timing];
        }
        add(&layer, Some(transition), "omperor.crossfade");
    }
}

/// A figure that changed: the new one rolls up into the old one's place,
/// the way a counter turns. The render server plays it from the layer's
/// last contents; nothing is laid out or drawn per frame. Reduce Motion
/// keeps a short fade.
pub(crate) fn tick(view: &NSView) {
    let Some(layer) = layer_of(view) else { return };
    let Some(class) = class(c"CATransition") else { return };
    let reduced = reduce_motion();
    // SAFETY: CATransition's class method and setters take these types; the
    // type and subtype names are Core Animation's documented string values.
    let transition: Retained<AnyObject> = unsafe {
        let transition: Retained<AnyObject> = msg_send![class, animation];
        let kind = NSString::from_str(if reduced { "fade" } else { "push" });
        let _: () = msg_send![&*transition, setType: &*kind];
        if !reduced {
            let subtype = NSString::from_str("fromBottom");
            let _: () = msg_send![&*transition, setSubtype: &*subtype];
        }
        let _: () = msg_send![&*transition, setDuration: if reduced { 0.12 } else { BLOOM }];
        if let Some(timing) = class_timing(c"easeOut") {
            let _: () = msg_send![&*transition, setTimingFunction: &*timing];
        }
        transition
    };
    add(&layer, Some(transition), "omperor.tick");
}
