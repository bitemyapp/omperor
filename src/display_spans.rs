//! Always-on `perf` spans around what AppKit does for a class in its display
//! cycle — `layout`, `viewWillDraw`, `drawRect:` — for classes Omperor does
//! not define, such as Upleft's hosted `MarkdownTextView`. A pass over the
//! frame budget then names the transcript's layout and drawing instead of
//! reporting no spans. Each wrapper is a clock read and a ledger push.
use std::{ffi::CStr, sync::Mutex};

use objc2::{
    runtime::{AnyClass, Imp, Sel},
    sel,
};
use objc2_app_kit::NSView;
use objc2_foundation::NSRect;

use crate::perf;

/// The wrapped classes' original implementations, by class and selector.
static ORIGINALS: Mutex<Vec<(usize, Sel, Imp)>> = Mutex::new(Vec::new());

/// Wraps `class`'s `layout`, `viewWillDraw`, and `drawRect:` (if its own) in
/// spans named `layout`, `will_draw`, and `draw`. Once per class; main thread.
pub(crate) fn wrap(class: &'static AnyClass, layout: &'static str, will_draw: &'static str, draw: &'static str) {
    NAMES.with(|names| {
        let key = class as *const _ as usize;
        let mut names = names.borrow_mut();
        if names.iter().any(|(kept, ..)| *kept == key) {
            return;
        }
        names.push((key, layout, will_draw, draw));
        hook(class, sel!(layout), wrapped_void as Imp0, c"v@:");
        hook(class, sel!(viewWillDraw), wrapped_void as Imp0, c"v@:");
        // SAFETY: the replacement has drawRect:'s C signature.
        let draw_imp: Imp = unsafe { std::mem::transmute::<extern "C-unwind" fn(&NSView, Sel, NSRect), Imp>(wrapped_draw) };
        // Only a drawRect: of the class's own: adding one would make AppKit
        // give a view that never drew a backing store to draw into.
        if class.instance_methods().iter().any(|method| method.name() == sel!(drawRect:)) {
            hook_imp(class, sel!(drawRect:), draw_imp, c"v@:{CGRect={CGPoint=dd}{CGSize=dd}}");
        }
    });
}

type Imp0 = extern "C-unwind" fn(&NSView, Sel);

thread_local! {
    static NAMES: std::cell::RefCell<Vec<(usize, &'static str, &'static str, &'static str)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn hook(class: &'static AnyClass, selector: Sel, imp: Imp0, types: &CStr) {
    // SAFETY: the replacement has the C signature of a `void` method with no
    // arguments, which `layout` and `viewWillDraw` are.
    let imp: Imp = unsafe { std::mem::transmute::<Imp0, Imp>(imp) };
    hook_imp(class, selector, imp, types);
}

fn hook_imp(class: &'static AnyClass, selector: Sel, imp: Imp, types: &CStr) {
    let key = class as *const _ as usize;
    let own = class.instance_methods().iter().find(|method| method.name() == selector).copied();
    let original = match own {
        // SAFETY: the replacement calls the kept original with the same
        // arguments.
        Some(method) => unsafe { method.set_implementation(imp) },
        None => {
            // Inherited: the class gets a method of its own that calls the
            // superclass's.
            let Some(inherited) = class.superclass().and_then(|superclass| superclass.instance_method(selector)) else {
                return;
            };
            let original = inherited.implementation();
            // SAFETY: the class exists, the selector and type encoding match
            // the inherited method's, and the IMP has its C signature.
            let added = unsafe {
                objc2::ffi::class_addMethod((class as *const AnyClass).cast_mut(), selector, imp, types.as_ptr())
            };
            if !added.as_bool() {
                return;
            }
            original
        }
    };
    ORIGINALS.lock().unwrap_or_else(|poison| poison.into_inner()).push((key, selector, original));
}

/// The original for `view`'s class (or the nearest wrapped superclass), and
/// the span name for `selector`.
fn original(view: &NSView, selector: Sel) -> (Option<Imp>, &'static str) {
    let kept = ORIGINALS.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut class = Some(view.class());
    while let Some(current) = class {
        let key = current as *const _ as usize;
        if let Some((_, _, imp)) = kept.iter().find(|(kept, kept_sel, _)| *kept == key && *kept_sel == selector) {
            let name = NAMES.with(|names| {
                names.borrow().iter().find(|(kept, ..)| *kept == key).map_or("gui.display.unnamed", |(_, layout, will_draw, draw)| {
                    if selector == sel!(layout) {
                        layout
                    } else if selector == sel!(viewWillDraw) {
                        will_draw
                    } else {
                        draw
                    }
                })
            });
            return (Some(*imp), name);
        }
        class = current.superclass();
    }
    (None, "gui.display.unnamed")
}

extern "C-unwind" fn wrapped_void(view: &NSView, selector: Sel) {
    let (imp, name) = original(view, selector);
    let Some(imp) = imp else { return };
    let _span = perf::span(name);
    // SAFETY: the kept IMP is the method's own, with this signature.
    let imp: Imp0 = unsafe { std::mem::transmute::<Imp, Imp0>(imp) };
    imp(view, selector);
}

extern "C-unwind" fn wrapped_draw(view: &NSView, selector: Sel, rect: NSRect) {
    let (imp, name) = original(view, selector);
    let Some(imp) = imp else { return };
    let _span = perf::span(name);
    // SAFETY: the kept IMP is drawRect:'s own, with this signature.
    let imp: extern "C-unwind" fn(&NSView, Sel, NSRect) = unsafe { std::mem::transmute(imp) };
    imp(view, selector, rect);
}
