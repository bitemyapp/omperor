//! Live, inert examples made from the conversation's native renderer and views.
//! These own sample text only: no delegate, actions, links, image loading or RPC.
use super::*;
use chat::{ToolStatus, TranscriptMessageKind};
use native_markdown::Typography;

#[derive(Debug)]
pub(super) enum TextExample {
    /// An activity, thinking or notice row, as the transcript's activity
    /// views set them.
    Transcript {
        view: Retained<NSTextView>,
        role: TextRole,
    },
    /// A user or assistant message, as the transcript's Markdown rows set it.
    Message {
        view: Retained<NSView>,
        role: TextRole,
    },
    Composer(ComposerExample),
    Header(HeaderExample),
}

impl TextExample {
    pub(super) fn new(role: TextRole, mtm: MainThreadMarker) -> Self {
        match role {
            TextRole::Composer | TextRole::Controls => Self::Composer(ComposerExample::new(mtm)),
            TextRole::Title | TextRole::Metadata => Self::Header(HeaderExample::new(mtm)),
            role if matches!(sample_message(role).kind, TranscriptMessageKind::User | TranscriptMessageKind::Assistant) => {
                let view = transcript_stack::example_container(mtm);
                view.setAccessibilityLabel(Some(&NSString::from_str(&format!("{} example", role.label()))));
                Self::Message { view, role }
            }
            _ => {
                let view =
                    NSTextView::initWithFrame(NSTextView::alloc(mtm), rect(0.0, 0.0, 320.0, 100.0));
                view.setEditable(false);
                view.setSelectable(true);
                view.setRichText(true);
                view.setDrawsBackground(false);
                view.setHorizontallyResizable(false);
                view.setVerticallyResizable(false);
                view.setTextContainerInset(NSSize::new(10.0, 10.0));
                view.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "{} example",
                    role.label()
                ))));
                Self::Transcript { view, role }
            }
        }
    }

    pub(super) fn view(&self) -> &NSView {
        match self {
            Self::Transcript { view, .. } => view,
            Self::Message { view, .. } => view,
            Self::Composer(example) => &example.view,
            Self::Header(example) => &example.view,
        }
    }

    /// Lay out at actual point size, never by scaling a rendered image. TextKit
    /// measures each transcript so even 40-point wrapped headings remain visible.
    pub(super) fn fit(&self, typography: Typography, width: f64, rerender: bool) -> f64 {
        match self {
            Self::Transcript { view, role } => {
                // SAFETY: NSTextView owns this container, storage and layout manager.
                let (container, storage, layout) = unsafe {
                    (
                        view.textContainer().unwrap(),
                        view.textStorage().unwrap(),
                        view.layoutManager().unwrap(),
                    )
                };
                let text_width = (width - 20.0).max(1.0);
                container.setWidthTracksTextView(false);
                container.setContainerSize(NSSize::new(text_width, 1.0e7));
                if rerender {
                    let rendered = native_markdown::render_message(
                        &sample_message(*role),
                        text_width,
                        std::path::Path::new("/"),
                        typography,
                        false,
                    );
                    storage.setAttributedString(&rendered);
                }
                layout.ensureLayoutForTextContainer(&container);
                let height = layout
                    .usedRectForTextContainer(&container)
                    .size
                    .height
                    .ceil()
                    + 20.0;
                view.setFrameSize(NSSize::new(width, height));
                height
            }
            Self::Message { view, role } => {
                if rerender || view.frame().size.width != width {
                    transcript_stack::set_example(view, &sample_message(*role), typography, width, 10.0)
                } else {
                    view.frame().size.height
                }
            }
            Self::Composer(example) => example.fit(typography, width),
            Self::Header(example) => example.fit(typography, width),
        }
    }
}

/// Each sample identifies the target through its real surrounding treatment:
/// speaker label, user enclosure, paragraph rhythm, code block, or status glyph.
fn sample_message(role: TextRole) -> TranscriptMessage {
    let (kind, body) = match role {
        TextRole::Assistant => (
            TranscriptMessageKind::Assistant,
            "Here’s a clearer approach.\n\nKeep the useful details close by.",
        ),
        TextRole::User => (
            TranscriptMessageKind::User,
            "Make this easier to read.\n\nKeep the details that matter.",
        ),
        TextRole::Heading1 => (
            TranscriptMessageKind::Assistant,
            "# The plan\n\nStart with a clear direction.",
        ),
        TextRole::Heading2 => (
            TranscriptMessageKind::Assistant,
            "## Next steps\n\nMake one change at a time.",
        ),
        TextRole::Heading3 => (
            TranscriptMessageKind::Assistant,
            "### A detail\n\nKeep the context in view.",
        ),
        TextRole::SmallHeading => (
            TranscriptMessageKind::Assistant,
            "#### A note\n\nSmaller headings organize the details.",
        ),
        TextRole::InlineCode => (
            TranscriptMessageKind::Assistant,
            "Run `cargo test` to check the change.",
        ),
        TextRole::CodeBlock | TextRole::CodeCaption => (
            TranscriptMessageKind::Assistant,
            "Here is the change:\n\n```rust\nlet ready = true;\n```",
        ),
        TextRole::Speaker => (TranscriptMessageKind::Assistant, "Here’s what I found."),
        TextRole::Activity => (TranscriptMessageKind::Tool, "Reviewed the interface"),
        TextRole::Thinking => (TranscriptMessageKind::Thinking, "Comparing the options…"),
        TextRole::Notice => (TranscriptMessageKind::Notice, "The session is ready."),
        _ => unreachable!("composer and header use native controls instead"),
    };
    TranscriptMessage {
        author: if kind == TranscriptMessageKind::User {
            "You"
        } else {
            "omp"
        }
        .into(),
        body: body.into(),
        kind,
        status: (role == TextRole::Activity).then_some(ToolStatus::Completed),
        detail_id: None,
    }
}

#[derive(Debug)]
pub(super) struct ComposerExample {
    view: Retained<NSView>,
    surface: Retained<NSBox>,
    input: Retained<NSTextField>,
    labels: [Retained<NSTextField>; 4],
    chips: [Retained<NSBox>; 4],
    bolt: Retained<NSImageView>,
    send: Retained<NSImageView>,
}

impl ComposerExample {
    fn new(mtm: MainThreadMarker) -> Self {
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 320.0, 170.0));
        let surface = card_box(rect(0.0, 0.0, 320.0, 158.0), 18.0, mtm);
        view.addSubview(&surface);
        let input = composer_input_field(mtm);
        input.setStringValue(ns_string!("Help me refine this idea."));
        input.setAccessibilityLabel(Some(ns_string!("Message input example")));
        input.setAccessibilityHelp(Some(ns_string!(
            "Message input example. Use the adjacent size control to adjust its text."
        )));
        view.addSubview(&input);
        let labels = ["GPT-6  ▾", "Reasoning: high  ▾", "Fast", "Personal  ▾"].map(|title| {
            let label = composer_control_label(title, mtm);
            view.addSubview(&label);
            label
        });
        let chips = std::array::from_fn(|index| {
            let chip = fill_box(
                rect(0.0, 0.0, 10.0, layout::CHIP_HEIGHT),
                &theme::surface_inset(),
                7.0,
                mtm,
            );
            view.addSubview_positioned_relativeTo(
                &chip,
                NSWindowOrderingMode::Below,
                Some(&labels[index]),
            );
            chip
        });
        let symbol_view = |symbol, size| {
            let image =
                NSImageView::initWithFrame(NSImageView::alloc(mtm), rect(0.0, 0.0, size, size));
            // SAFETY: AppKit exports this weight for the process lifetime.
            image.setImage(
                theme::symbol(symbol, size, unsafe { NSFontWeightSemibold }, None).as_deref(),
            );
            view.addSubview(&image);
            image
        };
        let bolt = symbol_view("bolt.fill", 12.0);
        bolt.setContentTintColor(Some(&theme::secondary_text()));
        let send = symbol_view("arrow.up.circle.fill", 32.0);
        send.setContentTintColor(Some(&theme::accent()));
        Self {
            view,
            surface,
            input,
            labels,
            chips,
            bolt,
            send,
        }
    }

    fn fit(&self, typography: Typography, width: f64) -> f64 {
        let input_height = layout::composer_input_height(typography.size(TextRole::Composer));

        self.input
            .setFont(Some(&typography.interface_font(TextRole::Composer)));

        let mut natural = std::array::from_fn(|index| {
            self.labels[index].setFont(Some(&typography.interface_font(TextRole::Controls)));
            self.labels[index].sizeToFit();
            self.labels[index].frame().size.width + 18.0 + if index == 2 { 16.0 } else { 0.0 }
        });
        // Settings can be narrower than the live composer. Keep the three
        // controls named by this setting legible in the compact example.
        let hide_fast = width < 340.0;
        self.labels[2].setHidden(hide_fast);
        self.chips[2].setHidden(hide_fast);
        self.bolt.setHidden(hide_fast);
        if hide_fast {
            natural[2] = 0.0;
        }
        let positions = layout::composer_chips(width, natural, false);
        let input_y = layout::composer_input_y(&positions);
        let height = input_y + input_height + 10.0;
        self.view.setFrameSize(NSSize::new(width, height));
        self.surface.setFrame(rect(0.0, 0.0, width, height));
        self.input
            .setFrame(rect(20.0, input_y, width - 40.0, input_height));
        for (index, position) in positions.into_iter().enumerate() {
            let leading = if index == 2 { 25.0 } else { 9.0 };
            let label_height = self.labels[index].frame().size.height;
            self.chips[index].setFrame(rect(
                position.x,
                position.y,
                position.width,
                layout::CHIP_HEIGHT,
            ));
            self.labels[index].setFrame(rect(
                position.x + leading,
                position.y + (layout::CHIP_HEIGHT - label_height) / 2.0,
                (position.width - leading - 9.0).max(1.0),
                label_height,
            ));
            if index == 2 {
                self.bolt
                    .setFrame(rect(position.x + 9.0, position.y + 9.0, 12.0, 12.0));
            }
        }
        self.send.setFrame(rect(width - 48.0, 10.0, 32.0, 32.0));
        height
    }
}

#[derive(Debug)]
pub(super) struct HeaderExample {
    view: Retained<NSView>,
    title: Retained<NSTextField>,
    path: Retained<NSTextField>,
    status: Retained<NSTextField>,
    hint: Retained<NSTextField>,
}

impl HeaderExample {
    fn new(mtm: MainThreadMarker) -> Self {
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 320.0, 132.0));
        let title = text_label("Plan the next step", 20.0, true, mtm);
        let path = text_label("Omperor · workspace", 12.0, false, mtm);
        let status = text_label("", 12.0, false, mtm);
        let hint = text_label(
            "Return to send · Shift Return for a new line",
            12.0,
            false,
            mtm,
        );
        for label in [&title, &path, &status, &hint] {
            label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            view.addSubview(label);
        }
        path.setTextColor(Some(&theme::secondary_text()));
        hint.setTextColor(Some(&theme::secondary_text()));
        Self {
            view,
            title,
            path,
            status,
            hint,
        }
    }

    fn fit(&self, typography: Typography, width: f64) -> f64 {
        self.view.setFrameSize(NSSize::new(width, 132.0));
        self.title
            .setFont(Some(&typography.interface_font(TextRole::Title)));
        self.path
            .setFont(Some(&typography.interface_font(TextRole::Metadata)));
        self.hint
            .setFont(Some(&typography.interface_font(TextRole::Metadata)));
        self.status.setAttributedStringValue(&status_attributed(
            "Ready for your next message",
            typography.size(TextRole::Metadata),
        ));
        for (label, y, height) in [
            (&self.title, 92.0, 32.0),
            (&self.path, 67.0, 24.0),
            (&self.status, 39.0, 24.0),
            (&self.hint, 7.0, 24.0),
        ] {
            label.setFrame(rect(10.0, y, width - 20.0, height));
            label.setToolTip(Some(&label.stringValue()));
        }
        132.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_examples_use_only_local_text_and_have_no_actions() {
        for role in TextRole::ALL {
            if matches!(
                role,
                TextRole::Composer | TextRole::Controls | TextRole::Title | TextRole::Metadata
            ) {
                continue;
            }
            let sample = sample_message(role);
            assert!(sample.detail_id.is_none());
            let document = markdown::parse(&sample.body);
            assert!(document.images.is_empty());
            assert!(document.runs.iter().all(|run| run.style.link.is_none()));
        }
    }
}
