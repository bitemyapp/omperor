use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSAttachmentAttributeName,
    NSBackgroundColorAttributeName, NSColor, NSFont, NSFontAttributeName,
    NSFontDescriptorSymbolicTraits, NSFontWeight, NSFontWeightRegular, NSFontWeightSemibold,
    NSForegroundColorAttributeName, NSImage, NSKernAttributeName, NSLinkAttributeName,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStrikethroughStyleAttributeName,
    NSTextAlignment, NSTextAttachment, NSTextBlock, NSTextBlockLayer, NSTextBlockValueType,
    NSTextTab, NSTextTable, NSTextTableBlock, NSTextTableLayoutAlgorithm, NSUnderlineStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSData, NSDictionary, NSMutableAttributedString, NSNumber, NSPoint,
    NSRange, NSRect, NSRectEdge, NSSize, NSString, NSURL,
};
use pulldown_cmark::Alignment;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, FontStyle, Theme};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

use crate::chat::{DetailSection, ToolStatus, TranscriptMessage, TranscriptMessageKind};
use crate::chat_typography::{TextRole, TextSizes};
use crate::markdown::{
    InlineStyle, MarkdownDocument, MarkdownImage, Paragraph, ParagraphKind, TableCell, TextRange, TextTone, ends_in_open_fence, link_definitions, parse, split_blocks,
};
use crate::perf;
use crate::theme::{self, ChatStyle};
use upleft_render::fragments::fragment_base::RemoteImage;
use upleft_render::theme::style_sheet::{BodyFamily, CodeLook, HostTypography};

/// How the transcript is set — the style chosen in Settings and the text
/// size — resolved into families, sizes, and rhythm. One value serves a
/// whole render, so every run agrees with every other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Typography {
    pub(crate) style: ChatStyle,
    pub(crate) scale: f64,
    sizes: TextSizes,
    body_role: TextRole,
}

/// A family: the system sans, the system serif (New York), or SF Mono.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Design {
    Sans,
    Serif,
    Mono,
}

impl Typography {
    pub(crate) const fn new(style: ChatStyle, scale: f64) -> Self {
        Self {
            style,
            scale,
            sizes: TextSizes::DEFAULTS,
            body_role: TextRole::Assistant,
        }
    }

    /// Where lines wrap, in points — roughly eighty characters of the body
    /// face, the width a reader's eye returns from without losing the line.
    pub(crate) fn measure(&self) -> f64 {
        let base = match self.style {
            ChatStyle::Modern => theme::READING_MEASURE,
            ChatStyle::Editorial => 600.0,
            ChatStyle::Terminal => 720.0,
        };
        base * self.scale
    }

    pub(crate) fn with_sizes(mut self, sizes: TextSizes) -> Self {
        self.sizes = sizes;
        self
    }

    pub(crate) fn size(&self, role: TextRole) -> f64 {
        self.sizes.resolve(role, self.style, self.scale)
    }

    /// The composer and header always use the native system face, independently
    /// of the transcript's style. Shared with Settings' live native examples.
    pub(crate) fn interface_font(&self, role: TextRole) -> Retained<NSFont> {
        // SAFETY: AppKit exports both weight constants for the process lifetime.
        let weight = unsafe {
            if role == TextRole::Title {
                NSFontWeightSemibold
            } else {
                NSFontWeightRegular
            }
        };
        NSFont::systemFontOfSize_weight(self.size(role), weight)
    }

    fn body_size(&self) -> f64 {
        self.size(self.body_role)
    }

    fn code_size(&self) -> f64 {
        self.size(TextRole::CodeBlock)
    }

    fn body_rhythm(&self) -> f64 {
        self.body_size() / self.body_role.default_size(self.style, self.scale)
    }

    /// Extra leading between lines; the serif page breathes the most.
    fn line_spacing(&self) -> f64 {
        let base = match self.style {
            ChatStyle::Modern => 6.0,
            ChatStyle::Editorial => 6.0,
            ChatStyle::Terminal => 4.0,
        };
        base * self.scale * self.body_rhythm()
    }

    fn paragraph_spacing(&self) -> f64 {
        let base = match self.style {
            ChatStyle::Modern => 13.0,
            ChatStyle::Editorial => 11.0,
            ChatStyle::Terminal => 9.0,
        };
        base * self.scale * self.body_rhythm()
    }

    /// The serif page hyphenates its rag, as a book would.
    fn hyphenates(&self) -> bool {
        self.style == ChatStyle::Editorial
    }

    fn body_design(&self) -> Design {
        match self.style {
            ChatStyle::Modern => Design::Sans,
            ChatStyle::Editorial => Design::Serif,
            ChatStyle::Terminal => Design::Mono,
        }
    }

    /// The face for labels and rows around the prose.
    fn ui_design(&self) -> Design {
        match self.style {
            ChatStyle::Terminal => Design::Mono,
            _ => Design::Sans,
        }
    }

    /// A chat column is narrow and headings recur often, so the scale is
    /// tighter than a document's: emphasis, not billboards. Monospace
    /// headings stay close to the text size, as they do in a terminal.
    fn heading_size(&self, level: u8) -> f64 {
        self.size(TextRole::heading(level))
    }

    /// The face for the labels and rows around the prose, at `size`.
    pub(crate) fn ui_font(&self, size: f64, weight: NSFontWeight) -> Retained<NSFont> {
        design_font(self.ui_design(), size, weight)
    }

    fn italic_font(&self, size: f64) -> Retained<NSFont> {
        // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
        let base = self.ui_font(size, unsafe { NSFontWeightRegular });
        with_traits(base, size, false, true)
    }

    /// The same type for the Markdown renderer's hosted views: `role`'s body
    /// face and size, the heading and code sizes, the leading (the face's own
    /// line plus the extra spacing, as a multiple of the size), the space
    /// after a paragraph, and the serif page's hyphenated rag.
    pub(crate) fn host_typography(&self, role: TextRole) -> HostTypography {
        let typography = Typography {
            body_role: role,
            ..*self
        };
        let body = typography.body_size();
        // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
        let font = design_font(typography.body_design(), body, unsafe { NSFontWeightRegular });
        let natural = font.ascender() - font.descender() + font.leading();
        HostTypography {
            body_family: Some(match typography.body_design() {
                Design::Sans => BodyFamily::System,
                Design::Serif => BodyFamily::NewYork,
                Design::Mono => BodyFamily::Monospaced,
            }),
            body_size: Some(body),
            heading_sizes: [1, 2, 3, 4, 5, 6].map(|level| Some(typography.heading_size(level))),
            code_size: Some(typography.code_size()),
            line_height_multiple: Some((natural + typography.line_spacing()) / body),
            paragraph_spacing: Some(typography.paragraph_spacing()),
            hyphenation_factor: typography.hyphenates().then_some(0.9),
            code_bleed: Some(0.0),
            // Code blocks wear a header bar: language pill, copy always there.
            code_header: Some(true),
            // Callouts are cards in the palette's colours (transcript_style).
            callout_card: Some(true),
            // Each chat style dresses the code header its own way; blocks of
            // three lines or more are numbered.
            code_look: Some(match typography.body_design() {
                Design::Sans => CodeLook::Card,
                Design::Serif => CodeLook::Ledger,
                Design::Mono => CodeLook::Console,
            }),
            code_line_numbers: Some(3),
            // `\(…\)` and `\[…\]` inline math is typeset from its content;
            // Downright hands SwiftMath the delimiters too, and it renders
            // nothing.
            inline_math_content: Some(true),
            // Models write `\bigl(` and `\lvert`, which SwiftMath lacks;
            // without the rewrite such a formula shows as source.
            math_dialect: Some(true),
        }
    }

    /// The speaker label's face: the interface face, semibold.
    pub(crate) fn speaker_font(&self) -> Retained<NSFont> {
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        self.ui_font(self.size(TextRole::Speaker), unsafe { NSFontWeightSemibold })
    }

    /// The face of the blank line after a message, which spaces it from the
    /// next one.
    pub(crate) fn spacer_font(&self) -> Retained<NSFont> {
        // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
        self.ui_font(13.0 * self.scale, unsafe { NSFontWeightRegular })
    }
}

fn design_font(design: Design, size: f64, weight: NSFontWeight) -> Retained<NSFont> {
    match design {
        Design::Sans => NSFont::systemFontOfSize_weight(size, weight),
        Design::Serif => theme::display_font(size, weight),
        Design::Mono => NSFont::monospacedSystemFontOfSize_weight(size, weight),
    }
}

/// Adds bold and italic through the descriptor, which works under TextKit 2
/// (unlike the deprecated obliqueness attribute) and keeps the family.
fn with_traits(base: Retained<NSFont>, size: f64, bold: bool, italic: bool) -> Retained<NSFont> {
    if !bold && !italic {
        return base;
    }
    let mut traits = base.fontDescriptor().symbolicTraits();
    if bold {
        traits.insert(NSFontDescriptorSymbolicTraits::TraitBold);
    }
    if italic {
        traits.insert(NSFontDescriptorSymbolicTraits::TraitItalic);
    }
    let descriptor = base
        .fontDescriptor()
        .fontDescriptorWithSymbolicTraits(traits);
    NSFont::fontWithDescriptor_size(&descriptor, size).unwrap_or(base)
}

const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FontKey {
    tenths: u16,
    bold: bool,
    italic: bool,
    semibold: bool,
    design: Design,
}

#[derive(Debug)]
enum ImageState {
    Loading,
    Loaded(Arc<[u8]>),
    Failed,
}

#[derive(Debug)]
struct ImageResult {
    destination: String,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug)]
struct ImageLoader {
    states: HashMap<String, ImageState>,
    tasks: flume::Sender<String>,
    results: flume::Receiver<ImageResult>,
}

impl ImageLoader {
    fn new() -> Self {
        let (tasks, task_receiver) = flume::unbounded::<String>();
        let (result_sender, results) = flume::unbounded();
        for worker in 0..2 {
            let task_receiver = task_receiver.clone();
            let result_sender = result_sender.clone();
            thread::Builder::new()
                .name(format!("omperor-image-loader-{worker}"))
                .spawn(move || image_worker(task_receiver, result_sender))
                .expect("native image loader worker must start");
        }
        Self {
            states: HashMap::new(),
            tasks,
            results,
        }
    }
}

static IMAGE_LOADER: OnceLock<Mutex<ImageLoader>> = OnceLock::new();
static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<EmbeddedLazyThemeSet> = OnceLock::new();

/// The images that finished loading (or failing) since the last call, by
/// destination.
pub(crate) fn poll_image_loads() -> Vec<String> {
    let mut landed = Vec::new();
    let Ok(mut loader) = image_loader().lock() else {
        return landed;
    };
    while let Ok(result) = loader.results.try_recv() {
        let state = result.bytes.map_or(ImageState::Failed, |bytes| {
            ImageState::Loaded(Arc::from(bytes))
        });
        loader.states.insert(result.destination.clone(), state);
        landed.push(result.destination);
    }
    landed
}

thread_local! {
    /// Remote images decoded for the transcript, by destination.
    static REMOTE_IMAGES: std::cell::RefCell<HashMap<String, Retained<NSImage>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// A remote (`http`, `https`) image for the transcript's hosted views: the
/// same loader workers as ever fetch it, and the transcript hears through
/// `poll_image_loads` when it lands.
pub(crate) fn remote_image(destination: &str) -> RemoteImage {
    if let Some(image) = REMOTE_IMAGES.with(|images| images.borrow().get(destination).cloned()) {
        return RemoteImage::Loaded(image);
    }
    if is_remote(destination) && !REMOTE_IMAGES_ALLOWED.load(std::sync::atomic::Ordering::Relaxed) {
        return RemoteImage::Failed;
    }
    if let Some(bytes) = request_image(destination) {
        let data = NSData::with_bytes(&bytes);
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data).filter(|image| image.size().width > 0.0) else {
            return RemoteImage::Failed;
        };
        REMOTE_IMAGES.with(|images| {
            let mut images = images.borrow_mut();
            // A bounded working set: the transcript shows a handful at once.
            if images.len() >= 64 {
                images.clear();
            }
            images.insert(destination.to_owned(), image.clone());
        });
        return RemoteImage::Loaded(image);
    }
    let failed = image_loader()
        .lock()
        .is_ok_and(|loader| matches!(loader.states.get(destination), Some(ImageState::Failed)));
    if failed { RemoteImage::Failed } else { RemoteImage::Loading }
}

/// Custom URL scheme carried by a detail row's clickable affordance; the chat
/// text view's delegate intercepts it to open the full payload in a modal.
pub(crate) const DETAIL_URL_SCHEME: &str = "omperor-detail";

/// How a message was set: the blocks it was rendered from, each remembered
/// so the next render of the same message reuses every block whose source
/// and context are unchanged. A streaming message then costs its open block
/// per refresh, not its length.
#[derive(Debug)]
pub(crate) struct RenderedMessage {
    blocks: Vec<RenderedBlock>,
    settings: RenderSettings,
}

#[derive(Debug)]
struct RenderedBlock {
    key: BlockKey,
    attributed: Retained<NSMutableAttributedString>,
    /// The UTF-16 length of `attributed`.
    length: usize,
    /// The last paragraph set, which spaces the block after it.
    last: Option<Paragraph>,
}

impl RenderedBlock {
    fn share(&self) -> Self {
        Self {
            key: self.key.share(),
            attributed: self.attributed.clone(),
            length: self.length,
            last: self.last,
        }
    }
}

#[derive(Debug, PartialEq)]
enum BlockKey {
    /// The speaker line above a chat message.
    Speaker(String),
    /// One markdown block: its source, the paragraph before it, and the
    /// link definitions it was parsed with.
    Markdown {
        source: Rc<str>,
        previous: Option<Paragraph>,
        definitions: u64,
    },
    /// The spacer after a message.
    Spacer,
    /// A message set whole, never reused across renders.
    Whole,
}

impl BlockKey {
    fn share(&self) -> Self {
        match self {
            Self::Speaker(name) => Self::Speaker(name.clone()),
            Self::Markdown { source, previous, definitions } => Self::Markdown {
                source: Rc::clone(source),
                previous: *previous,
                definitions: *definitions,
            },
            Self::Spacer => Self::Spacer,
            Self::Whole => Self::Whole,
        }
    }
}

/// What a render depends on besides the message; a change to any of these
/// makes every remembered block stale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RenderSettings {
    typography: Typography,
    content_width: f64,
    show_inline_images: bool,
}

/// A render of one message, with how much of its start the previous render
/// already had — the part the transcript need not touch. The chat transcript
/// no longer sets messages here (`transcript_stack`); the block reuse stays
/// tested until the remaining callers move to the new renderer too.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct MessageRender {
    pub(crate) rendered: RenderedMessage,
    /// The leading UTF-16 units identical to the previous render's.
    pub(crate) unchanged_prefix: usize,
    unchanged_blocks: usize,
}

#[cfg_attr(not(test), allow(dead_code))]
impl MessageRender {
    /// Everything after the unchanged prefix.
    pub(crate) fn changed_tail(&self) -> Retained<NSMutableAttributedString> {
        self.rendered.tail(self.unchanged_blocks)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl RenderedMessage {
    /// The UTF-16 length of the whole message.
    pub(crate) fn length(&self) -> usize {
        self.blocks.iter().map(|block| block.length).sum()
    }

    pub(crate) fn attributed(&self) -> Retained<NSMutableAttributedString> {
        self.tail(0)
    }

    fn tail(&self, from: usize) -> Retained<NSMutableAttributedString> {
        let joined =
            NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &empty());
        for block in self.blocks.iter().skip(from) {
            joined.appendAttributedString(&block.attributed);
        }
        joined
    }
}

pub(crate) fn render_message(
    message: &TranscriptMessage,
    content_width: f64,
    image_base: &Path,
    typography: Typography,
    show_inline_images: bool,
) -> Retained<NSMutableAttributedString> {
    render_message_incrementally(
        message,
        None,
        content_width,
        image_base,
        typography,
        show_inline_images,
    )
    .rendered
    .attributed()
}

/// Sets `message`, reusing from `previous` — the same message as last
/// rendered — every block that is unchanged. Only an assistant message is
/// set by block: it is the one that grows while it is read.
pub(crate) fn render_message_incrementally(
    message: &TranscriptMessage,
    previous: Option<&RenderedMessage>,
    content_width: f64,
    image_base: &Path,
    typography: Typography,
    show_inline_images: bool,
) -> MessageRender {
    let _span = perf::span_with("gui.markdown.render_message", || {
        serde_json::json!({
            "source_bytes": message.body.len(),
            "content_width": content_width,
            "kind": format!("{:?}", message.kind),
            "style": typography.style.label(),
        })
    });
    let settings = RenderSettings {
        typography,
        content_width,
        show_inline_images,
    };
    let previous = previous.filter(|rendered| rendered.settings == settings);
    let typography = Typography {
        body_role: if message.kind == TranscriptMessageKind::User {
            TextRole::User
        } else {
            TextRole::Assistant
        },
        ..typography
    };
    let mut blocks = Vec::new();
    if message.kind == TranscriptMessageKind::Assistant {
        if !message.author.is_empty() {
            blocks.push(speaker_block(message, typography));
        }
        markdown_blocks(
            &message.body,
            previous,
            settings,
            typography,
            image_base,
            &mut blocks,
        );
        blocks.push(spacer_block(typography, "\n"));
    } else {
        let attributed = render_whole(message, settings, typography, image_base);
        let length = attributed.length();
        blocks.push(RenderedBlock {
            key: BlockKey::Whole,
            attributed,
            length,
            last: None,
        });
    }
    let mut unchanged_blocks = 0;
    let mut unchanged_prefix = 0;
    if let Some(previous) = previous {
        for (new, old) in blocks.iter().zip(&previous.blocks) {
            if new.key == BlockKey::Whole || new.key != old.key {
                break;
            }
            unchanged_blocks += 1;
            unchanged_prefix += new.length;
        }
    }
    MessageRender {
        rendered: RenderedMessage { blocks, settings },
        unchanged_prefix,
        unchanged_blocks,
    }
}

/// The body's blocks, in order, each taken from `previous` when its source
/// still starts at the same place in the same context, rendered otherwise.
fn markdown_blocks(
    body: &str,
    previous: Option<&RenderedMessage>,
    settings: RenderSettings,
    typography: Typography,
    image_base: &Path,
    blocks: &mut Vec<RenderedBlock>,
) {
    let definitions = link_definitions(body);
    let definitions_hash = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        definitions.hash(&mut hasher);
        hasher.finish()
    };
    let suffix = if definitions.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", definitions.join("\n"))
    };
    let old = previous
        .map(|rendered| {
            rendered
                .blocks
                .iter()
                .filter(|block| matches!(block.key, BlockKey::Markdown { .. }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // Keep the leading blocks whose source still starts the body, in the
    // same context, with the same definitions.
    let mut offset = 0;
    let mut context: Option<Paragraph> = None;
    let mut kept = 0;
    for block in &old {
        let BlockKey::Markdown {
            source,
            previous,
            definitions,
        } = &block.key
        else {
            break;
        };
        if *definitions == definitions_hash
            && *previous == context
            && body[offset..].starts_with(&**source)
        {
            offset += source.len();
            context = block.last;
            kept += 1;
        } else {
            break;
        }
    }
    // The cut after the last kept block was decided by the line that
    // followed it, which may since have changed: give that block back so
    // the split decides again.
    if kept > 0 {
        kept -= 1;
        if let BlockKey::Markdown {
            source, previous, ..
        } = &old[kept].key
        {
            offset -= source.len();
            context = *previous;
        }
    }
    blocks.extend(old[..kept].iter().map(|block| block.share()));
    for (index, range) in split_blocks(&body[offset..]).into_iter().enumerate() {
        let source = &body[offset + range.start..offset + range.end];
        let key = BlockKey::Markdown {
            source: Rc::from(source),
            previous: context,
            definitions: definitions_hash,
        };
        let block = match old.get(kept + index) {
            Some(old) if old.key == key => old.share(),
            _ => render_markdown_block(source, &suffix, context, key, settings, typography, image_base),
        };
        context = block.last;
        blocks.push(block);
    }
}

fn render_markdown_block(
    source: &str,
    definitions: &str,
    previous: Option<Paragraph>,
    key: BlockKey,
    settings: RenderSettings,
    typography: Typography,
    image_base: &Path,
) -> RenderedBlock {
    // Definitions ride along so a link in this block resolves; not into an
    // open fence, where they would read as code.
    let input: Cow<'_, str> = if definitions.is_empty() || ends_in_open_fence(source) {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(format!("{source}{definitions}"))
    };
    let document = {
        let _span = perf::span("gui.markdown.parse");
        parse(&input)
    };
    let attributed = render_document(
        &document,
        typography,
        settings.content_width,
        image_base,
        settings.show_inline_images,
        None,
        previous,
    );
    let length = attributed.length();
    RenderedBlock {
        key,
        attributed,
        length,
        last: document.paragraphs.last().copied().or(previous),
    }
}

fn speaker_block(message: &TranscriptMessage, typography: Typography) -> RenderedBlock {
    let rendered =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &empty());
    let name = if message.author == "omp" {
        "Omperor"
    } else {
        &message.author
    };
    // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
    let font = typography.ui_font(typography.size(TextRole::Speaker), unsafe {
        NSFontWeightSemibold
    });
    append_label_line(&rendered, name, &font, &theme::secondary_text());
    let length = rendered.length();
    RenderedBlock {
        key: BlockKey::Speaker(name.to_owned()),
        attributed: rendered,
        length,
        last: None,
    }
}

fn spacer_block(typography: Typography, trailing: &str) -> RenderedBlock {
    let rendered =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &empty());
    // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
    let font = typography.ui_font(13.0 * typography.scale, unsafe { NSFontWeightRegular });
    append_run(&rendered, trailing, &font, &NSColor::labelColor(), None);
    let length = rendered.length();
    RenderedBlock {
        key: BlockKey::Spacer,
        attributed: rendered,
        length,
        last: None,
    }
}

/// A message set in one piece: everything but an assistant's turn.
fn render_whole(
    message: &TranscriptMessage,
    settings: RenderSettings,
    typography: Typography,
    image_base: &Path,
) -> Retained<NSMutableAttributedString> {
    let text_scale = typography.scale;
    let rendered =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &empty());

    // Tool rows read as a single self-contained line — a status glyph, the
    // action, and a chevron to expand — like a task-list entry, so they carry
    // no speaker prefix. Spoken messages carry a sentence-case label; activity
    // already identifies itself, without a redundant author heading.
    if matches!(
        message.kind,
        TranscriptMessageKind::User | TranscriptMessageKind::Assistant
    ) && !message.author.is_empty()
    {
        let author_color = match message.kind {
            TranscriptMessageKind::User => theme::accent(),
            TranscriptMessageKind::Assistant | TranscriptMessageKind::Notice => {
                theme::secondary_text()
            }
            TranscriptMessageKind::Thinking
            | TranscriptMessageKind::Event
            | TranscriptMessageKind::Tool
            | TranscriptMessageKind::Usage => theme::secondary_text(),
        };
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        let author_font = typography.ui_font(typography.size(TextRole::Speaker), unsafe {
            NSFontWeightSemibold
        });
        append_label_line(
            &rendered,
            if message.author == "omp" {
                "Omperor"
            } else {
                &message.author
            },
            &author_font,
            &author_color,
        );
    }

    match message.kind {
        TranscriptMessageKind::User | TranscriptMessageKind::Assistant => {
            let document = {
                let _span = perf::span("gui.markdown.parse");
                parse(&message.body)
            };
            // The user's turn sits on a lifted surface with a fine rule
            // down its left edge, so the conversation's two voices are told
            // apart at a glance; the assistant's prose runs on the page.
            let enclosure = (message.kind == TranscriptMessageKind::User).then(user_message_block);
            let body = render_document(
                &document,
                typography,
                settings.content_width,
                image_base,
                settings.show_inline_images,
                enclosure.as_ref(),
                None,
            );
            rendered.appendAttributedString(&body);
        }
        TranscriptMessageKind::Thinking => {
            // Chain-of-thought reads as quiet, italic prose behind a plum
            // spark, set in from a plum margin rule (drawn by the activity
            // view, `CHIP_KEY`), like a note in the book's margin.
            let start = rendered.length();
            let font = typography.italic_font(typography.size(TextRole::Thinking));
            let plain = parse(message.body.trim()).text;
            let text = plain.trim();
            let text = if text.is_empty() { "…" } else { text };
            let color = theme::secondary_text();
            append_symbol(
                &rendered,
                "sparkle",
                "✦",
                typography.size(TextRole::Thinking) * 0.73,
                &theme::plum(),
                None,
            );
            append_run(&rendered, "  ", &font, &color, None);
            // Folded to its title: the thought's first line, and how much
            // more there is; the chevron opens the whole of it.
            let (title, rest) = fold_thought(text);
            append_run(&rendered, title, &font, &color, None);
            if rest > 0 {
                // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
                let quiet = typography.ui_font(typography.size(TextRole::Thinking) * 0.92, unsafe { NSFontWeightRegular });
                let words = if rest == 1 { "1 more word".to_owned() } else { format!("{rest} more words") };
                append_run(&rendered, &format!("  ·  {words}"), &quiet, &theme::secondary_text(), None);
            }
            append_detail_chevron(&rendered, message, typography);
            mark_chip(&rendered, start, &chip_kind(typography, "thinking"), THINKING_INDENT);
        }
        TranscriptMessageKind::Tool | TranscriptMessageKind::Event => {
            append_activity_row(&rendered, message, typography);
        }
        // Drawn by its own view (`usage_card`); set as text only by name.
        TranscriptMessageKind::Usage => {
            // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
            let font = typography.ui_font(typography.size(TextRole::Notice), unsafe { NSFontWeightRegular });
            append_label_line(&rendered, "Usage report", &font, &theme::secondary_text());
        }
        TranscriptMessageKind::Notice => {
            // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
            let font = typography.ui_font(typography.size(TextRole::Notice), unsafe {
                NSFontWeightRegular
            });
            let (symbol, fallback, color) = if message.status == Some(ToolStatus::Failed) {
                (
                    "exclamationmark.triangle.fill",
                    "!",
                    NSColor::systemOrangeColor(),
                )
            } else {
                ("info.circle", "i", theme::secondary_text())
            };
            append_symbol(
                &rendered,
                symbol,
                fallback,
                typography.size(TextRole::Notice) * 0.88,
                &color,
                None,
            );
            append_run(&rendered, "  ", &font, &color, None);
            append_run(&rendered, message.body.trim(), &font, &color, None);
        }
    }

    // Chat bodies already end in a blank line; activity rows need one added so
    // the transcript keeps an even vertical rhythm.
    let trailing = match message.kind {
        TranscriptMessageKind::User | TranscriptMessageKind::Assistant => "\n",
        _ => "\n\n",
    };
    // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
    let spacer_font = typography.ui_font(13.0 * text_scale, unsafe { NSFontWeightRegular });
    append_run(
        &rendered,
        trailing,
        &spacer_font,
        &NSColor::labelColor(),
        None,
    );
    rendered
}

fn empty() -> Retained<NSString> {
    NSString::from_str("")
}

/// The block a user message sits in: a neutral surface, a fine rule on the left,
/// and room to breathe. Every paragraph of the message shares this one
/// object, so TextKit draws a single box around the whole turn.
fn user_message_block() -> Retained<NSTextBlock> {
    let block = NSTextBlock::new();
    block.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
    block.setBackgroundColor(Some(&theme::surface()));
    block.setBorderColor(Some(&theme::hairline()));
    block.setWidth_type_forLayer_edge(
        1.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Border,
        NSRectEdge::MinX,
    );
    for edge in [NSRectEdge::MinY, NSRectEdge::MaxY] {
        block.setWidth_type_forLayer_edge(
            9.0,
            NSTextBlockValueType::AbsoluteValueType,
            NSTextBlockLayer::Padding,
            edge,
        );
    }
    block.setWidth_type_forLayer_edge(
        14.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Padding,
        NSRectEdge::MinX,
    );
    block.setWidth_type_forLayer_edge(
        12.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Padding,
        NSRectEdge::MaxX,
    );
    block.setWidth_type_forLayer_edge(
        4.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Margin,
        NSRectEdge::MaxY,
    );
    block
}

/// A sentence-case speaker label with breathing room above the message.
fn append_label_line(
    target: &NSMutableAttributedString,
    text: &str,
    font: &NSFont,
    color: &NSColor,
) {
    let string = NSString::from_str(&format!("{text}\n"));
    let run =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &string);
    let range = NSRange::new(0, string.length());
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setParagraphSpacing(8.0);
    // SAFETY: the attribute names are paired with the value types AppKit
    // documents for them, over exactly the new run.
    unsafe {
        run.addAttribute_value_range(NSFontAttributeName, font, range);
        run.addAttribute_value_range(NSForegroundColorAttributeName, color, range);
        run.addAttribute_value_range(NSParagraphStyleAttributeName, &paragraph, range);
    }
    target.appendAttributedString(&run);
}

/// Appends an SF Symbol as an inline attachment, tinted, sitting on the text
/// baseline; falls back to `fallback` where the symbol is unavailable.
fn append_symbol(
    target: &NSMutableAttributedString,
    name: &str,
    fallback: &str,
    point_size: f64,
    color: &NSColor,
    link: Option<&str>,
) {
    // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
    let weight = unsafe { NSFontWeightSemibold };
    let Some(image) = theme::symbol(name, point_size, weight, Some(color)) else {
        append_run(
            target,
            fallback,
            &NSFont::systemFontOfSize(point_size),
            color,
            link,
        );
        return;
    };
    let attachment = NSTextAttachment::new();
    attachment.setImage(Some(&image));
    let size = image.size();
    // Symbols are drawn from the baseline; dropping them by a fifth of their
    // height centers them on the lowercase text beside them.
    attachment.setBounds(NSRect::new(NSPoint::new(0.0, -(size.height * 0.2)), size));
    let string = NSString::from_str("\u{fffc}");
    let run =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &string);
    let range = NSRange::new(0, 1);
    // SAFETY: NSTextAttachment is the documented value for the attachment
    // attribute; the object-replacement character is one UTF-16 unit.
    unsafe {
        run.addAttribute_value_range(NSAttachmentAttributeName, &attachment, range);
        if let Some(link) = link
            && let Some(url) = NSURL::URLWithString(&NSString::from_str(link))
        {
            run.addAttribute_value_range(NSLinkAttributeName, &url, range);
        }
    }
    target.appendAttributedString(&run);
}

/// Renders a detail modal's sections as an attributed document: quiet
/// headings over prose (the model's intent, its reasoning) or
/// syntax-highlighted, monospaced payloads (arguments, results).
pub(crate) fn render_detail(
    sections: &[DetailSection],
    text_scale: f64,
) -> Retained<NSMutableAttributedString> {
    let rendered =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &empty());
    let mut fonts = HashMap::<FontKey, Retained<NSFont>>::new();
    for (index, section) in sections.iter().enumerate() {
        let body = section.body.trim_matches('\n');
        if section.heading.is_none() && body.is_empty() {
            continue;
        }
        if let Some(heading) = &section.heading {
            if index > 0 {
                append_run(
                    &rendered,
                    "\n\n",
                    &NSFont::systemFontOfSize(6.0 * text_scale),
                    &NSColor::labelColor(),
                    None,
                );
            }
            // SAFETY: AppKit exports NSFontWeightSemibold for the process life.
            let font =
                unsafe { NSFont::systemFontOfSize_weight(12.0 * text_scale, NSFontWeightSemibold) };
            append_heading(&rendered, heading, &font);
            append_run(
                &rendered,
                "\n",
                &NSFont::systemFontOfSize(5.0 * text_scale),
                &NSColor::labelColor(),
                None,
            );
        }
        if body.is_empty() {
            continue;
        }
        if section.code {
            append_code_block(&rendered, body, 13.5 * text_scale, &mut fonts);
        } else {
            let font = NSFont::systemFontOfSize(16.0 * text_scale);
            append_run(&rendered, body, &font, &NSColor::labelColor(), None);
        }
    }
    rendered
}

/// Appends a section heading in the shared secondary color.
fn append_heading(target: &NSMutableAttributedString, text: &str, font: &NSFont) {
    let string = NSString::from_str(text);
    let run =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &string);
    let range = NSRange::new(0, string.length());
    // SAFETY: the attribute names are paired with the value types AppKit
    // documents for them, over exactly the new run.
    unsafe {
        run.addAttribute_value_range(NSFontAttributeName, font, range);
        run.addAttribute_value_range(
            NSForegroundColorAttributeName,
            &theme::secondary_text(),
            range,
        );
    }
    target.appendAttributedString(&run);
}

/// Appends a monospaced payload and syntax-highlights it as JSON over its own
/// range, reusing the transcript's code-block highlighter and theme.
fn append_code_block(
    target: &NSMutableAttributedString,
    code: &str,
    size: f64,
    fonts: &mut HashMap<FontKey, Retained<NSFont>>,
) {
    let start = target.length();
    // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
    let base = unsafe { NSFont::monospacedSystemFontOfSize_weight(size, NSFontWeightRegular) };
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setLineHeightMultiple(1.15);
    let string = NSString::from_str(code);
    let run =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &string);
    let full = NSRange::new(0, string.length());
    // SAFETY: attribute names paired with their documented value types, over
    // the run's full range.
    unsafe {
        run.addAttribute_value_range(NSFontAttributeName, &base, full);
        run.addAttribute_value_range(NSForegroundColorAttributeName, &NSColor::labelColor(), full);
        run.addAttribute_value_range(NSParagraphStyleAttributeName, &paragraph, full);
    }
    target.appendAttributedString(&run);

    let syntaxes = SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines);
    let syntax = syntax_for(syntaxes, "json");
    let theme = syntax_theme();
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut location = start;
    for line in LinesWithEndings::from(code) {
        apply_highlighted_line(
            target,
            line,
            &mut highlighter,
            syntaxes,
            location,
            size,
            fonts,
        );
        location += line.encode_utf16().count();
    }
}

fn status_color(status: ToolStatus) -> Retained<NSColor> {
    match status {
        ToolStatus::Running => theme::accent(),
        ToolStatus::Completed => theme::success(),
        ToolStatus::Failed => theme::danger(),
    }
}

/// A thought folded to its title: its first line, and the words after it.
/// A thought of one line is its own title.
pub(crate) fn fold_thought(text: &str) -> (&str, usize) {
    let text = text.trim();
    match text.split_once('\n') {
        Some((title, rest)) if !title.trim().is_empty() => (title.trim(), rest.split_whitespace().count()),
        _ => (text, 0),
    }
}

/// The attribute the activity view reads to draw a row's chip or margin
/// rule behind its text: "running", "done", "failed", "event", "thinking".
pub(crate) const CHIP_KEY: &str = "OmperorChip";
/// A chip's text is set in by this much on each side, so its pill has room.
pub(crate) const CHIP_INDENT: f64 = 12.0;
/// Thinking's text is set in from its margin rule by this much.
pub(crate) const THINKING_INDENT: f64 = 16.0;

/// Marks `target` from `start` to its end as one chip of `kind`, and sets
/// the row in so the pill (or rule) the activity view draws has room.
fn mark_chip(target: &NSMutableAttributedString, start: usize, kind: &str, indent: f64) {
    let end = target.length();
    if end <= start {
        return;
    }
    let range = NSRange::new(start, end - start);
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setFirstLineHeadIndent(indent);
    paragraph.setHeadIndent(indent);
    paragraph.setTailIndent(-indent);
    // SAFETY: the paragraph style and an NSString are the documented value
    // types for these attributes; the range lies inside the string.
    unsafe {
        target.addAttribute_value_range(NSParagraphStyleAttributeName, &paragraph, range);
        target.addAttribute_value_range(&NSString::from_str(CHIP_KEY), &NSString::from_str(kind), range);
    }
}

/// A compact status mark as an SF Symbol (with a text fallback): a dotted
/// ring while running, a filled check when done, a filled cross when failed.
fn status_symbol(status: ToolStatus) -> (&'static str, &'static str) {
    match status {
        ToolStatus::Running => ("circle.dotted", "●"),
        ToolStatus::Completed => ("checkmark.circle.fill", "✓"),
        ToolStatus::Failed => ("xmark.circle.fill", "✗"),
    }
}

/// Renders a tool or event row as one clickable line — status mark, action
/// summary, and an expand chevron — with the detail link spanning the whole
/// line so a click anywhere opens the detail.
fn append_activity_row(
    target: &NSMutableAttributedString,
    message: &TranscriptMessage,
    typography: Typography,
) {
    let text_scale = typography.size(TextRole::Activity) / 13.0;
    // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
    let regular = unsafe { NSFontWeightRegular };
    let url = message
        .detail_id
        .map(|id| format!("{DETAIL_URL_SCHEME}:{id}"));
    let link = url.as_deref();
    let start = target.length();

    if let Some(status) = message.status {
        let (symbol, fallback) = status_symbol(status);
        append_symbol(
            target,
            symbol,
            fallback,
            11.0 * text_scale,
            &status_color(status),
            link,
        );
        append_run(
            target,
            "  ",
            &typography.ui_font(12.0 * text_scale, regular),
            &theme::secondary_text(),
            link,
        );
    }

    let summary = message.body.trim();
    let summary = if summary.is_empty() {
        message.author.as_str()
    } else {
        summary
    };
    let font = typography.ui_font(13.0 * text_scale, regular);
    let color = if message.status == Some(ToolStatus::Failed) { theme::danger() } else { theme::secondary_text() };
    append_run(target, summary, &font, &color, link);

    if link.is_some() {
        append_chevron(target, typography, link);
    }
    let kind = match message.status {
        Some(ToolStatus::Running) => "running",
        Some(ToolStatus::Completed) => "done",
        Some(ToolStatus::Failed) => "failed",
        None => "event",
    };
    mark_chip(target, start, &chip_kind(typography, kind), CHIP_INDENT);
}

/// A chip's kind as the activity view reads it: Modern's plain, the other
/// styles' prefixed with their name ("terminal:done").
fn chip_kind(typography: Typography, kind: &str) -> String {
    match typography.style {
        ChatStyle::Modern => kind.to_owned(),
        style => format!("{}:{kind}", style.stored()),
    }
}

/// Appends a trailing chevron linking to the row's full payload, or nothing
/// when the row has no expandable detail.
fn append_detail_chevron(
    target: &NSMutableAttributedString,
    message: &TranscriptMessage,
    typography: Typography,
) {
    let Some(id) = message.detail_id else {
        return;
    };
    let url = format!("{DETAIL_URL_SCHEME}:{id}");
    append_chevron(target, typography, Some(&url));
}

/// A small tertiary chevron after a row: the affordance that it opens.
fn append_chevron(target: &NSMutableAttributedString, typography: Typography, link: Option<&str>) {
    let color = theme::secondary_text();
    // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
    let font = typography.ui_font(typography.size(TextRole::Activity) * 12.0 / 13.0, unsafe {
        NSFontWeightRegular
    });
    append_run(target, "  ", &font, &color, link);
    append_symbol(
        target,
        "chevron.right",
        "›",
        typography.size(TextRole::Activity) * 8.5 / 13.0,
        &color,
        link,
    );
}

/// Builds an attributed run and appends it, applying font, color, and an
/// optional link URL.
fn append_run(
    target: &NSMutableAttributedString,
    text: &str,
    font: &NSFont,
    color: &NSColor,
    link: Option<&str>,
) {
    let string = NSString::from_str(text);
    let run =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &string);
    let range = NSRange::new(0, string.length());
    // SAFETY: every attribute name is paired with the object type AppKit
    // documents for it, and the range spans exactly the new run.
    unsafe {
        run.addAttribute_value_range(NSFontAttributeName, font, range);
        run.addAttribute_value_range(NSForegroundColorAttributeName, color, range);
        if let Some(link) = link
            && let Some(url) = NSURL::URLWithString(&NSString::from_str(link))
        {
            run.addAttribute_value_range(NSLinkAttributeName, &url, range);
        }
    }
    target.appendAttributedString(&run);
}

pub(crate) fn render_diff(source: &str, file_path: &Path) -> Retained<NSMutableAttributedString> {
    let _span = perf::span_with(
        "gui.diff.render",
        || serde_json::json!({ "source_bytes": source.len(), "file": file_path }),
    );
    let value = NSString::from_str(source);
    let rendered =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &value);
    let length = value.length();
    if length == 0 {
        return rendered;
    }

    let base_size = 11.5;
    // SAFETY: AppKit exports this font-weight constant for the process lifetime.
    let base_font =
        unsafe { NSFont::monospacedSystemFontOfSize_weight(base_size, NSFontWeightRegular) };
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setLineSpacing(0.0);
    paragraph.setParagraphSpacing(0.0);
    paragraph.setMinimumLineHeight(15.0);
    paragraph.setMaximumLineHeight(15.0);
    paragraph.setLineHeightMultiple(1.0);
    // SAFETY: Attribute values have the AppKit-documented types and cover the
    // complete UTF-16 range of the immutable source string.
    unsafe {
        rendered.addAttribute_value_range(NSFontAttributeName, &base_font, NSRange::new(0, length));
        rendered.addAttribute_value_range(
            NSForegroundColorAttributeName,
            &NSColor::labelColor(),
            NSRange::new(0, length),
        );
        rendered.addAttribute_value_range(
            NSParagraphStyleAttributeName,
            &paragraph,
            NSRange::new(0, length),
        );
    }

    let syntaxes = SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines);
    let syntax = syntax_for_path(syntaxes, file_path);
    let theme = syntax_theme();
    let mut old_highlighter = HighlightLines::new(syntax, theme);
    let mut new_highlighter = HighlightLines::new(syntax, theme);
    let mut fonts = HashMap::<FontKey, Retained<NSFont>>::new();
    let is_patch = source.lines().any(|line| {
        line.starts_with("diff --git ") || line.starts_with("@@ ") || line.starts_with("@@@ ")
    });
    let syntax_is_plain = syntax.name == "Plain Text";
    let mut location = 0;

    for line in LinesWithEndings::from(source) {
        let line_length = line.encode_utf16().count();
        let kind = diff_line_kind(line, is_patch);
        apply_diff_line_tone(&rendered, kind, NSRange::new(location, line_length));

        let prefix_length = usize::from(matches!(
            kind,
            DiffLineKind::Addition | DiffLineKind::Deletion | DiffLineKind::Context
        ));
        let code = line.get(prefix_length..).unwrap_or(line);
        match kind {
            DiffLineKind::Addition => {
                if !syntax_is_plain {
                    apply_highlighted_line(
                        &rendered,
                        code,
                        &mut new_highlighter,
                        syntaxes,
                        location + prefix_length,
                        base_size,
                        &mut fonts,
                    );
                }
            }
            DiffLineKind::Deletion => {
                if !syntax_is_plain {
                    apply_highlighted_line(
                        &rendered,
                        code,
                        &mut old_highlighter,
                        syntaxes,
                        location + prefix_length,
                        base_size,
                        &mut fonts,
                    );
                }
            }
            DiffLineKind::Context => {
                if !syntax_is_plain {
                    apply_highlighted_line(
                        &rendered,
                        code,
                        &mut new_highlighter,
                        syntaxes,
                        location + prefix_length,
                        base_size,
                        &mut fonts,
                    );
                    // Keep the old-side parser in the same lexical state; its
                    // colors are identical for an unchanged context line.
                    let _ = old_highlighter.highlight_line(code, syntaxes);
                }
            }
            DiffLineKind::Plain => {
                if !syntax_is_plain {
                    apply_highlighted_line(
                        &rendered,
                        line,
                        &mut new_highlighter,
                        syntaxes,
                        location,
                        base_size,
                        &mut fonts,
                    );
                }
            }
            DiffLineKind::Header | DiffLineKind::Hunk | DiffLineKind::Meta => {}
        }
        location += line_length;
    }
    rendered
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLineKind {
    Header,
    Hunk,
    Addition,
    Deletion,
    Context,
    Meta,
    Plain,
}

fn diff_line_kind(line: &str, is_patch: bool) -> DiffLineKind {
    if line.starts_with("New file: ") {
        return DiffLineKind::Header;
    }
    if !is_patch {
        return DiffLineKind::Plain;
    }
    if line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
    {
        DiffLineKind::Header
    } else if line.starts_with("@@ ") || line.starts_with("@@@ ") {
        DiffLineKind::Hunk
    } else if line.starts_with('+') {
        DiffLineKind::Addition
    } else if line.starts_with('-') {
        DiffLineKind::Deletion
    } else if line.starts_with(' ') {
        DiffLineKind::Context
    } else {
        DiffLineKind::Meta
    }
}

fn apply_diff_line_tone(rendered: &NSMutableAttributedString, kind: DiffLineKind, range: NSRange) {
    let (foreground, background) = match kind {
        DiffLineKind::Header => (Some(NSColor::systemBlueColor()), None),
        DiffLineKind::Hunk => (Some(NSColor::systemPurpleColor()), None),
        DiffLineKind::Addition => (
            Some(NSColor::systemGreenColor()),
            Some(NSColor::systemGreenColor().colorWithAlphaComponent(0.10)),
        ),
        DiffLineKind::Deletion => (
            Some(NSColor::systemRedColor()),
            Some(NSColor::systemRedColor().colorWithAlphaComponent(0.10)),
        ),
        DiffLineKind::Meta => (Some(theme::secondary_text()), None),
        DiffLineKind::Context | DiffLineKind::Plain => (None, None),
    };
    // SAFETY: Both optional values are NSColor instances, and caller-provided
    // ranges are calculated from the same source string in UTF-16 units.
    unsafe {
        if let Some(color) = foreground {
            rendered.addAttribute_value_range(NSForegroundColorAttributeName, &color, range);
        }
        if let Some(color) = background {
            rendered.addAttribute_value_range(NSBackgroundColorAttributeName, &color, range);
        }
    }
}

fn render_document(
    document: &MarkdownDocument,
    typography: Typography,
    content_width: f64,
    image_base: &Path,
    show_inline_images: bool,
    enclosure: Option<&Retained<NSTextBlock>>,
    previous: Option<Paragraph>,
) -> Retained<NSMutableAttributedString> {
    let plain = NSString::from_str(&document.text);
    let rendered =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &plain);
    let length = plain.length();
    if length == 0 {
        return rendered;
    }

    // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
    let base_font = design_font(typography.body_design(), typography.body_size(), unsafe {
        NSFontWeightRegular
    });
    // SAFETY: Both AppKit values have the documented attribute types, and the
    // range covers the full UTF-16 length of the native string.
    unsafe {
        rendered.addAttribute_value_range(
            NSForegroundColorAttributeName,
            &NSColor::labelColor(),
            NSRange::new(0, length),
        );
        rendered.addAttribute_value_range(NSFontAttributeName, &base_font, NSRange::new(0, length));
    }

    let mut fonts = HashMap::<FontKey, Retained<NSFont>>::new();
    let theme = syntax_theme();
    let code_background = theme
        .settings
        .background
        .map(native_color)
        .unwrap_or_else(NSColor::quaternarySystemFillColor);
    {
        let _span = perf::span("gui.markdown.inline_styles");
        for run in &document.runs {
            apply_inline_style(&rendered, run.range, &run.style, typography, &mut fonts);
        }
    }
    let paragraph_span = perf::span("gui.markdown.paragraph_styles");
    // Space before a paragraph depends on what it follows: a heading claims
    // its own air, and prose after a table or a quotation gets a paragraph's
    // worth back, since cells and quoted paragraphs carry none of their own.
    let scale = typography.scale;
    let mut previous: Option<Paragraph> = previous;
    for paragraph in &document.paragraphs {
        let own = match paragraph.kind {
            ParagraphKind::Heading(1 | 2) => 14.0 * scale,
            ParagraphKind::Heading(_) => 10.0 * scale,
            _ => 0.0,
        };
        let after = match previous {
            Some(prev)
                if prev.kind == ParagraphKind::TableCell
                    && paragraph.kind != ParagraphKind::TableCell =>
            {
                16.0 * scale
            }
            Some(prev)
                if prev.kind == ParagraphKind::CodeBlock
                    && paragraph.kind != ParagraphKind::CodeBlock =>
            {
                12.0 * scale
            }
            Some(prev) if prev.quote_depth > 0 && paragraph.quote_depth == 0 => 10.0 * scale,
            Some(prev) if prev.kind == ParagraphKind::Rule => 2.0 * scale,
            _ => 0.0,
        };
        // A fenced block's language label shares the block but not the code's
        // tall line grid; the renderer needs to know where the code begins.
        let label_end = document
            .code_blocks
            .iter()
            .find(|block| block.range.location == paragraph.range.location)
            .map(|block| block.code_range.location);
        apply_paragraph_style(
            &rendered,
            paragraph,
            typography,
            &code_background,
            enclosure,
            own.max(after),
            label_end,
        );
        previous = Some(*paragraph);
    }
    drop(paragraph_span);
    {
        let _span = perf::span("gui.markdown.syntax");
        apply_syntax_highlighting(&rendered, document, theme, typography, &mut fonts);
    }
    {
        let _span = perf::span("gui.markdown.tables");
        apply_tables(&rendered, document, enclosure);
    }
    if show_inline_images {
        let _span = perf::span("gui.markdown.images");
        apply_images(&rendered, document, content_width, image_base);
    }
    rendered
}

fn apply_inline_style(
    rendered: &NSMutableAttributedString,
    range: TextRange,
    style: &InlineStyle,
    typography: Typography,
    fonts: &mut HashMap<FontKey, Retained<NSFont>>,
) {
    if range.length == 0 {
        return;
    }
    let native_range = NSRange::new(range.location, range.length);
    let font = font_for(style, typography, fonts);
    let color = match style.tone {
        TextTone::Primary => NSColor::labelColor(),
        TextTone::Secondary => theme::secondary_text(),
        TextTone::Accent => theme::accent(),
        // The rule itself is drawn by the paragraph's block; its text is
        // kept only so plain copies carry a separator.
        TextTone::Separator => NSColor::clearColor(),
    };
    // SAFETY: Every attribute is paired with its documented AppKit/Foundation
    // object type. Parser ranges are maintained in UTF-16 code units.
    unsafe {
        rendered.addAttribute_value_range(NSFontAttributeName, &font, native_range);
        rendered.addAttribute_value_range(NSForegroundColorAttributeName, &color, native_range);
        if style.code_label {
            rendered.addAttribute_value_range(
                NSKernAttributeName,
                &NSNumber::numberWithDouble(0.35),
                native_range,
            );
        }
        // Large sans headings tighten a touch, as display sizes want.
        if matches!(style.heading, Some(1 | 2)) && typography.body_design() == Design::Sans {
            rendered.addAttribute_value_range(
                NSKernAttributeName,
                &NSNumber::numberWithDouble(-0.3),
                native_range,
            );
        }
        if style.code && !style.code_block {
            rendered.addAttribute_value_range(
                NSBackgroundColorAttributeName,
                &NSColor::tertiarySystemFillColor(),
                native_range,
            );
        }
        if style.strikethrough {
            let value = NSNumber::numberWithInteger(NSUnderlineStyle::Single.0);
            rendered.addAttribute_value_range(
                NSStrikethroughStyleAttributeName,
                &value,
                native_range,
            );
        }
        if let Some(destination) = style.link.as_deref().filter(|url| safe_link(url)) {
            let destination = NSString::from_str(destination);
            if let Some(url) = NSURL::URLWithString(&destination) {
                rendered.addAttribute_value_range(NSLinkAttributeName, &url, native_range);
                rendered.addAttribute_value_range(
                    NSForegroundColorAttributeName,
                    &NSColor::linkColor(),
                    native_range,
                );
            }
        }
    }
}

fn font_for(
    style: &InlineStyle,
    typography: Typography,
    fonts: &mut HashMap<FontKey, Retained<NSFont>>,
) -> Retained<NSFont> {
    let (size, design, semibold) = if style.code_label {
        (
            typography.size(TextRole::CodeCaption),
            typography.ui_design(),
            false,
        )
    } else if style.tone == TextTone::Separator {
        // The rule's text is invisible; only its height matters.
        (2.0, Design::Sans, false)
    } else if let Some(level) = style.heading {
        (
            typography.heading_size(level),
            typography.body_design(),
            true,
        )
    } else if style.code_block {
        (typography.code_size(), Design::Mono, false)
    } else if style.code {
        (typography.size(TextRole::InlineCode), Design::Mono, false)
    } else {
        (typography.body_size(), typography.body_design(), false)
    };
    let key = FontKey {
        tenths: (size * 10.0).round() as u16,
        bold: style.bold,
        italic: style.italic,
        semibold,
        design,
    };
    if let Some(font) = fonts.get(&key) {
        return font.clone();
    }
    // SAFETY: AppKit exports the font-weight constants for the process lifetime.
    let weight = unsafe {
        if semibold {
            NSFontWeightSemibold
        } else {
            NSFontWeightRegular
        }
    };
    let font = with_traits(
        design_font(design, size, weight),
        size,
        key.bold,
        key.italic,
    );
    fonts.insert(key, font.clone());
    font
}

/// The monospaced face for a highlighted token, at the code size in force.
fn code_font(
    size: f64,
    bold: bool,
    italic: bool,
    fonts: &mut HashMap<FontKey, Retained<NSFont>>,
) -> Retained<NSFont> {
    let key = FontKey {
        tenths: (size * 10.0).round() as u16,
        bold,
        italic,
        semibold: false,
        design: Design::Mono,
    };
    if let Some(font) = fonts.get(&key) {
        return font.clone();
    }
    // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
    let base = NSFont::monospacedSystemFontOfSize_weight(size, unsafe { NSFontWeightRegular });
    let font = with_traits(base, size, bold, italic);
    fonts.insert(key, font.clone());
    font
}

/// The block behind a quotation: a rule down the left edge and a little
/// inset, the way a quotation is set off on a page. Consecutive quoted
/// paragraphs abut, so the rule runs unbroken down the whole quotation.
fn quotation_block() -> Retained<NSTextBlock> {
    let block = NSTextBlock::new();
    block.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
    block.setBorderColor(Some(&theme::secondary_text()));
    block.setWidth_type_forLayer_edge(
        2.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Border,
        NSRectEdge::MinX,
    );
    block.setWidth_type_forLayer_edge(
        14.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Padding,
        NSRectEdge::MinX,
    );
    for edge in [NSRectEdge::MinY, NSRectEdge::MaxY] {
        block.setWidth_type_forLayer_edge(
            3.0,
            NSTextBlockValueType::AbsoluteValueType,
            NSTextBlockLayer::Padding,
            edge,
        );
    }
    block
}

/// The block that draws a horizontal rule: a single hairline along its
/// bottom edge with air on both sides; the paragraph's own text is invisible.
fn rule_block() -> Retained<NSTextBlock> {
    let block = NSTextBlock::new();
    block.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
    block.setBorderColor(Some(&NSColor::separatorColor()));
    block.setWidth_type_forLayer_edge(
        1.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Border,
        NSRectEdge::MaxY,
    );
    block.setWidth_type_forLayer_edge(
        8.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Margin,
        NSRectEdge::MinY,
    );
    block.setWidth_type_forLayer_edge(
        10.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Margin,
        NSRectEdge::MaxY,
    );
    block
}

#[allow(clippy::too_many_arguments)]
fn apply_paragraph_style(
    rendered: &NSMutableAttributedString,
    paragraph: &Paragraph,
    typography: Typography,
    code_background: &NSColor,
    enclosure: Option<&Retained<NSTextBlock>>,
    spacing_before: f64,
    label_end: Option<usize>,
) {
    let Paragraph {
        range,
        kind,
        quote_depth,
        list_depth,
    } = *paragraph;
    if range.length == 0 {
        return;
    }
    let scale = typography.scale;
    let style = NSMutableParagraphStyle::new();
    style.setLineSpacing(match kind {
        ParagraphKind::CodeBlock | ParagraphKind::Rule => 0.0,
        ParagraphKind::TableCell => 1.5 * scale,
        _ => typography.line_spacing(),
    });
    style.setParagraphSpacing(match kind {
        ParagraphKind::Heading(_) => 6.0 * scale,
        ParagraphKind::CodeBlock | ParagraphKind::TableCell | ParagraphKind::Rule => 0.0,
        // Quoted paragraphs are spaced by their block's padding, which keeps
        // the rule beside them unbroken.
        ParagraphKind::Body if quote_depth > 0 => 0.0,
        ParagraphKind::Body if list_depth > 0 => 6.0 * scale,
        ParagraphKind::Body => typography.paragraph_spacing(),
    });
    style.setParagraphSpacingBefore(spacing_before);
    if typography.hyphenates() && kind == ParagraphKind::Body {
        style.setHyphenationFactor(0.9);
    }

    // A quotation is set in by its block; only nesting beyond the first
    // level indents the text further. List items hang their wrapped lines
    // under the first line's text, past the marker.
    let quote_indent = quote_depth.saturating_sub(1) as f64 * 16.0;
    let list_scale = scale * typography.body_rhythm();
    let list_indent = list_depth.saturating_sub(1) as f64 * 22.0 * list_scale;
    let first_line = quote_indent + list_indent;
    let wrapped_line = first_line
        + if list_depth > 0 {
            26.0 * list_scale
        } else {
            0.0
        };
    style.setFirstLineHeadIndent(first_line);
    style.setHeadIndent(wrapped_line);
    if list_depth > 0 {
        // A real tab keeps text after both bullets and numbers on the same
        // edge as its wrapped continuation, including at larger text sizes.
        // SAFETY: an empty dictionary contains no mistyped tab options.
        let stop = unsafe {
            NSTextTab::initWithTextAlignment_location_options(
                NSTextTab::alloc(),
                NSTextAlignment::Left,
                wrapped_line,
                &NSDictionary::new(),
            )
        };
        style.setTabStops(Some(&NSArray::from_retained_slice(&[stop])));
    }
    style.setTailIndent(0.0);
    if kind == ParagraphKind::CodeBlock {
        // Code uses a deliberate baseline grid. Newlines inside a code block
        // are TextKit paragraphs, so paragraph spacing here would be repeated
        // after every source line rather than only around the block.
        let line_height = (typography.code_size() * 1.36).max(17.0);
        style.setMinimumLineHeight(line_height);
        style.setMaximumLineHeight(line_height);
    }
    style.setLineHeightMultiple(1.0);

    // A paragraph inside a user message nests its own blocks (a quotation,
    // a code block, a rule) within the message's enclosure.
    let mut blocks: Vec<Retained<NSTextBlock>> = enclosure.cloned().into_iter().collect();
    if quote_depth > 0 {
        blocks.push(quotation_block());
    }
    if kind == ParagraphKind::Rule {
        blocks.push(rule_block());
    }
    if kind == ParagraphKind::CodeBlock {
        let block = NSTextBlock::new();
        block.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
        block.setBackgroundColor(Some(code_background));
        block.setBorderColor(Some(&NSColor::separatorColor()));
        block.setWidth_type_forLayer(
            0.5,
            NSTextBlockValueType::AbsoluteValueType,
            NSTextBlockLayer::Border,
        );
        // Tighter above (the label line already lifts the code) than below.
        block.setWidth_type_forLayer_edge(
            6.0,
            NSTextBlockValueType::AbsoluteValueType,
            NSTextBlockLayer::Padding,
            NSRectEdge::MinY,
        );
        block.setWidth_type_forLayer_edge(
            9.0,
            NSTextBlockValueType::AbsoluteValueType,
            NSTextBlockLayer::Padding,
            NSRectEdge::MaxY,
        );
        for edge in [NSRectEdge::MinX, NSRectEdge::MaxX] {
            block.setWidth_type_forLayer_edge(
                12.0,
                NSTextBlockValueType::AbsoluteValueType,
                NSTextBlockLayer::Padding,
                edge,
            );
        }
        for edge in [NSRectEdge::MinY, NSRectEdge::MaxY] {
            block.setWidth_type_forLayer_edge(
                7.0,
                NSTextBlockValueType::AbsoluteValueType,
                NSTextBlockLayer::Margin,
                edge,
            );
        }
        blocks.push(block);
    }
    let block_array = (!blocks.is_empty()).then(|| NSArray::from_retained_slice(&blocks));
    if let Some(block_array) = &block_array {
        style.setTextBlocks(block_array);
    }

    // A fenced block's language label is a small line of its own inside the
    // same block: on the code's tall grid it would float above a gap.
    let mut code_from = range.location;
    if let (ParagraphKind::CodeBlock, Some(label_end)) = (kind, label_end)
        && label_end > range.location
        && label_end < range.end()
    {
        let label_style = NSMutableParagraphStyle::new();
        label_style.setParagraphSpacingBefore(spacing_before);
        label_style.setParagraphSpacing(2.0 * scale);
        label_style.setMinimumLineHeight(typography.size(TextRole::CodeCaption) * 1.3);
        label_style.setMaximumLineHeight(typography.size(TextRole::CodeCaption) * 1.3);
        if let Some(block_array) = &block_array {
            label_style.setTextBlocks(block_array);
        }
        // SAFETY: as below, over the label's own range.
        unsafe {
            rendered.addAttribute_value_range(
                NSParagraphStyleAttributeName,
                &label_style,
                NSRange::new(range.location, label_end - range.location),
            );
        }
        code_from = label_end;
    }

    // SAFETY: The paragraph style has the required AppKit type and the parser
    // range is expressed in UTF-16 code units for NSMutableAttributedString.
    unsafe {
        rendered.addAttribute_value_range(
            NSParagraphStyleAttributeName,
            &style,
            NSRange::new(code_from, range.end() - code_from),
        );
    }
}

fn apply_syntax_highlighting(
    rendered: &NSMutableAttributedString,
    document: &MarkdownDocument,
    theme: &Theme,
    typography: Typography,
    fonts: &mut HashMap<FontKey, Retained<NSFont>>,
) {
    let syntaxes = SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines);
    for block in &document.code_blocks {
        let Some(source) = utf16_slice(&document.text, block.code_range) else {
            continue;
        };
        let syntax = syntax_for(syntaxes, &block.language);
        let mut highlighter = HighlightLines::new(syntax, theme);
        let mut location = block.code_range.location;
        for line in LinesWithEndings::from(source) {
            apply_highlighted_line(
                rendered,
                line,
                &mut highlighter,
                syntaxes,
                location,
                typography.code_size(),
                fonts,
            );
            location += line.encode_utf16().count();
        }
    }
}

/// Colors one line of code by syntax, in the monospaced face at `size`.
fn apply_highlighted_line(
    rendered: &NSMutableAttributedString,
    line: &str,
    highlighter: &mut HighlightLines<'_>,
    syntaxes: &SyntaxSet,
    mut location: usize,
    size: f64,
    fonts: &mut HashMap<FontKey, Retained<NSFont>>,
) {
    let Ok(regions) = highlighter.highlight_line(line, syntaxes) else {
        return;
    };
    for (style, token) in regions {
        let length = token.encode_utf16().count();
        if length > 0 {
            let color = native_color(style.foreground);
            let font = code_font(
                size,
                style.font_style.contains(FontStyle::BOLD),
                style.font_style.contains(FontStyle::ITALIC),
                fonts,
            );
            // SAFETY: Syntect regions partition the source text, and the
            // location is advanced in native UTF-16 code units.
            unsafe {
                rendered.addAttribute_value_range(
                    NSForegroundColorAttributeName,
                    &color,
                    NSRange::new(location, length),
                );
                rendered.addAttribute_value_range(
                    NSFontAttributeName,
                    &font,
                    NSRange::new(location, length),
                );
            }
        }
        location += length;
    }
}

fn syntax_for<'a>(
    syntaxes: &'a SyntaxSet,
    language: &str,
) -> &'a syntect::parsing::SyntaxReference {
    let lower = language.to_ascii_lowercase();
    let token = match lower.as_str() {
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "sh" | "shell" | "zsh" => "bash",
        "yml" => "yaml",
        other => other,
    };
    syntaxes
        .find_syntax_by_token(token)
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text())
}

fn syntax_for_path<'a>(
    syntaxes: &'a SyntaxSet,
    path: &Path,
) -> &'a syntect::parsing::SyntaxReference {
    let file_name = path.file_name().and_then(|value| value.to_str());
    let extension = path.extension().and_then(|value| value.to_str());
    file_name
        .and_then(|value| syntaxes.find_syntax_by_extension(value))
        .or_else(|| extension.and_then(|value| syntaxes.find_syntax_by_extension(value)))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text())
}

/// Loads the syntax and theme sets, which take a good part of a second to
/// deserialize the first time; called from a worker at launch so no message
/// pays for it on the main thread.
pub(crate) fn warm_up_highlighting() {
    let syntaxes = SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines);
    let theme = syntax_theme();
    // A syntax compiles its patterns the first time it highlights a line —
    // a good part of a second for the larger grammars — so the languages a
    // transcript is likely to show are exercised here, off the main thread.
    for language in [
        "rust", "python", "javascript", "typescript", "bash", "json", "toml", "yaml", "markdown",
        "html", "css", "go", "c", "cpp", "java", "swift", "sql", "ruby", "text",
    ] {
        let mut highlighter = HighlightLines::new(syntax_for(syntaxes, language), theme);
        let _ = highlighter.highlight_line("let x = 1; // warm\n", syntaxes);
    }
}

fn syntax_theme() -> &'static Theme {
    let themes = THEME_SET.get_or_init(two_face::theme::extra);
    let preferred = if is_dark_appearance() {
        EmbeddedThemeName::OneHalfDark
    } else {
        EmbeddedThemeName::OneHalfLight
    };
    themes.get(preferred)
}

fn native_color(color: Color) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(color.r) / 255.0,
        f64::from(color.g) / 255.0,
        f64::from(color.b) / 255.0,
        f64::from(color.a) / 255.0,
    )
}

fn is_dark_appearance() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return true;
    };
    // SAFETY: These AppKit appearance-name constants live for the process.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let choices = NSArray::from_slice(&[aqua, dark]);
    NSApplication::sharedApplication(mtm)
        .effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&choices)
        .is_some_and(|appearance| appearance.to_string() == dark.to_string())
}

fn apply_tables(
    rendered: &NSMutableAttributedString,
    document: &MarkdownDocument,
    enclosure: Option<&Retained<NSTextBlock>>,
) {
    for markdown_table in &document.tables {
        if markdown_table.columns == 0 {
            continue;
        }
        let table = NSTextTable::new();
        table.setNumberOfColumns(markdown_table.columns);
        table.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
        table.setLayoutAlgorithm(NSTextTableLayoutAlgorithm::AutomaticLayoutAlgorithm);
        table.setCollapsesBorders(true);
        table.setHidesEmptyCells(false);
        let last_row = markdown_table
            .cells
            .iter()
            .map(|cell| cell.row)
            .max()
            .unwrap_or(0);
        for cell in &markdown_table.cells {
            apply_table_cell(rendered, &table, cell, cell.row == last_row, enclosure);
        }
    }
}

fn apply_table_cell(
    rendered: &NSMutableAttributedString,
    table: &NSTextTable,
    cell: &TableCell,
    last_row: bool,
    enclosure: Option<&Retained<NSTextBlock>>,
) {
    let Ok(row) = isize::try_from(cell.row) else {
        return;
    };
    let Ok(column) = isize::try_from(cell.column) else {
        return;
    };
    let block = NSTextTableBlock::initWithTable_startingRow_rowSpan_startingColumn_columnSpan(
        NSTextTableBlock::alloc(),
        table,
        row,
        1,
        column,
        1,
    );
    // Rules run horizontally only, as a well-set table's do: a firmer pair
    // around the header row, a hairline between body rows, and no vertical
    // rules or header fill — the columns' alignment does that work.
    block.setWidth_type_forLayer(
        0.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Border,
    );
    let (rule, top, bottom) = if cell.header {
        (theme::secondary_text(), 1.0, 1.0)
    } else {
        (NSColor::separatorColor(), 0.0, 0.5)
    };
    block.setBorderColor(Some(&rule));
    block.setWidth_type_forLayer_edge(
        top,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Border,
        NSRectEdge::MinY,
    );
    block.setWidth_type_forLayer_edge(
        bottom,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Border,
        NSRectEdge::MaxY,
    );
    for edge in [NSRectEdge::MinY, NSRectEdge::MaxY] {
        block.setWidth_type_forLayer_edge(
            6.0,
            NSTextBlockValueType::AbsoluteValueType,
            NSTextBlockLayer::Padding,
            edge,
        );
    }
    // The first column starts on the text edge, like the prose around it.
    block.setWidth_type_forLayer_edge(
        if cell.column == 0 { 2.0 } else { 10.0 },
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Padding,
        NSRectEdge::MinX,
    );
    block.setWidth_type_forLayer_edge(
        10.0,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Padding,
        NSRectEdge::MaxX,
    );
    // Cell paragraphs carry no paragraph spacing (it would open every row);
    // the paragraph after the table gets its air back through the space
    // `render_document` adds before it, and the last row adds a little more.
    if last_row {
        block.setWidth_type_forLayer_edge(
            4.0,
            NSTextBlockValueType::AbsoluteValueType,
            NSTextBlockLayer::Margin,
            NSRectEdge::MaxY,
        );
    }
    let style = NSMutableParagraphStyle::new();
    style.setParagraphSpacing(0.0);
    style.setLineSpacing(1.0);
    style.setAlignment(match cell.alignment {
        Alignment::Center => NSTextAlignment::Center,
        Alignment::Right => NSTextAlignment::Right,
        Alignment::Left | Alignment::None => NSTextAlignment::Left,
    });
    let block: Retained<NSTextBlock> = block.into_super();
    let mut blocks: Vec<Retained<NSTextBlock>> = enclosure.cloned().into_iter().collect();
    blocks.push(block);
    style.setTextBlocks(&NSArray::from_retained_slice(&blocks));
    // SAFETY: The native table-cell range is maintained in UTF-16 units and
    // the paragraph style owns the table block assigned to it.
    unsafe {
        rendered.addAttribute_value_range(
            NSParagraphStyleAttributeName,
            &style,
            NSRange::new(cell.range.location, cell.range.length),
        );
    }
}

fn apply_images(
    rendered: &NSMutableAttributedString,
    document: &MarkdownDocument,
    content_width: f64,
    image_base: &Path,
) {
    for image in document.images.iter().rev() {
        let Some(replacement) = image_replacement(image, content_width, image_base) else {
            continue;
        };
        rendered.replaceCharactersInRange_withAttributedString(
            NSRange::new(image.range.location, image.range.length),
            &replacement,
        );
    }
}

fn image_replacement(
    spec: &MarkdownImage,
    content_width: f64,
    image_base: &Path,
) -> Option<Retained<NSMutableAttributedString>> {
    let source = resolved_image_source(&spec.destination, image_base);
    let bytes = request_image(&source)?;
    let data = NSData::with_bytes(&bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    let original = image.size();
    if original.width <= 0.0 || original.height <= 0.0 {
        return None;
    }
    let max_width = (content_width - 28.0).clamp(180.0, 760.0);
    let max_height = 520.0;
    let scale = (max_width / original.width)
        .min(max_height / original.height)
        .min(1.0);
    let rendered_size = NSSize::new(original.width * scale, original.height * scale);
    let attachment = NSTextAttachment::new();
    attachment.setImage(Some(&image));
    attachment.setBounds(NSRect::new(NSPoint::new(0.0, 0.0), rendered_size));

    let value = NSString::from_str(&format!("\u{fffc}\n{}", spec.alt));
    let replacement =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &value);
    let caption_start = 2;
    let caption_length = value.length().saturating_sub(caption_start);
    // SAFETY: NSTextAttachment is the documented value for the attachment
    // attribute; the object-replacement character is exactly one UTF-16 unit.
    unsafe {
        replacement.addAttribute_value_range(
            NSAttachmentAttributeName,
            &attachment,
            NSRange::new(0, 1),
        );
        replacement.addAttribute_value_range(
            NSFontAttributeName,
            &NSFont::systemFontOfSize(12.0),
            NSRange::new(caption_start, caption_length),
        );
        replacement.addAttribute_value_range(
            NSForegroundColorAttributeName,
            &theme::secondary_text(),
            NSRange::new(caption_start, caption_length),
        );
    }
    Some(replacement)
}

fn resolved_image_source(destination: &str, image_base: &Path) -> String {
    if destination.starts_with("https://")
        || destination.starts_with("http://")
        || destination.starts_with("file://")
        || Path::new(destination).is_absolute()
    {
        destination.to_owned()
    } else {
        image_base.join(destination).to_string_lossy().into_owned()
    }
}

/// Whether images on the web may be fetched; see
/// `Preferences::load_remote_images`. Local images are unaffected.
static REMOTE_IMAGES_ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn allow_remote_images(allowed: bool) {
    REMOTE_IMAGES_ALLOWED.store(allowed, std::sync::atomic::Ordering::Relaxed);
}

fn is_remote(destination: &str) -> bool {
    let lower = destination.get(..8).unwrap_or(destination).to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn request_image(destination: &str) -> Option<Arc<[u8]>> {
    if is_remote(destination) && !REMOTE_IMAGES_ALLOWED.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let Ok(mut loader) = image_loader().lock() else {
        return None;
    };
    match loader.states.get(destination) {
        Some(ImageState::Loaded(bytes)) => return Some(Arc::clone(bytes)),
        Some(ImageState::Loading | ImageState::Failed) => return None,
        None => {}
    }
    loader
        .states
        .insert(destination.to_owned(), ImageState::Loading);
    if loader.tasks.send(destination.to_owned()).is_err() {
        loader
            .states
            .insert(destination.to_owned(), ImageState::Failed);
    }
    None
}

fn image_loader() -> &'static Mutex<ImageLoader> {
    IMAGE_LOADER.get_or_init(|| Mutex::new(ImageLoader::new()))
}

fn image_worker(tasks: flume::Receiver<String>, results: flume::Sender<ImageResult>) {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build();
    let agent: ureq::Agent = config.into();
    while let Ok(destination) = tasks.recv() {
        let bytes = load_image_bytes(&agent, &destination);
        if results.send(ImageResult { destination, bytes }).is_err() {
            return;
        }
    }
}

fn load_image_bytes(agent: &ureq::Agent, destination: &str) -> Option<Vec<u8>> {
    if is_remote(destination) {
        if !REMOTE_IMAGES_ALLOWED.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let mut response = agent.get(destination).call().ok()?;
        let bytes = response.body_mut().read_to_vec().ok()?;
        return (bytes.len() <= MAX_IMAGE_BYTES).then_some(bytes);
    }
    let path = destination.strip_prefix("file://").unwrap_or(destination);
    if !Path::new(path).is_absolute() {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_IMAGE_BYTES as u64 {
        return None;
    }
    std::fs::read(path).ok()
}

fn utf16_slice(value: &str, range: TextRange) -> Option<&str> {
    let start = utf16_byte_index(value, range.location)?;
    let end = utf16_byte_index(value, range.end())?;
    value.get(start..end)
}

fn utf16_byte_index(value: &str, target: usize) -> Option<usize> {
    if target == 0 {
        return Some(0);
    }
    let mut utf16 = 0;
    for (index, character) in value.char_indices() {
        if utf16 == target {
            return Some(index);
        }
        utf16 += character.len_utf16();
    }
    (utf16 == target).then_some(value.len())
}

pub(crate) fn safe_link(destination: &str) -> bool {
    let lower = destination.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with("file://")
}

#[cfg(test)]
mod fold_tests {
    use super::fold_thought;

    #[test]
    fn a_thought_folds_to_its_title_and_counts_the_rest() {
        assert_eq!(fold_thought("Reading the request\nThe prompt asks for three things."), ("Reading the request", 6));
        assert_eq!(fold_thought("One line only"), ("One line only", 0));
        assert_eq!(fold_thought("  \nleading blank"), ("leading blank", 0));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use crate::chat::{TranscriptMessage, TranscriptMessageKind};
    use crate::chat_typography::{TextRole, TextSizes};
    use crate::markdown::{MarkdownImage, TextRange};
    use objc2_app_kit::NSParagraphStyle;

    use super::{
        ChatStyle, DiffLineKind, ImageState, NSAttachmentAttributeName,
        NSBackgroundColorAttributeName, NSLinkAttributeName, NSParagraphStyleAttributeName,
        NSTextTableBlock, SYNTAX_SET, Typography, diff_line_kind, image_loader, image_replacement,
        render_diff, render_message, resolved_image_source, safe_link, syntax_for, syntax_for_path,
        utf16_byte_index,
    };

    const ONE_PIXEL_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5,
        0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64,
        0xf8, 0x0f, 0x00, 0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xe3, 0x66, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn only_makes_explicit_safe_schemes_clickable() {
        assert!(safe_link("https://example.com"));
        assert!(safe_link("MAILTO:user@example.com"));
        assert!(safe_link("file:///tmp/report.txt"));
        assert!(!safe_link("javascript:alert(1)"));
        assert!(!safe_link("relative/path"));
    }

    #[test]
    fn extended_syntax_pack_recognizes_swift() {
        let syntaxes = SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines);
        assert_eq!(syntax_for(syntaxes, "swift").name, "Swift");
        assert_eq!(
            syntax_for_path(syntaxes, Path::new("Cargo.lock")).name,
            "TOML"
        );
    }

    #[test]
    fn classifies_patch_lines_without_treating_file_headers_as_edits() {
        assert_eq!(
            diff_line_kind("diff --git a/a b/a\n", true),
            DiffLineKind::Header
        );
        assert_eq!(diff_line_kind("--- a/a\n", true), DiffLineKind::Header);
        assert_eq!(diff_line_kind("+++ b/a\n", true), DiffLineKind::Header);
        assert_eq!(diff_line_kind("@@ -1 +1 @@\n", true), DiffLineKind::Hunk);
        assert_eq!(
            diff_line_kind("+let value = 2;\n", true),
            DiffLineKind::Addition
        );
        assert_eq!(
            diff_line_kind("-let value = 1;\n", true),
            DiffLineKind::Deletion
        );
        assert_eq!(
            diff_line_kind("New file: src/main.rs\n", false),
            DiffLineKind::Header
        );
        assert_eq!(diff_line_kind("+not a patch\n", false), DiffLineKind::Plain);
    }

    #[test]
    fn native_diff_marks_edit_lines_with_adaptive_backgrounds() {
        let source = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-let old = 1;\n+let new = 2;\n";
        let rendered = render_diff(source, Path::new("src/main.rs"));
        let text = rendered.string().to_string();
        let deletion = text.find("-let").expect("deletion line");
        let addition = text.find("+let").expect("addition line");
        // SAFETY: Both queried indexes point to ASCII characters in the same
        // immutable source string, and AppKit accepts null effective ranges.
        unsafe {
            assert!(
                rendered
                    .attribute_atIndex_effectiveRange(
                        NSBackgroundColorAttributeName,
                        deletion,
                        std::ptr::null_mut(),
                    )
                    .is_some()
            );
            assert!(
                rendered
                    .attribute_atIndex_effectiveRange(
                        NSBackgroundColorAttributeName,
                        addition,
                        std::ptr::null_mut(),
                    )
                    .is_some()
            );
        }
    }

    #[test]
    fn context_sizes_reach_native_fonts_without_scaling_overrides() {
        let mut sizes = TextSizes::default();
        for (role, size) in [
            (TextRole::Assistant, 18.5),
            (TextRole::User, 21.0),
            (TextRole::Heading1, 31.0),
            (TextRole::Heading2, 27.5),
            (TextRole::Heading3, 25.0),
            (TextRole::SmallHeading, 23.0),
            (TextRole::InlineCode, 16.0),
            (TextRole::CodeBlock, 22.0),
            (TextRole::CodeCaption, 20.0),
            (TextRole::Speaker, 14.0),
            (TextRole::Activity, 19.0),
            (TextRole::Thinking, 17.0),
            (TextRole::Notice, 15.0),
        ] {
            assert!(sizes.set(role, Some(size)));
        }
        let typography = Typography::new(ChatStyle::Editorial, 1.12).with_sizes(sizes);
        let mut message = TranscriptMessage {
            author: "omp".into(),
            body: "# First\n\n## Second\n\n### Third\n\n#### Fourth\n\n##### Fifth\n\n###### Sixth\n\nProse and `inline`.\n\n```rust\nlet value = 1;\n```".into(),
            kind: TranscriptMessageKind::Assistant, status: None, detail_id: None,
        };
        let font_size = |rendered: &super::NSMutableAttributedString, needle: &str| {
            let text = rendered.string().to_string();
            let index = text[..text.find(needle).unwrap()].encode_utf16().count();
            // SAFETY: the index points into this attributed string; NSFont is
            // the documented type, and a null effective-range is accepted.
            unsafe {
                rendered
                    .attribute_atIndex_effectiveRange(
                        super::NSFontAttributeName,
                        index,
                        std::ptr::null_mut(),
                    )
                    .unwrap()
                    .downcast::<super::NSFont>()
                    .unwrap()
                    .pointSize()
            }
        };
        for kind in [
            TranscriptMessageKind::Assistant,
            TranscriptMessageKind::User,
        ] {
            message.kind = kind;
            let rendered = render_message(&message, 640.0, Path::new("/tmp"), typography, false);
            for (needle, expected) in [
                ("Omperor", 14.0),
                ("First", 31.0),
                ("Second", 27.5),
                ("Third", 25.0),
                ("Fourth", 23.0),
                ("Fifth", 23.0),
                ("Sixth", 23.0),
                (
                    "Prose",
                    if kind == TranscriptMessageKind::User {
                        21.0
                    } else {
                        18.5
                    },
                ),
                ("inline", 16.0),
                ("rust", 20.0),
                ("let value", 22.0),
            ] {
                assert_eq!(
                    font_size(&rendered, needle),
                    expected,
                    "{needle} in {kind:?}"
                );
            }
        }
        message.body = "Context text".into();
        for (kind, expected) in [
            (TranscriptMessageKind::Tool, 19.0),
            (TranscriptMessageKind::Event, 19.0),
            (TranscriptMessageKind::Thinking, 17.0),
            (TranscriptMessageKind::Notice, 15.0),
        ] {
            message.kind = kind;
            let rendered = render_message(&message, 640.0, Path::new("/tmp"), typography, false);
            assert_eq!(font_size(&rendered, "Context"), expected);
        }
    }

    #[test]
    fn a_growing_message_reuses_its_settled_blocks_and_reads_as_one_render() {
        let typography = Typography::new(ChatStyle::Modern, 1.0);
        let mut message = TranscriptMessage {
            author: "omp".into(),
            body: "## Title\n\nFirst paragraph, [a link][ref].\n\n```rust\nfn a() {}\n".into(),
            kind: TranscriptMessageKind::Assistant,
            status: None,
            detail_id: None,
        };
        let first = super::render_message_incrementally(
            &message, None, 640.0, Path::new("/tmp"), typography, false,
        );
        assert_eq!(first.unchanged_prefix, 0);
        // The fence closes and more arrives: only the tail is new.
        message.body.push_str("```\n\nSecond paragraph.\n\n[ref]: https://example.com\n");
        let second = super::render_message_incrementally(
            &message, Some(&first.rendered), 640.0, Path::new("/tmp"), typography, false,
        );
        let whole = render_message(&message, 640.0, Path::new("/tmp"), typography, false);
        assert_eq!(second.rendered.attributed().string().to_string(), whole.string().to_string());
        assert_eq!(second.rendered.length(), whole.length());
        // A definition arrived, which any block may use, so every block is
        // set again; only the speaker line is kept.
        assert_eq!(second.unchanged_prefix, "Omperor\n".encode_utf16().count());
        let prefix = whole.string().to_string().encode_utf16().take(second.unchanged_prefix).collect::<Vec<_>>();
        let tail = second.changed_tail().string().to_string();
        assert_eq!(String::from_utf16(&prefix).unwrap() + &tail, whole.string().to_string());

        // A third delta touches only the open tail (the definition line's
        // block, which it continues): everything before it is reused.
        message.body.push_str("More of the end.\n");
        let third = super::render_message_incrementally(
            &message, Some(&second.rendered), 640.0, Path::new("/tmp"), typography, false,
        );
        let whole = render_message(&message, 640.0, Path::new("/tmp"), typography, false);
        assert_eq!(third.rendered.attributed().string().to_string(), whole.string().to_string());
        assert_eq!(
            third.unchanged_prefix,
            "Omperor\nTitle\nFirst paragraph, a link.\nrust\nfn a() {}\nSecond paragraph.\n"
                .encode_utf16()
                .count()
        );
    }

    #[test]
    fn builds_native_attributes_for_markdown_content() {
        let message = TranscriptMessage {
            author: "omp".into(),
            body: "## Native\n\nA **bold** [link](https://example.com) and `code`.".into(),
            kind: TranscriptMessageKind::Assistant,
            status: None,
            detail_id: None,
        };
        let rendered = render_message(
            &message,
            640.0,
            Path::new("/tmp"),
            Typography::new(ChatStyle::Modern, 1.0),
            true,
        );
        let text = rendered.string().to_string();

        // Speaker labels use readable sentence case.
        assert_eq!(text, "Omperor\nNative\nA bold link and code.\n\n");
        let link_index = text.find("link").expect("rendered link");
        let code_index = text.find("code").expect("rendered inline code");
        // SAFETY: The queried indexes point at ASCII inside the native string,
        // and a null effective-range pointer is accepted by the API.
        unsafe {
            assert!(
                rendered
                    .attribute_atIndex_effectiveRange(
                        NSLinkAttributeName,
                        link_index,
                        std::ptr::null_mut(),
                    )
                    .is_some()
            );
            assert!(
                rendered
                    .attribute_atIndex_effectiveRange(
                        NSBackgroundColorAttributeName,
                        code_index,
                        std::ptr::null_mut(),
                    )
                    .is_some()
            );
        }
    }

    #[test]
    fn builds_native_code_blocks_and_table_cells() {
        let message = TranscriptMessage {
            author: "omp".into(),
            body: "```rust\nfn main() {}\n```\n\n| A | B |\n| :--- | ---: |\n| x | y |".into(),
            kind: TranscriptMessageKind::Assistant,
            status: None,
            detail_id: None,
        };
        let rendered = render_message(
            &message,
            640.0,
            Path::new("/tmp"),
            Typography::new(ChatStyle::Modern, 1.0),
            true,
        );
        let text = rendered.string().to_string();

        assert!(text.contains("rust\nfn main() {}"));
        assert!(text.contains("A\nB\nx\ny\n"));
        let code_index = text.find("fn main").expect("rendered code");
        let cell_index = text.find("A\nB").expect("rendered table");
        // SAFETY: The queried indexes point into ASCII content and AppKit
        // accepts a null effective-range pointer.
        unsafe {
            let code_style = rendered
                .attribute_atIndex_effectiveRange(
                    NSParagraphStyleAttributeName,
                    code_index,
                    std::ptr::null_mut(),
                )
                .expect("code paragraph style")
                .downcast::<NSParagraphStyle>()
                .expect("native paragraph style");
            assert_eq!(code_style.textBlocks().len(), 1);
            let code_block = code_style.textBlocks().objectAtIndex(0);
            assert_eq!(code_block.contentWidth(), 100.0);
            assert_eq!(
                code_block.contentWidthValueType(),
                super::NSTextBlockValueType::PercentageValueType
            );

            let cell_style = rendered
                .attribute_atIndex_effectiveRange(
                    NSParagraphStyleAttributeName,
                    cell_index,
                    std::ptr::null_mut(),
                )
                .expect("table paragraph style")
                .downcast::<NSParagraphStyle>()
                .expect("native paragraph style");
            let blocks = cell_style.textBlocks();
            assert_eq!(blocks.len(), 1);
            let block = blocks.objectAtIndex(0);
            assert!(block.downcast_ref::<NSTextTableBlock>().is_some());
        }
    }

    #[test]
    fn maps_utf16_offsets_to_rust_byte_indexes() {
        assert_eq!(utf16_byte_index("a😀b", 0), Some(0));
        assert_eq!(utf16_byte_index("a😀b", 1), Some(1));
        assert_eq!(utf16_byte_index("a😀b", 3), Some(5));
        assert_eq!(utf16_byte_index("a😀b", 4), Some(6));
        assert_eq!(utf16_byte_index("a😀b", 2), None);
    }

    #[test]
    fn builds_a_native_attachment_for_loaded_image_bytes() {
        let destination = "/omperor-native-image-test.png";
        image_loader()
            .lock()
            .expect("image loader lock")
            .states
            .insert(
                destination.to_owned(),
                ImageState::Loaded(Arc::from(ONE_PIXEL_PNG)),
            );
        let image = MarkdownImage {
            range: TextRange {
                location: 0,
                length: 1,
            },
            destination: destination.to_owned(),
            alt: "pixel".to_owned(),
        };

        let rendered =
            image_replacement(&image, 640.0, Path::new("/tmp")).expect("valid PNG renders");

        assert!(rendered.string().to_string().starts_with('\u{fffc}'));
        // SAFETY: Index zero is the native attachment placeholder and AppKit
        // accepts a null effective-range pointer.
        unsafe {
            assert!(
                rendered
                    .attribute_atIndex_effectiveRange(
                        NSAttachmentAttributeName,
                        0,
                        std::ptr::null_mut(),
                    )
                    .is_some()
            );
        }
    }

    #[test]
    fn resolves_relative_images_against_the_active_workspace() {
        assert_eq!(
            resolved_image_source("assets/chart.png", Path::new("/work/project")),
            "/work/project/assets/chart.png"
        );
        assert_eq!(
            resolved_image_source("https://example.com/chart.png", Path::new("/work/project")),
            "https://example.com/chart.png"
        );
    }
}
