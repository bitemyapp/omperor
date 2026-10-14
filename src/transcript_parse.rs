//! Parsing for the transcript, off the main thread (AGENTS.md). One worker
//! parses what the transcript shows: a finished message, cut into segments
//! when it is long, and every growth of the message being streamed, diffed
//! against its previous parse. Results come back through a channel the main
//! thread drains when the worker wakes it.
//!
//! A long message is cut between top-level blocks into segments of about
//! `SEGMENT_TARGET` UTF-16 units, each its own hosted view, so opening,
//! resizing or scrolling a long answer lays out only the part in view: the
//! first layout of a view is proportional to its text (about 2 ms per KB of
//! dense Markdown).
//!
//! Segmentation never shows. A segment is parsed with what the rest of the
//! message contributes to it (`SegmentContext`: the definitions it cites and
//! the `<details>` tags its own pair with), and every cut is checked on the
//! worker against the message parsed whole (`transcript_invariant`): a
//! boundary where the parts would present anything differently from one
//! view is passed over, and the segment keeps going. A streamed answer is
//! sealed only before a block the stream can no longer change; a sealed
//! segment whose context changes as the stream goes on (a definition it
//! cites arrives) is parsed again and updated in the pass that shows the
//! change, and one the check finds changed some other way goes back into
//! the growing tail.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use block2::RcBlock;
use objc2_core_foundation::{CFRunLoop, kCFRunLoopCommonModes};
use upleft_core::ast_diff::ASTDiff;
use upleft_core::derived::SourceScanner;
use upleft_core::parser::MarkdownParser;
use upleft_core::source_positions::SourceMap;
use upleft_core::safe_html::SafeHTMLParser;
use upleft_core::{BlockContent, DirtySet, InlineKind, ParsedDocument, SegmentContext};

use crate::transcript_invariant::{self as invariant, Part};

/// A message at most this long (UTF-16 units) is one view: making it fits
/// the 3 ms a pass may spend making views.
pub(crate) const WHOLE_LIMIT: usize = 1024;
/// Roughly how long a segment of a longer message is: a view costs about
/// 1 ms and dense Markdown lays out at about 2.2 ms per KB, so making a
/// segment's view fits in a 240 Hz pass beside the rest of it.
pub(crate) const SEGMENT_TARGET: usize = 640;
/// How many boundaries a seal tries, latest first, before the tail keeps
/// growing until the next append.
const SEAL_CANDIDATES: usize = 8;
/// Past this length a document is parsed without its extension passes
/// (`ParseOptions::extension_pass_limit`), which its parts would not be.
const EXTENSION_LIMIT: usize = 5_000_000;

/// The sizes segmentation works to; tests use smaller ones, to cut short
/// documents often.
#[derive(Clone, Copy)]
struct Limits {
    whole: usize,
    target: usize,
}

const LIMITS: Limits = Limits { whole: WHOLE_LIMIT, target: SEGMENT_TARGET };

/// What the transcript asks of the worker. `row` names the transcript row;
/// `generation` tells a stale answer from a current one.
pub(crate) enum Job {
    /// Parse a finished message, cut into segments when it is long.
    Segment { row: u64, generation: u64, text: String },
    /// The streamed message's tail, from UTF-16 offset `start` of the
    /// message, now reads `text`. With `reset`, the answer is a wholesale
    /// parse (a view that has not seen the previous one). `sealed` is what
    /// was sealed before the tail, in order; `heading_before`, whether it
    /// holds a heading.
    Stream {
        row: u64,
        generation: u64,
        start: usize,
        text: String,
        reset: bool,
        heading_before: bool,
        sealed: Arc<[SealedSource]>,
    },
    /// The row is gone: forget its previous parse.
    Forget { row: u64 },
}

pub(crate) struct Segment {
    /// The segment's slice of the message.
    pub(crate) text: String,
    /// Its parse, with what the rest of the message gives it.
    pub(crate) document: Arc<ParsedDocument>,
    /// A heading of the message comes before the segment, so its view is a
    /// hosted continuation: its first heading is not the message's first.
    pub(crate) heading_before: bool,
    /// Where the slice starts in the message, in UTF-16 units.
    pub(crate) start: usize,
    /// The slice's length, in UTF-16 units.
    pub(crate) length: usize,
}

/// A sealed segment of a streamed message, as the worker needs it: to give
/// the rest its definitions and `<details>` tags, and to keep its context.
#[derive(Clone)]
pub(crate) struct SealedSource {
    /// Its slice of the message, and the slice lowercased.
    pub(crate) own: Arc<str>,
    pub(crate) lowered: Arc<str>,
    pub(crate) document: Arc<ParsedDocument>,
    pub(crate) heading_before: bool,
    pub(crate) exports: Arc<Exports>,
}

/// What a part of a message offers the rest.
#[derive(Clone, PartialEq, Default)]
pub(crate) struct Exports {
    definitions: Vec<Definition>,
    /// Every label it defines (`keys`).
    keys: Vec<String>,
    /// A `<details>` opening or closing tag, as pairing looks for one.
    opens: bool,
    closes: bool,
}

pub(crate) enum Done {
    Segmented { row: u64, generation: u64, segments: Vec<Segment> },
    /// The tail at `start` parsed and diffed against the previous parse of
    /// the same tail; with `seal`, cut in two; with `unseal`, joined again
    /// with sealed segments from that index on (then `start`, `text` and
    /// `document` are the joined tail's). `updates` are sealed segments
    /// parsed again in a new context.
    Streamed {
        row: u64,
        generation: u64,
        start: usize,
        text: String,
        document: Arc<ParsedDocument>,
        dirty: DirtySet,
        seal: Option<Seal>,
        updates: Vec<SealedUpdate>,
        unseal: Option<usize>,
    },
}

/// A streamed tail grown past `2 × SEGMENT_TARGET`, cut before a complete
/// top-level block where the cut cannot show: the head is sealed into the
/// view that showed the tail, and the rest starts a new tail. A long answer
/// then streams into a view of a few KB however long it grows, and every
/// append costs what an append to a short message costs.
pub(crate) struct Seal {
    /// The head's length, in UTF-16 units and in bytes of the tail.
    pub(crate) units: usize,
    pub(crate) bytes: usize,
    /// The head. It is set whole, never diffed against the tail it was cut
    /// from (`Transcript::apply_tail`).
    pub(crate) head: Arc<ParsedDocument>,
    /// The head holds a heading.
    pub(crate) head_heading: bool,
    pub(crate) head_exports: Arc<Exports>,
    /// The rest, the new tail, from `start + units`.
    pub(crate) rest: Arc<ParsedDocument>,
}

/// Sealed segment `index`, parsed again in its new context.
pub(crate) struct SealedUpdate {
    pub(crate) index: usize,
    pub(crate) document: Arc<ParsedDocument>,
    pub(crate) dirty: DirtySet,
}

impl SealedSource {
    /// A head just sealed: `own` is its slice.
    pub(crate) fn sealed(own: &str, seal: &Seal, heading_before: bool) -> SealedSource {
        SealedSource {
            own: Arc::from(own),
            lowered: Arc::from(own.to_lowercase()),
            document: seal.head.clone(),
            heading_before,
            exports: seal.head_exports.clone(),
        }
    }
}

pub(crate) struct Parser {
    jobs: flume::Sender<Job>,
    done: flume::Receiver<Done>,
}

/// Called on the main thread when results are waiting.
static WAKE: OnceLock<fn()> = OnceLock::new();
static QUEUED: AtomicBool = AtomicBool::new(false);

impl Parser {
    /// Starts the worker. `wake` runs on the main run loop after results
    /// arrive, at most once per batch.
    pub(crate) fn start(wake: fn()) -> Parser {
        let _ = WAKE.set(wake);
        let (jobs, job_receiver) = flume::unbounded::<Job>();
        let (done_sender, done) = flume::unbounded();
        std::thread::Builder::new()
            .name("omperor-transcript-parse".into())
            .spawn(move || work(job_receiver, done_sender))
            .expect("the transcript parser starts");
        Parser { jobs, done }
    }

    pub(crate) fn submit(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    pub(crate) fn try_next(&self) -> Option<Done> {
        self.done.try_recv().ok()
    }
}

/// A streamed row's last tail parse on the worker.
struct Previous {
    start: usize,
    document: Arc<ParsedDocument>,
    /// What the tail offered the sealed segments.
    exports: Exports,
}

fn work(jobs: flume::Receiver<Job>, done: flume::Sender<Done>) {
    let mut previous: HashMap<u64, Previous> = HashMap::new();
    while let Ok(first) = jobs.recv() {
        // Everything queued meanwhile, with only the newest text of a
        // stream: a stream's parse is of the whole text, so an older one is
        // work nobody will see.
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        let mut latest: HashMap<u64, usize> = HashMap::new();
        for (index, job) in batch.iter().enumerate() {
            if let Job::Stream { row, .. } = job {
                latest.insert(*row, index);
            }
        }
        let mut reset_rows: Vec<u64> = Vec::new();
        for (index, job) in batch.into_iter().enumerate() {
            let answer = match job {
                Job::Forget { row } => {
                    previous.remove(&row);
                    continue;
                }
                Job::Segment { row, generation, text } => Done::Segmented { row, generation, segments: segment(&text) },
                Job::Stream { row, generation, start, text, reset, heading_before, sealed } => {
                    if reset {
                        reset_rows.push(row);
                    }
                    if latest.get(&row) != Some(&index) {
                        continue;
                    }
                    if reset_rows.contains(&row) || previous.get(&row).is_some_and(|old| old.start != start) {
                        previous.remove(&row);
                    }
                    let (old_document, old_exports) = match previous.remove(&row) {
                        Some(old) => (Some(old.document), Some(old.exports)),
                        None => (None, None),
                    };
                    let step = stream_step(start, &text, heading_before, &sealed, old_exports.as_ref(), LIMITS);
                    stream_answer(row, generation, start, text, old_document, step, &mut previous)
                }
            };
            if done.send(answer).is_err() {
                return;
            }
            wake();
        }
    }
}

/// A stream step as the answer the main thread applies; `previous` keeps
/// the parse of the tail the answer leaves.
fn stream_answer(
    row: u64,
    generation: u64,
    start: usize,
    text: String,
    old: Option<Arc<ParsedDocument>>,
    step: Step,
    previous: &mut HashMap<u64, Previous>,
) -> Done {
    if let Some(joined) = step.joined {
        previous.insert(row, Previous { start: joined.start, document: step.document.clone(), exports: step.exports });
        return Done::Streamed {
            row,
            generation,
            start: joined.start,
            text: joined.text,
            document: step.document,
            dirty: DirtySet::wholesale(),
            seal: None,
            updates: step.updates,
            unseal: Some(joined.index),
        };
    }
    let seal = step.seal.map(|plan| Seal {
        units: plan.units,
        bytes: plan.bytes,
        head: plan.head,
        head_heading: plan.head_heading,
        head_exports: Arc::new(plan.head_exports),
        rest: plan.rest,
    });
    let dirty = match &seal {
        Some(_) => DirtySet::wholesale(),
        None => ASTDiff::dirty_set(old.as_deref(), &step.document),
    };
    let kept = match &seal {
        Some(seal) => Previous { start: start + seal.units, document: seal.rest.clone(), exports: step.exports },
        None => Previous { start, document: step.document.clone(), exports: step.exports },
    };
    previous.insert(row, kept);
    Done::Streamed { row, generation, start, text, document: step.document, dirty, seal, updates: step.updates, unseal: None }
}

fn wake() {
    if QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(run_loop) = CFRunLoop::main() else { return };
    let block = RcBlock::new(|| {
        // Cleared before the drain: a result sent during it wakes again.
        QUEUED.store(false, Ordering::Release);
        if let Some(wake) = WAKE.get() {
            wake();
        }
    });
    // SAFETY: CFRunLoopPerformBlock copies the block and may be called from
    // any thread; the block captures nothing and runs on the main run loop.
    unsafe {
        run_loop.perform_block(kCFRunLoopCommonModes.map(|mode| mode.as_ref()), Some(&block));
    }
    run_loop.wake_up();
}

// MARK: - Finished messages

/// `text` as the views that show it: one segment up to `WHOLE_LIMIT`,
/// else cut between top-level blocks wherever the cut cannot show.
pub(crate) fn segment(text: &str) -> Vec<Segment> {
    segment_with(text, LIMITS)
}

fn segment_with(text: &str, limits: Limits) -> Vec<Segment> {
    let document = MarkdownParser::parse(text);
    let length = document.utf16.len();
    let whole = |document: Arc<ParsedDocument>| {
        vec![Segment { text: text.to_owned(), document, heading_before: false, start: 0, length }]
    };
    if length <= limits.whole || length > EXTENSION_LIMIT {
        return whole(document);
    }
    let pieces = plan(&document, text, limits);
    if pieces.len() <= 1 {
        return whole(document);
    }
    pieces
        .into_iter()
        .map(|piece| Segment {
            heading_before: invariant::heading_before(&document, piece.start),
            text: piece.text,
            document: piece.document,
            start: piece.start,
            length: piece.end - piece.start,
        })
        .collect()
}

/// A segment's text and parse, checked against the message.
struct Built {
    start: usize,
    end: usize,
    text: String,
    document: Arc<ParsedDocument>,
}

/// Where a finished message is cut, and its segments: at the line start of
/// top-level blocks, once the segment so far is at least `SEGMENT_TARGET`
/// long, where the segment before the cut presents its text as the message
/// whole does, and so does the start of the rest. The last segment is
/// checked too; one that fails joins the segment before it.
fn plan(document: &ParsedDocument, text: &str, limits: Limits) -> Vec<Built> {
    let children = &document.root.children;
    let length = document.utf16.len();
    let starts: Vec<usize> = children.iter().map(|block| line_start(document, block.range.location)).collect();
    let bytes = byte_offsets(text, &starts);
    let byte_at = |units: usize| -> usize {
        if units >= length {
            return text.len();
        }
        starts.binary_search(&units).map(|index| bytes[index]).unwrap_or_else(|_| utf16_to_byte(text, units))
    };
    let whole = WholeMessage::of(document, &starts);
    let build = |start: usize, end: usize| -> Option<Built> {
        let own = &text[byte_at(start)..byte_at(end)];
        let mine = exports(&MarkdownParser::parse(own));
        let (before, after) = (whole.region(0, start), whole.region(end, length));
        let context = context_for(&[&before], Some(&mine), &[&after], &own.to_lowercase());
        let parsed = MarkdownParser::parse_segment(own, &context);
        let expected = Part { document, start, end, continues: false };
        let part = Part { document: &parsed, start: 0, end: end - start, continues: invariant::heading_before(document, start) };
        invariant::verify(&expected, &part).ok()?;
        Some(Built { start, end, text: own.to_owned(), document: parsed })
    };
    let mut built: Vec<Built> = Vec::new();
    let mut current = 0usize;
    for index in 1..children.len() {
        let start = starts[index];
        if start <= current || start - current < limits.target {
            continue;
        }
        // The rest must be worth a view of its own.
        if length - start < limits.target / 4 {
            break;
        }
        if !boundary_allowed(document, text, index, start, byte_at(start)) {
            continue;
        }
        let Some(head) = build(current, start) else { continue };
        // The rest's start, up to where its own first cut could be: a cut
        // whose rest starts differently would only be joined again later.
        let window = starts.iter().copied().find(|&next| next >= start + limits.target).unwrap_or(length);
        if build(start, window).is_none() {
            continue;
        }
        built.push(head);
        current = start;
    }
    loop {
        if let Some(last) = build(current, length) {
            built.push(last);
            break;
        }
        match built.pop() {
            Some(previous) => current = previous.start,
            None => {
                // One view of the whole message is the message itself.
                built.push(Built { start: 0, end: length, text: text.to_owned(), document: MarkdownParser::parse(text) });
                break;
            }
        }
    }
    built
}

/// What a finished message's parts offer each other, by where they lie.
struct WholeMessage {
    /// Its definitions: one per label, where the message keeps it.
    definitions: Vec<Definition>,
    /// Every label it keeps, with where its definition starts.
    keys: Vec<(String, usize)>,
    /// The first `<details>` opening tag ends before this offset; the last
    /// closing tag starts at or after this one. A tag lies within a line,
    /// so none spans a cut.
    first_opening: Option<usize>,
    last_closing: Option<usize>,
}

impl WholeMessage {
    fn of(document: &ParsedDocument, starts: &[usize]) -> WholeMessage {
        let text = &document.utf16;
        let (opens, closes) = SafeHTMLParser::details_tags(text);
        let mut bounds: Vec<usize> = starts.to_vec();
        bounds.push(text.len());
        // Whether an opening tag lies before `bounds[k]` rises with k;
        // whether a closing tag lies from `bounds[k]` on, falls.
        let first_opening = opens.then(|| {
            let k = bounds.partition_point(|&at| !SafeHTMLParser::details_tags(&text[..at]).0);
            bounds[k.min(bounds.len() - 1)]
        });
        let last_closing = closes.then(|| {
            let k = bounds.partition_point(|&at| SafeHTMLParser::details_tags(&text[at..]).1);
            bounds[k.saturating_sub(1)]
        });
        WholeMessage { definitions: definitions(document), keys: keys(document), first_opening, last_closing }
    }

    /// What the part from `start` to `end` offers the rest.
    fn region(&self, start: usize, end: usize) -> Exports {
        let inside = |at: usize| start <= at && at < end;
        Exports {
            definitions: self.definitions.iter().filter(|definition| inside(definition.start)).cloned().collect(),
            keys: self.keys.iter().filter(|(_, at)| inside(*at)).map(|(key, _)| key.clone()).collect(),
            opens: self.first_opening.is_some_and(|at| at <= end && start == 0),
            closes: self.last_closing.is_some_and(|at| at >= start && end >= at),
        }
    }
}

/// The layout rule for a cut before top-level block `index`, at `start`
/// (byte `byte` of `text`). A view drops the space above its first
/// paragraph, and the host puts it back above the view; that is exact for
/// lines of text, not for what a fragment draws from its frame (a code or
/// callout band, a quote's bar, a table, a centered diagram, formula, image
/// or rule), so a segment opens with prose: a paragraph, a heading, a list
/// or HTML. A blank line comes before the cut, so the segment before ends
/// with it, not with a block whose frame would take the empty line TextKit
/// lays out after a final line break. A heading stays with the block after
/// it, at any depth, as does front matter; and a rest that opens with a
/// `---` line could be front matter on its own.
fn boundary_allowed(document: &ParsedDocument, text: &str, index: usize, start: usize, byte: usize) -> bool {
    let children = &document.root.children;
    let opens_with_prose = match &children[index].content {
        BlockContent::Paragraph => !children[index].inlines.iter().any(|inline| matches!(inline.kind, InlineKind::Image { .. })),
        BlockContent::Heading { .. } | BlockContent::List { .. } | BlockContent::HtmlBlock => true,
        _ => false,
    };
    if !opens_with_prose {
        return false;
    }
    if !blank_line_before(&text[..byte]) {
        return false;
    }
    if matches!(children[index - 1].content, BlockContent::Heading { .. } | BlockContent::FrontMatter(_)) {
        return false;
    }
    if invariant::ends_with_heading(document, start) {
        return false;
    }
    first_line(&text[byte..]).trim() != "---"
}

/// Whether `before` (the text up to a cut, a line start) ends with a line
/// holding nothing but whitespace.
fn blank_line_before(before: &str) -> bool {
    let Some(body) = before.strip_suffix('\n') else { return false };
    let line = body.rsplit('\n').next().unwrap_or(body);
    line.trim().is_empty()
}

fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or("")
}

/// The start of the line holding `location`: a block indented by a few
/// spaces starts after them, and a cut must not leave them behind.
fn line_start(document: &ParsedDocument, location: isize) -> usize {
    let index = document.line_starts.partition_point(|&start| start <= location);
    document.line_starts.get(index.wrapping_sub(1)).map_or(0, |&start| start.max(0) as usize)
}

// MARK: - Streaming

/// What the worker makes of one parse of a streamed tail.
struct Step {
    document: Arc<ParsedDocument>,
    /// What the tail offers the sealed segments.
    exports: Exports,
    seal: Option<SealPlan>,
    updates: Vec<SealedUpdate>,
    /// Sealed segments joined to the tail again.
    joined: Option<Joined>,
}

struct Joined {
    /// The first sealed segment joined.
    index: usize,
    /// Where the joined tail starts in the message, and its text.
    start: usize,
    text: String,
}

struct SealPlan {
    units: usize,
    bytes: usize,
    head: Arc<ParsedDocument>,
    head_heading: bool,
    head_exports: Exports,
    rest: Arc<ParsedDocument>,
}

/// One parse of a streamed tail `text` at `start`, after the `sealed`
/// segments (with a heading among them: `heading_before`). When what the
/// tail offers the sealed segments differs from `previous` (or there is no
/// previous parse), each sealed segment is parsed again if its context
/// changed, and every sealed segment is checked against the message whole;
/// the first that fails is joined to the tail again, with all after it.
/// Otherwise the tail may be sealed.
fn stream_step(
    start: usize,
    text: &str,
    heading_before: bool,
    sealed: &[SealedSource],
    previous: Option<&Exports>,
    limits: Limits,
) -> Step {
    let earlier: Vec<&Exports> = sealed.iter().map(|source| &*source.exports).collect();
    let document = MarkdownParser::parse_segment(text, &context_for(&earlier, None, &[], &text.to_lowercase()));
    // The tail is last: nothing supersedes its definitions.
    let exports = exports(&document);
    let mut updates = Vec::new();
    let too_long = start + document.utf16.len() > EXTENSION_LIMIT;
    if !sealed.is_empty() && (previous != Some(&exports) || too_long) {
        let mut shown: Vec<Arc<ParsedDocument>> = Vec::with_capacity(sealed.len());
        for (index, source) in sealed.iter().enumerate() {
            let mut later: Vec<&Exports> = earlier[index + 1..].to_vec();
            later.push(&exports);
            let context = context_for(&earlier[..index], Some(&source.exports), &later, &source.lowered);
            if context == source.document.segment_context {
                shown.push(source.document.clone());
                continue;
            }
            let parsed = MarkdownParser::parse_segment(&source.own, &context);
            updates.push(SealedUpdate { index, dirty: ASTDiff::dirty_set(Some(&source.document), &parsed), document: parsed.clone() });
            shown.push(parsed);
        }
        // The message whole, to check every sealed segment against.
        let mut message = String::with_capacity(sealed.iter().map(|source| source.own.len()).sum::<usize>() + text.len());
        for source in sealed {
            message.push_str(&source.own);
        }
        message.push_str(text);
        let whole = MarkdownParser::parse(&message);
        let lengths: Vec<usize> = sealed.iter().map(|source| source.own.encode_utf16().count()).collect();
        let mut at = 0usize;
        for (index, source) in sealed.iter().enumerate() {
            let expected = Part { document: &whole, start: at, end: at + lengths[index], continues: false };
            let part = Part { document: &shown[index], start: 0, end: lengths[index], continues: source.heading_before };
            if too_long || invariant::verify(&expected, &part).is_err() {
                updates.retain(|update| update.index < index);
                let joined: String = sealed[index..].iter().map(|source| &*source.own).chain(std::iter::once(text)).collect();
                let context = context_for(&earlier[..index], None, &[], &joined.to_lowercase());
                let document = MarkdownParser::parse_segment(&joined, &context);
                let exports = self::exports(&document);
                let back: usize = lengths[index..].iter().sum();
                return Step { document, exports, seal: None, updates, joined: Some(Joined { index, start: start - back, text: joined }) };
            }
            at += lengths[index];
        }
    }
    let seal = if too_long { None } else { seal_point(&document, text, heading_before, start == 0, &earlier, limits) };
    Step { document, exports, seal, updates, joined: None }
}

/// Where a streamed tail is sealed, if it is long enough: at the line start
/// of the latest top-level block that starts at least `SEGMENT_TARGET` in
/// and whose first line is complete, so the stream can no longer change
/// where it starts — and not, at the start of the message, while its first
/// line is a `---` that later text could close as front matter — where head
/// and rest each present their text as the tail whole does, each with what
/// the rest of the message gives it.
fn seal_point(
    document: &ParsedDocument,
    text: &str,
    heading_before: bool,
    at_start: bool,
    earlier: &[&Exports],
    limits: Limits,
) -> Option<SealPlan> {
    let length = document.utf16.len();
    if length < 2 * limits.target {
        return None;
    }
    if at_start && first_line(text).trim() == "---" && document.front_matter.is_none() {
        return None;
    }
    let children = &document.root.children;
    let mut tried = 0;
    for index in (1..children.len()).rev() {
        let start = line_start(document, children[index].range.location);
        if start < limits.target || tried == SEAL_CANDIDATES {
            break;
        }
        tried += 1;
        // A block whose first line is still growing may yet turn out to
        // continue the one before (`#` becoming `#x`).
        if !document.utf16[start..].contains(&0x0A) {
            continue;
        }
        let bytes = utf16_to_byte(text, start);
        if !boundary_allowed(document, text, index, start, bytes) {
            continue;
        }
        let (own, rest_text) = text.split_at(bytes);
        let head_exports = exports(&MarkdownParser::parse(own));
        let rest_exports = exports(&MarkdownParser::parse(rest_text));
        let head_context = context_for(earlier, Some(&head_exports), &[&rest_exports], &own.to_lowercase());
        let head = MarkdownParser::parse_segment(own, &head_context);
        let whole = Part { document, start: 0, end: start, continues: heading_before };
        if invariant::verify(&whole, &Part { document: &head, start: 0, end: start, continues: heading_before }).is_err() {
            continue;
        }
        let head_heading = invariant::heading_before(document, start);
        let mut before = earlier.to_vec();
        before.push(&head_exports);
        let rest = MarkdownParser::parse_segment(rest_text, &context_for(&before, None, &[], &rest_text.to_lowercase()));
        let whole = Part { document, start, end: length, continues: heading_before };
        let part = Part { document: &rest, start: 0, end: length - start, continues: heading_before || head_heading };
        if invariant::verify(&whole, &part).is_err() {
            continue;
        }
        return Some(SealPlan { units: start, bytes, head, head_heading, head_exports, rest });
    }
    None
}

// MARK: - Definitions

/// A link reference or footnote definition, as the rest of a message gives
/// it to a segment that cites it.
#[derive(Clone, PartialEq)]
pub(crate) struct Definition {
    /// Where it starts, in UTF-16 units of its document.
    start: usize,
    /// The label, lowercased, or `^` and the footnote's identifier.
    key: String,
    /// What a citation of it contains: `[label` or `[^id]`, lowercased.
    needle: String,
    /// Its first line of source, which cmark reads.
    line: String,
    /// A footnote's identifier and text.
    footnote: Option<(String, String)>,
    /// cmark reads its line as a definition, not as text of a paragraph.
    read_as_definition: bool,
}

/// The definitions of `document` that a view of it resolves, in order:
/// footnotes and link references, as the scanner finds them. One cmark
/// reads as text (a line that continues a paragraph) resolves nothing in
/// the message, and a segment given it fails the check.
fn definitions(document: &ParsedDocument) -> Vec<Definition> {
    let line = |location: isize| {
        let start = location.max(0) as usize;
        let end = document.utf16[start..].iter().position(|&unit| unit == 0x0A).map_or(document.utf16.len(), |at| start + at);
        String::from_utf16_lossy(&document.utf16[start..end])
    };
    let mut out = Vec::new();
    for (label, reference) in &document.link_references {
        out.push(Definition {
            start: reference.range.location.max(0) as usize,
            key: label.clone(),
            needle: format!("[{}", label.to_lowercase()),
            line: line(reference.range.location),
            footnote: None,
            read_as_definition: true,
        });
    }
    // Every footnote the scanner finds is one a reference may cite, though
    // consecutive definition lines are one block, kept under the last
    // identifier: only an identifier the document keeps has a tool tip.
    let scanner = SourceScanner::new(&SourceMap::new(&document.text));
    for definition in &scanner.footnote_definitions {
        let identifier = &definition.identifier;
        let text = document.footnotes.get(identifier).map_or_else(String::new, |block| document.substring(block.content_range));
        // cmark took the line as a link reference definition (its text is
        // one word, at a paragraph's start) when no paragraph holds it: the
        // footnote's block is then made for it, without text of its own.
        let location = definition.range.location;
        let read_as_definition = document
            .root
            .children
            .iter()
            .find(|block| block.range.location <= location && location < block.range.upper_bound())
            .is_none_or(|block| matches!(block.content, BlockContent::FootnoteDefinition { .. }) && block.inlines.is_empty() && block.children.is_empty());
        out.push(Definition {
            start: location.max(0) as usize,
            key: format!("^{identifier}"),
            needle: format!("[^{}]", identifier.to_lowercase()),
            line: line(location),
            footnote: Some((identifier.clone(), text)),
            read_as_definition,
        });
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.key.cmp(&b.key)));
    out
}

/// Every label `document` keeps a definition for, with where it starts:
/// link reference labels as its keys, `^` and each footnote identifier.
fn keys(document: &ParsedDocument) -> Vec<(String, usize)> {
    let references = document.link_references.iter().map(|(label, reference)| (label.clone(), reference.range.location));
    let footnotes = document.footnotes.iter().map(|(identifier, block)| (format!("^{identifier}"), block.range.location));
    references.chain(footnotes).map(|(key, at)| (key, at.max(0) as usize)).collect()
}

/// What a part of a message offers the rest, from a parse of it on its own.
fn exports(document: &ParsedDocument) -> Exports {
    let (opens, closes) = SafeHTMLParser::details_tags(&document.utf16);
    Exports { definitions: definitions(document), keys: keys(document).into_iter().map(|(key, _)| key).collect(), opens, closes }
}

/// A part's context, from what the parts `before` and `after` it offer and
/// what it offers itself (`own`, unknown for a tail still growing, which
/// nothing follows): the link references it cites (`lowered` is its text
/// lowercased), each from where its label is first defined, which wins over
/// the part's own when it comes before; the footnotes it cites, with the
/// text the message keeps, its last; the labels the message defines again
/// after it; and the `<details>` tags around it.
fn context_for(before: &[&Exports], own: Option<&Exports>, after: &[&Exports], lowered: &str) -> SegmentContext {
    let mine = own.map(|own| &own.definitions);
    let parts = || before.iter().map(|part| &part.definitions).chain(mine).chain(after.iter().map(|part| &part.definitions));
    let is_mine = |definition: &Definition| mine.is_some_and(|mine| mine.iter().any(|other| std::ptr::eq(other, definition)));
    let mut context = SegmentContext::default();
    let mut seen: Vec<&str> = Vec::new();
    // cmark resolves a reference to the first line it read as a definition.
    // A footnote's line is one only when its text is one word, at a
    // paragraph's start (consecutive definition lines make one paragraph,
    // whose lines after the first are text); otherwise its identifier is
    // enough for its references.
    for definition in parts().flat_map(|part| part.iter()).filter(|definition| definition.read_as_definition) {
        if seen.contains(&definition.key.as_str()) {
            continue;
        }
        seen.push(&definition.key);
        if !is_mine(definition) && lowered.contains(&definition.needle) {
            context.references.push(definition.line.clone());
        }
    }
    let mut last: Vec<&Definition> = Vec::new();
    for definition in parts().flat_map(|part| part.iter()).filter(|definition| definition.footnote.is_some()) {
        last.retain(|other| other.key != definition.key);
        last.push(definition);
    }
    for definition in last {
        if !is_mine(definition) && lowered.contains(&definition.needle) {
            context.footnotes.push(definition.footnote.clone().expect("a footnote"));
        }
    }
    if let Some(own) = own {
        for key in &own.keys {
            if after.iter().any(|part| part.keys.contains(key)) {
                match key.strip_prefix('^') {
                    Some(identifier) => context.superseded_footnotes.push(identifier.to_owned()),
                    None => context.superseded_references.push(key.clone()),
                }
            }
        }
    }
    context.details_opened_before = before.iter().any(|part| part.opens);
    context.details_closed_after = after.iter().any(|part| part.closes);
    context
}

// MARK: - Offsets

/// The byte offset of UTF-16 offset `units` in `text`.
pub(crate) fn utf16_to_byte(text: &str, units: usize) -> usize {
    let mut counted = 0;
    for (byte, character) in text.char_indices() {
        if counted >= units {
            return byte;
        }
        counted += character.len_utf16();
    }
    text.len()
}

/// The byte offsets of the sorted UTF-16 offsets `units` in `text`.
fn byte_offsets(text: &str, units: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(units.len());
    let mut wanted = units.iter().copied().peekable();
    let mut counted = 0;
    for (byte, character) in text.char_indices() {
        while wanted.peek().is_some_and(|&next| next <= counted) {
            wanted.next();
            out.push(byte);
        }
        counted += character.len_utf16();
    }
    out.extend(wanted.map(|_| text.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stress(copies: usize) -> String {
        let mut text = String::new();
        for _ in 0..copies {
            text.push_str(crate::mock_inference::STRESS_DOCUMENT);
            text.push_str("\n\n");
        }
        text
    }

    /// Small sizes, so short documents are cut often.
    const SMALL: Limits = Limits { whole: 0, target: 96 };
    const MEDIUM: Limits = Limits { whole: 0, target: 400 };

    /// Every segment of `text` covers its slice in order and presents it as
    /// one view of the whole message does, checked afresh.
    fn check_segments(name: &str, text: &str, segments: &[Segment]) {
        let whole = MarkdownParser::parse(text);
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let mut at = 0;
        for piece in segments {
            assert_eq!(piece.start, at, "{name}: segments are contiguous");
            let own = String::from_utf16_lossy(&utf16[piece.start..piece.start + piece.length]);
            assert_eq!(piece.text, own, "{name}: a segment's text is its slice");
            assert_eq!(piece.document.text, own, "{name}: a segment's parse is of its slice");
            assert_eq!(piece.heading_before, invariant::heading_before(&whole, piece.start), "{name}");
            let expected = Part { document: &whole, start: piece.start, end: piece.start + piece.length, continues: false };
            let part = Part { document: &piece.document, start: 0, end: piece.length, continues: piece.heading_before };
            if let Err(error) = invariant::verify(&expected, &part) {
                panic!("{name}: the segment at {} shows: {error}", piece.start);
            }
            at += piece.length;
        }
        assert_eq!(at, utf16.len(), "{name}: segments cover the message");
    }

    fn corpus() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/upleft/corpus/generated");
        let mut out = Vec::new();
        for folder in ["docs", "fixtures", "agent", "spec"] {
            let mut paths: Vec<_> = std::fs::read_dir(root.join(folder)).expect("Upleft's generated corpus: run `just test`, or `python3 scripts/build-corpus.py` in vendor/upleft").flatten().map(|entry| entry.path()).collect();
            paths.sort();
            for path in paths.into_iter().filter(|path| path.extension().is_some_and(|extension| extension == "md")) {
                let text = std::fs::read_to_string(&path).expect("a corpus document");
                out.push((format!("{folder}/{}", path.file_name().unwrap().to_string_lossy()), text));
            }
        }
        assert!(out.len() > 800, "{} corpus documents", out.len());
        out
    }

    /// A deterministic generator for mutations and chunk sizes.
    struct Seeded(u64);

    impl Seeded {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound.max(1) as u64) as usize
        }
    }

    /// `text` with a few lines changed in ways that reach across blocks.
    fn mutate(text: &str, seed: u64) -> String {
        const INSERTS: [&str; 14] = [
            "---",
            "<details>",
            "</details>",
            "<details open>\n<summary>More</summary>",
            "[^1]: A late note.",
            "[docs]: https://example.com/docs",
            "> lazy quote",
            "```",
            "```mermaid",
            "# Heading",
            "- item",
            "1. first",
            "$$",
            "",
        ];
        let mut random = Seeded(seed);
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        for _ in 0..1 + random.below(4) {
            let at = random.below(lines.len() + 1);
            match random.below(4) {
                0 if at < lines.len() => {
                    lines.remove(at);
                }
                1 if at < lines.len() => {
                    let copy = lines[at].clone();
                    lines.insert(at, copy);
                }
                _ => lines.insert(at, INSERTS[random.below(INSERTS.len())].to_owned()),
            }
        }
        lines.join("\n")
    }

    #[test]
    fn a_short_message_is_one_view() {
        let segments = segment("Hello, **world**.");
        assert_eq!(segments.len(), 1);
        assert!(!segments[0].heading_before);
        assert_eq!(segments[0].text, "Hello, **world**.");
    }

    #[test]
    fn a_long_message_is_cut_between_blocks_and_covers_the_text() {
        let text = stress(8);
        let segments = segment(&text);
        assert!(segments.len() > 4, "{} segments", segments.len());
        check_segments("stress × 8", &text, &segments);
        // A heading is never the last block of a segment.
        for piece in &segments[..segments.len() - 1] {
            let last = piece.document.root.children.iter().filter(|block| block.range.location < piece.length as isize).last();
            assert!(!matches!(last.map(|block| &block.content), Some(BlockContent::Heading { .. })));
        }
    }

    #[test]
    fn a_segment_carries_the_definitions_it_cites() {
        let mut text = String::from("See the note[^n] and [the docs][docs].\n\n");
        for index in 0..400 {
            text.push_str(&format!("Paragraph {index} of filler text that runs on for a while.\n\n"));
        }
        text.push_str("[^n]: The note.\n\n[docs]: https://example.com/docs\n");
        let segments = segment(&text);
        assert!(segments.len() > 2);
        check_segments("definitions", &text, &segments);
        let first = &segments[0].document.segment_context;
        assert!(!first.references.iter().any(|line| line.starts_with("[^n]")));
        assert!(first.references.iter().any(|line| line == "[docs]: https://example.com/docs"));
        assert_eq!(first.footnotes, vec![("n".to_owned(), "The note.".to_owned())]);
        // A segment that cites nothing gets nothing.
        assert!(segments[1].document.segment_context.is_empty());
    }

    #[test]
    fn every_cut_of_the_corpus_and_the_stress_document_is_invisible() {
        let mut documents = corpus();
        let samples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/markdown-conformance");
        for entry in std::fs::read_dir(samples).expect("the chat samples").flatten() {
            let text = std::fs::read_to_string(entry.path()).expect("a chat sample");
            documents.push((entry.file_name().to_string_lossy().into_owned(), text));
        }
        documents.push(("stress".into(), stress(1)));
        documents.push(("stress × 2".into(), stress(2)));
        documents.push(("stress × 4".into(), stress(4)));
        let mut cuts = 0;
        for (name, text) in &documents {
            for limits in [LIMITS, MEDIUM, SMALL] {
                let segments = segment_with(text, limits);
                cuts += segments.len() - 1;
                check_segments(name, text, &segments);
            }
        }
        assert!(cuts > 1_000, "{cuts} cuts");
    }

    #[test]
    fn every_cut_of_mutated_documents_is_invisible() {
        let base = stress(2);
        let mut sources = vec![base];
        for (_, text) in corpus().into_iter().filter(|(name, _)| name.starts_with("docs/")).take(8) {
            sources.push(text);
        }
        for (index, source) in sources.iter().enumerate() {
            for seed in 0..40 {
                let text = mutate(source, seed * 31 + index as u64);
                for limits in [MEDIUM, SMALL] {
                    check_segments(&format!("mutation {index}/{seed}"), &text, &segment_with(&text, limits));
                }
            }
        }
    }

    fn filler(paragraphs: usize) -> String {
        (0..paragraphs).map(|index| format!("Paragraph {index} of filler text that runs on for a while, as prose does.\n\n")).collect()
    }

    /// Adversarial messages: constructs a cut could separate.
    fn adversarial() -> Vec<(&'static str, String)> {
        let details = format!("Intro.\n\n<details>\n<summary>Long</summary>\n\n{}</details>\n\n{}", filler(60), filler(30));
        let late_definitions = format!("See [the docs][docs] and the note[^n].\n\n{}[^n]: The note.\n\n[docs]: https://example.com\n", filler(80));
        let early_definitions = format!("[docs]: https://example.com\n\n[^n]: The note.\n\n{}See [the docs][docs] and[^n].\n\n", filler(80));
        let list: String = (0..300).map(|index| format!("- item {index} of a long list\n")).collect::<String>() + "\nAfter.\n";
        let quote = format!("> A quote\nwith lazy lines\nthat run on\n\n{}> Another\nlazy\n\n{}", filler(30), filler(30));
        let code = format!("{}```rust\n{}```\n\n{}", filler(10), "let x = 1; // a line of code\n".repeat(400), filler(10));
        let open_mermaid = format!("[docs]: https://example.com\n\n{}See [docs].\n\n```mermaid\ngraph TD\n  A --> B\n", filler(60));
        let stray_closer = format!("<details>\n<summary>S</summary>\n\nBody.\n\n</details>\n\n{}</details>\n\n{}", filler(40), filler(20));
        let front_matter_later = format!("{}---\ntitle: not front matter\n---\n\n{}", filler(30), filler(30));
        let adjacent_heading = format!("{}- # Heading in a list\n\n# Heading after\n\n{}", filler(30), filler(30));
        vec![
            ("details", details),
            ("late definitions", late_definitions),
            ("early definitions", early_definitions),
            ("long list", list),
            ("lazy quote", quote),
            ("huge code block", code),
            ("open mermaid", open_mermaid),
            ("stray closer", stray_closer),
            ("front matter later", front_matter_later),
            ("adjacent heading", adjacent_heading),
        ]
    }

    #[test]
    fn adversarial_messages_are_cut_only_where_it_cannot_show() {
        for (name, text) in adversarial() {
            for limits in [LIMITS, MEDIUM, SMALL] {
                check_segments(name, &text, &segment_with(&text, limits));
            }
        }
        // `<details>` and its closing tag may be in two views: each pairs
        // with the other through its context.
        let (_, text) = adversarial().into_iter().find(|(name, _)| *name == "details").unwrap();
        let segments = segment_with(&text, SMALL);
        let opening = segments.iter().position(|piece| piece.text.contains("<details>")).unwrap();
        let closing = segments.iter().position(|piece| piece.text.contains("</details>")).unwrap();
        assert!(opening < closing);
        assert!(segments[opening].document.segment_context.details_closed_after);
        assert!(segments[closing].document.segment_context.details_opened_before);
        // A huge code block is one view, not cut.
        let (_, text) = adversarial().into_iter().find(|(name, _)| *name == "huge code block").unwrap();
        assert!(segment_with(&text, SMALL).iter().any(|piece| piece.text.contains("```rust") && piece.text.matches("let x").count() == 400));
        // A citation before a fence left open resolves in its own view.
        let (_, text) = adversarial().into_iter().find(|(name, _)| *name == "open mermaid").unwrap();
        let segments = segment_with(&text, SMALL);
        let citing = segments.iter().find(|piece| piece.text.contains("See [docs].")).unwrap();
        assert!(citing.start > 0 && citing.document.segment_context.references == vec!["[docs]: https://example.com".to_owned()]);
    }

    /// The main thread's side of a streamed message, as `transcript_stack`
    /// applies the worker's answers.
    struct Stream {
        limits: Limits,
        /// Sealed: source, start (UTF-16).
        sealed: Vec<(SealedSource, usize)>,
        tail_start: usize,
        tail_byte: usize,
        heading_before: bool,
        tail: Option<Arc<ParsedDocument>>,
        exports: Option<Exports>,
        seals: usize,
        updates: usize,
        unseals: usize,
    }

    impl Stream {
        fn new(limits: Limits) -> Stream {
            Stream {
                limits,
                sealed: Vec::new(),
                tail_start: 0,
                tail_byte: 0,
                heading_before: false,
                tail: None,
                exports: None,
                seals: 0,
                updates: 0,
                unseals: 0,
            }
        }

        fn step(&mut self, body: &str) {
            let sources: Vec<SealedSource> = self.sealed.iter().map(|(source, _)| source.clone()).collect();
            let tail = &body[self.tail_byte..];
            let step = stream_step(self.tail_start, tail, self.heading_before, &sources, self.exports.as_ref(), self.limits);
            for update in step.updates {
                self.updates += 1;
                self.sealed[update.index].0.document = update.document;
            }
            self.exports = Some(step.exports);
            if let Some(joined) = step.joined {
                self.unseals += 1;
                self.heading_before = self.sealed[joined.index].0.heading_before;
                self.tail_byte = self.sealed[..joined.index].iter().map(|(source, _)| source.own.len()).sum();
                self.sealed.truncate(joined.index);
                self.tail_start = joined.start;
                assert_eq!(joined.text, body[self.tail_byte..]);
                self.tail = Some(step.document);
                return;
            }
            match step.seal {
                Some(plan) => {
                    self.seals += 1;
                    let seal = Seal {
                        units: plan.units,
                        bytes: plan.bytes,
                        head: plan.head,
                        head_heading: plan.head_heading,
                        head_exports: Arc::new(plan.head_exports),
                        rest: plan.rest,
                    };
                    let source = SealedSource::sealed(&tail[..seal.bytes], &seal, self.heading_before);
                    self.sealed.push((source, self.tail_start));
                    self.heading_before |= seal.head_heading;
                    self.tail_start += seal.units;
                    self.tail_byte += seal.bytes;
                    self.tail = Some(seal.rest);
                }
                None => self.tail = Some(step.document),
            }
        }

        /// Every view presents its text as one view of `body` does.
        fn check(&self, name: &str, body: &str) {
            let whole = MarkdownParser::parse(body);
            let length = whole.utf16.len();
            for (index, (source, start)) in self.sealed.iter().enumerate() {
                let own = source.own.encode_utf16().count();
                assert_eq!(source.document.text, *source.own);
                let expected = Part { document: &whole, start: *start, end: start + own, continues: false };
                let part = Part { document: &source.document, start: 0, end: own, continues: source.heading_before };
                if let Err(error) = invariant::verify(&expected, &part) {
                    panic!("{name}: sealed segment {index} at {start} of {length} shows: {error}");
                }
            }
            let tail = self.tail.as_ref().expect("a tail parse");
            assert_eq!(tail.text, body[self.tail_byte..]);
            let expected = Part { document: &whole, start: self.tail_start, end: length, continues: false };
            let part = Part { document: tail, start: 0, end: length - self.tail_start, continues: self.heading_before };
            if let Err(error) = invariant::verify(&expected, &part) {
                panic!("{name}: the tail at {} of {length} shows: {error}", self.tail_start);
            }
        }
    }

    /// Streams `text` in appends of 1 to `chunk` characters, checking every
    /// view after each; returns the stream.
    fn stream(name: &str, text: &str, limits: Limits, chunk: usize, seed: u64) -> Stream {
        let characters: Vec<(usize, char)> = text.char_indices().collect();
        let mut random = Seeded(seed);
        let mut stream = Stream::new(limits);
        let mut at = 0;
        while at < characters.len() {
            at = (at + 1 + random.below(chunk)).min(characters.len());
            let end = characters.get(at).map_or(text.len(), |(byte, _)| *byte);
            let body = &text[..end];
            stream.step(body);
            stream.check(name, body);
        }
        stream
    }

    #[test]
    fn a_long_tail_is_sealed_before_a_complete_block_where_it_cannot_show() {
        let short = MarkdownParser::parse("One paragraph.\n\nAnother.");
        assert!(seal_point(&short, "One paragraph.\n\nAnother.", false, true, &[], LIMITS).is_none());
        let text = stress(3);
        let document = MarkdownParser::parse(&text);
        let plan = seal_point(&document, &text, false, true, &[], LIMITS).expect("a cut");
        assert!(plan.units >= SEGMENT_TARGET);
        assert_eq!(text[..plan.bytes].encode_utf16().count(), plan.units);
        let children = &document.root.children;
        let index = children.iter().position(|block| block.range.location as usize == plan.units).expect("a block start");
        assert!(!matches!(children[index - 1].content, BlockContent::Heading { .. }));
    }

    #[test]
    fn a_streamed_message_shows_as_one_view_at_every_append() {
        let long = stress(2);
        let summary = stream("stress × 2", &long, LIMITS, 8, 1);
        assert!(summary.seals >= 2, "{} seals", summary.seals);
        assert_eq!(summary.unseals, 0);
        stream("stress × 2, small", &long, MEDIUM, 12, 2);
        stream("stress × 4", &stress(4), LIMITS, 40, 3);
        for (name, text) in corpus().iter().filter(|(name, _)| name.starts_with("docs/")).take(6) {
            stream(name, text, MEDIUM, 30, 4);
        }
        let chat_long = include_str!("../tests/markdown-conformance/chat-long.md");
        // Joining sealed segments to the tail again is a fallback; a clean
        // answer never needs it, however its appends fall.
        for seed in 0..6 {
            let summary = stream("chat-long", chat_long, LIMITS, 6, seed);
            assert!(summary.seals >= 2 && summary.updates >= 1, "{} seals, {} updates", summary.seals, summary.updates);
            assert_eq!(summary.unseals, 0, "seed {seed}");
        }
        let summary = stream("chat-long, small", chat_long, SMALL, 4, 7);
        assert_eq!(summary.unseals, 0);
        for (name, text) in corpus().iter().filter(|(name, _)| name.starts_with("spec/")).step_by(25) {
            stream(name, text, SMALL, 6, 5);
        }
    }

    #[test]
    fn adversarial_streams_show_as_one_view_at_every_append() {
        for (name, text) in adversarial() {
            let summary = stream(name, &text, MEDIUM, 10, 7);
            if name == "late definitions" {
                // Sealed before the definitions arrived, then given them.
                assert!(summary.seals >= 2 && summary.updates >= 2, "{} seals, {} updates", summary.seals, summary.updates);
            }
        }
        // Character by character, where a block's first line is still
        // growing at every other append.
        let text = format!("{}#\n\n#x and on\n\n<div\n\n<divx\n\n{}", filler(12), filler(8));
        stream("growing first lines", &text, SMALL, 1, 9);
    }

    #[test]
    fn a_stream_that_stops_mid_construct_ends_as_one_view() {
        let base = stress(2);
        let mut random = Seeded(11);
        for _ in 0..12 {
            let end = random.below(base.len());
            let end = (0..=end).rev().find(|&byte| base.is_char_boundary(byte)).unwrap();
            stream("stopped", &base[..end], MEDIUM, 25, end as u64);
        }
    }
}
