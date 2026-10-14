//! Exercise the staged Sidestep build with the transcript's real renderer.
//! The null backend runs native layout and rendering without a desktop window.

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() {
    use objc2::rc::Retained;
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, msg_send};
    use objc2_app_kit::{
        NSAnimatablePropertyContainer, NSAnimationContext, NSApplication, NSBackingStoreType,
        NSScrollView, NSTextStorage, NSView, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use sidestep::appkit::testing;
    use upleft_core::{DirtySet, parser::MarkdownParser};
    use upleft_render::view::markdown_text_view::MarkdownTextView;

    // Prepare the large document before starting the GUI. Production parses
    // transcripts on its job pool; this test likewise keeps parsing off AppKit.
    let (source, parsed) = std::thread::spawn(|| {
        let source = (0..240)
            .map(|i| {
                format!("Paragraph {i}: **scrolling** should reveal this text immediately.\n\n")
            })
            .collect::<String>();
        let parsed = MarkdownParser::parse(&source);
        (source, parsed)
    })
    .join()
    .unwrap();
    let mtm = MainThreadMarker::new().unwrap();
    testing::use_null_backend();
    NSApplication::sharedApplication(mtm);
    let size = NSSize::new(600.0, 240.0);
    let frame = NSRect::new(NSPoint::ZERO, size);
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    // No scrollers: their damage must not mask a missing viewport pass.
    let storage: Retained<NSTextStorage> =
        unsafe { msg_send![NSTextStorage::alloc(), initWithString: &*NSString::from_str(&source)] };
    let view = MarkdownTextView::new_hosted(
        &storage,
        MarkdownTextView::fallback_style_sheet(),
        size.width,
        mtm,
    );
    view.update(parsed, &DirtySet::wholesale(), false);
    scroll.setDocumentView(Some(&view));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setContentView(Some(&scroll));
    // From the first frame: tiles drawn then may be shown much later.
    testing::note_painted_text(true);
    window.orderFront(None);
    testing::settle_first_frames();
    let controller = view
        .textLayoutManager()
        .unwrap()
        .textViewportLayoutController();
    let clip = scroll.contentView();
    // The viewport is what shows and the overdraw drawn into scroll tiles
    // around it: it covers what shows, and stays near it.
    let covers = |viewport: NSRect, visible: NSRect| {
        let top = visible.origin.y.max(0.0);
        let bottom = (visible.origin.y + visible.size.height).min(view.frame().size.height);
        viewport.origin.y <= top + 1.0
            && viewport.origin.y + viewport.size.height >= bottom - 1.0
            && viewport.size.height <= visible.size.height * 5.0 + 2048.0
    };
    let end = view.frame().size.height - clip.bounds().size.height;
    assert!(end > 2000.0, "the fixture must exceed several viewports");
    // Alternate directions, including jumps beyond the layer canvas and
    // small moves within an existing tile. No explicit viewport layout,
    // invalidation or idle delay is allowed to make these steps work.
    let mut display_us = Vec::new();
    for fraction in [0.25, 0.75, 0.1, 0.9, 0.5, 0.0, 1.0, 0.4, 0.41, 0.42] {
        let started = std::time::Instant::now();
        clip.scrollToPoint(NSPoint::new(0.0, end * fraction));
        testing::display_now(&window);
        display_us.push(started.elapsed().as_micros());
        testing::settle();
        let expected = view.convertRect_fromView(clip.bounds(), Some(&clip));
        let actual = controller.viewportBounds();
        assert!(
            covers(actual, expected),
            "stale markdown viewport: {actual:?}, visible: {expected:?}"
        );
        assert!(
            controller.viewportRange().is_some(),
            "the new viewport contains laid out text"
        );
    }
    // Real input stays in a live touchpad gesture throughout these checks;
    // rendering may not wait for its ended phase or a quiet period.
    let start = clip.bounds().origin.y;
    let id = testing::showing_id(&window);
    for phase in [1, 2, 2, 2] {
        testing::inject_scroll(id, 40.0, 40.0, (0.0, 240.0), phase, (0.0, 0.0));
        testing::settle();
        let expected = view.convertRect_fromView(clip.bounds(), Some(&clip));
        let actual = controller.viewportBounds();
        assert!(
            covers(actual, expected),
            "markdown must prepare during the gesture: {actual:?}, visible: {expected:?}"
        );
    }
    assert!(
        clip.bounds().origin.y > start + 500.0,
        "injected touchpad input must actually scroll"
    );
    testing::inject_scroll(id, 40.0, 40.0, (0.0, 0.0), 3, (0.0, 0.0));
    testing::settle();

    // Small steps, as a touchpad moves: tiles come into view from those
    // drawn ahead of the scroll, which must hold the text. Down first, then
    // back up over text laid out already, past the tiles kept from the way
    // down, checking that what shows has no blank taller than a paragraph
    // gap where nothing was ever painted.
    let mut painted: Vec<f64> = Vec::new();
    let check = |painted: &mut Vec<f64>| {
        painted.extend(testing::take_painted_text().iter().map(|run| f64::from(run.y)));
        let visible = clip.bounds();
        let (top, bottom) = (visible.origin.y, visible.origin.y + visible.size.height);
        let mut seen: Vec<f64> = painted.iter().copied().filter(|p| *p >= top && *p <= bottom).collect();
        seen.sort_by(f64::total_cmp);
        let mut last = top;
        for p in seen.into_iter().chain([bottom]) {
            assert!(p - last < 80.0, "a blank from {last:.0} to {p:.0} shows: text there was never painted");
            last = p;
        }
    };
    let step = |dy: f64| {
        let y = (clip.bounds().origin.y + dy).clamp(0.0, end);
        clip.scrollToPoint(NSPoint::new(0.0, y));
        scroll.reflectScrolledClipView(&clip);
        testing::settle();
    };
    clip.scrollToPoint(NSPoint::new(0.0, end * 0.3));
    scroll.reflectScrolledClipView(&clip);
    testing::settle();
    for _ in 0..50 {
        step(40.0);
        check(&mut painted);
    }
    painted.clear();
    let turned = clip.bounds().origin.y;
    for _ in 0..100 {
        step(-40.0);
        if clip.bounds().origin.y < turned - 2000.0 {
            check(&mut painted);
        }
    }
    assert!(clip.bounds().origin.y < turned - 2400.0, "the fixture must allow the way back up");
    testing::note_painted_text(false);

    // Verify animator dispatch and interpolation in this exact build too,
    // including release builds where objc2's debug checks are disabled.
    use objc2_quartz_core::{CAMediaTiming, CAMediaTimingFunction, kCAMediaTimingFunctionLinear};
    let marker = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(20.0, 20.0)),
    );
    marker.setWantsLayer(true);
    scroll.addSubview(&marker);
    testing::settle();
    let target = marker.clone();
    let changes = block2::RcBlock::new(move |context: std::ptr::NonNull<NSAnimationContext>| {
        let context = unsafe { context.as_ref() };
        context.setDuration(1000.0);
        context.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
            kCAMediaTimingFunctionLinear
        })));
        assert!(!context.allowsImplicitAnimation());
        target.animator().setFrame(NSRect::new(
            NSPoint::new(100.0, 0.0),
            NSSize::new(40.0, 20.0),
        ));
        target.animator().setAlphaValue(0.5);
    });
    NSAnimationContext::runAnimationGroup(&changes);
    testing::settle();
    let layer = marker.layer().unwrap();
    let animation = unsafe { layer.animationForKey(&NSString::from_str("position")) }
        .expect("animated movement");
    let halfway = testing::presented_layer(&layer, animation.beginTime() + 500.0).unwrap();
    assert!((halfway.position[0] - 50.0).abs() < 0.01);
    assert!((halfway.bounds[2] - 30.0).abs() < 0.01);
    assert!((halfway.opacity - 0.75).abs() < 0.01);
    window.close();
    testing::settle();
    display_us.sort_unstable();
    println!(
        "Markdown scrolling main-thread display: median {} us, maximum {} us ({} steps)",
        display_us[display_us.len() / 2],
        display_us.last().unwrap(),
        display_us.len()
    );
    println!("PASS Linux markdown viewport follows jumps and live touchpad scrolling");
    println!("PASS Linux markdown text is painted into tiles drawn ahead of a scroll");
    println!("PASS Linux animator interpolates position, size, and opacity");
}
