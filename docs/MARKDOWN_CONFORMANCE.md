# Markdown benchmark and conformance dump

Two headless modes of the app binary compare Markdown renderers across
branches. They use the same harness in every branch: `src/markdown_bench.rs`.
Each renderer supplies one function that reports what it displays.

```sh
# Timing: five phases through the real transcript path, about 2 minutes.
HOME="$(mktemp -d)" target-bench/release/omperor --markdown-bench out.json

# Conformance: every input × {Modern, Editorial, Terminal} × {light, dark}.
HOME="$(mktemp -d)" target-bench/release/omperor --markdown-dump dump/ [inputs.md …]

# Presentation review: whole-window PNGs of a showcase turn (thinking, four
# tools with one failing, the stress answer), finished at four scroll
# positions and streaming, in every style and appearance.
HOME="$(mktemp -d)" target-bench/release/omperor --screenshot shots/

# Side by side: medians of N interleaved runs per binary, with ratios.
scripts/compare-markdown-bench.py main/omperor other/omperor --runs 3
# A | B contact sheet with the differences in text, links, code, and emphasis.
scripts/compare-markdown-dump.py dump-main/ dump-other/ report/
```

Build with `CARGO_TARGET_DIR=target-bench` and the `SDKROOT` from the justfile.
Neither mode touches `target/`, `/Applications`, or the user's running app.

## Scratch home

Both modes refuse to run when `HOME` is the real home. The real home comes
from the user database (`getpwuid`), not from `HOME`. They also refuse a home
that already holds Omperor data unless an earlier run left
`.omperor-markdown-bench-home` in it. Before AppKit starts, the run fills
the scratch home itself:

- It stores preferences with Developer mode on, the mock backend, the Modern
  style, and a light appearance.
- It creates the project `$HOME/project`, or `OMPEROR_PROJECT` when that is
  set, and exports it.
- It generates a deterministic `markdown-stress.png` in the project. The
  stress document's relative image resolves against the session's working
  directory, which is the project. The run never uses the network.
- A dump copies its inputs to `project/markdown-inputs/<name>.md`. It then
  shows each one with `/mock scenario=markdown file=markdown-inputs/<name>.md
  rate=0 ttft=0`. The mock backend reads the file on its runtime thread.

## Invisibility

- The activation policy is `Prohibited` before anything is shown. The
  process never activates, never appears in the Dock, and has no menu bar.
  The regular launch's `activateIgnoringOtherApps` is never reached.
- The main window is 1440 × 900 pt at (−30000, −30000). In these modes only,
  `-[NSWindow constrainFrameRect:toScreen:]` returns the frame unchanged,
  and `-[NSWindow orderWindow:relativeTo:]` moves any window that would
  meet a screen off every screen first.
- By default the window is never ordered in. Setting
  `OMPEROR_MARKDOWN_BENCH_ORDER_IN=1` orders it in off screen, for looking
  at AppKit's own drawing.
- Every harness step checks that no visible window meets any `NSScreen`
  frame. It also checks that the process is neither active nor the
  frontmost application. A violation ends the run with exit status 3.
- The report records the frontmost application before and after the run,
  the window frame, and the screens.

## Drawing

Core Animation's commit, through macOS 26's UpdateCycle, still draws a
layer-backed view that the app invalidates, even in a window that was never
ordered in. That happens in its own run-loop pass, and it draws only the
dirty rect. It does not draw what the harness exposes by scrolling,
resizing, or switching conversations. So:

- **App-driven changes**, such as streaming ticks and image loads, are
  drawn by AppKit, as on screen. When the transcript's document view class
  has its own `drawRect:`, the harness wraps those draws in the span
  `gui.bench.appkit_draw` and counts them (`draws.appkit`).
- **Harness-driven exposures** are drawn by the harness. That covers every
  scroll step, every window width, and the conversation switch in
  `open-long`. The harness draws the transcript scroll view's bounds into a
  2× RGBA bitmap with `cacheDisplayInRect:toBitmapImageRep:`, inside the
  span `gui.bench.draw`, in the same pass as the step (`draws.harness`).
- The harness counts a pass as changing the transcript when
  `-[NSView setNeedsDisplayInRect:]` reaches the document view or a view
  inside it. It watches that method in these modes.

A renderer whose document view class has no `drawRect:` of its own still
runs. Its AppKit draws are not attributed, and `draws.appkit.count` stays 0
in the stream phases. Check that count before trusting the stream numbers.
Also compare `invalidated_passes` with the draws.

A transcript made of row views inside its document view names the row
classes that draw with `markdown_bench::count_row_draws(class)`. The
harness then counts AppKit's draws of views of those classes inside the
document view the same way, in `gui.bench.appkit_draw`. It does nothing
outside these modes, and a renderer that does not call it is measured as
before.

## Benchmark phases

Each phase records every main run-loop pass. The frame budget's collector
(`frame_budget::collect_passes`) keeps each pass's length and all of its
spans. Spans past the per-pass ledger of 1,024 are summed by name.

| phase | what happens | what to read |
| --- | --- | --- |
| `stream-realistic` | New conversation; `/mock scenario=markdown rate=240` (the stress document, about 13 s) | pass distribution, `turn_ms`, span totals |
| `stream-heavy` | New conversation; `/mock scenario=markdown template=stress size=200000 rate=4000` (about 50 s) | same |
| `open-long` | Switches to the realistic conversation and waits until settled. Then switches back to the heavy one; the switch and a drawn frame share one pass. Every later invalidation is drawn too. | `first_complete_frame_ms`: from the switch until the end of the first drawn frame that matches the settled frame and stays that way (sampled hash) |
| `resize` | Window width 1320, 1200, 1080, 1260, then 1440 pt, each followed by a draw and a settle | `steps[]`: the passes ending in each step, their total and maximum |
| `scroll` | From the top of the heavy transcript to the bottom in 400 pt steps, one step per 60 Hz timer pass, drawn each step | pass distribution, `steps`, `document_height_pt` |

A phase has settled when nothing in the transcript has been invalidated or
drawn for 1.2 s. That is longer than two of the app's idle ticks.

`out.json` holds:

- `build`: the source tree's commit, branch, and uncommitted file count at
  run time, plus the profile, the binary's path, size, and mtime.
- `isolation`
- `prompts`
- For each phase:
  - `passes` and `pass_us` (`p50`, `p90`, `p99`, `max`, `mean`, `total`,
    nearest rank)
  - `over_4ms` and `over_8_33ms`
  - `wall_ms`, `turn_ms`, `first_draw_ms`, and `first_complete_frame_ms`
  - `draws.harness` and `draws.appkit`, each a count plus a µs distribution
  - `invalidated_passes`
  - `spans`: every span name with `count`, `total_us`, and `max_us`
  - `pass_list`: every pass, as its length plus its three longest spans

The run also prints a table with the same numbers.

## Stability phases

Seven more phases watch the reader's place and what the viewport shows,
pass by pass, through a long mixed session opened the way the sidebar opens
one. `OMPEROR_MARKDOWN_BENCH_PHASES` picks `timing` (the five above),
`stability`, or `all` (the default).

The sessions are made up (`long_session`): 70 and 110 turns of a question,
often reasoning, runs of tool calls (which repeat, as they do), sometimes a
first reply between them, and an answer — short, a list with code, a chat
sample, a Mermaid diagram or display math that differs from turn to turn,
the stress image, a long answer cut into segments, now and then the stress
document. `Host::open_history` opens one through the app's own path: the
history worker pages it newest first (8, then 16 messages, as a journal's
are read), the tick takes one page a pass, and older pages go above.

| phase | what happens |
| --- | --- |
| `history-open` | The first session opens at its end while its history streams in above. |
| `history-middle` | The second session opens; once four screens are in, the harness scrolls to the middle and sits still while pages, parses, views and diagrams land. |
| `scroll-up-normal` | From 70 % down the whole history, settled (the same start whatever the phase before left), up through unmade rows at 30 pt a 60 Hz frame (1 800 pt/s), 360 frames, drawn each frame. |
| `scroll-up-fast` | On up at 250 pt a frame (15 000 pt/s) to the top. |
| `stream-follow` | An answer with a diagram streams in (48 bytes a tick) with the reader at the end. |
| `stream-scrolled` | Another, with the reader 1.5 screens up. |
| `width-history` | Scrolled 40 % into the history, just past it to where a paragraph's second or later line is at the top (a line a new width rewraps), the window steps through 1 080, 900, 1 260 and 1 440 pt (the transcript 415, 235, 595 and 775 pt wide). |

At the end of every pass (`frame_budget::set_pass_hook`, outside the pass's
timing) the harness looks:

- **Drift.** At the end of each pass it asks the renderer for the content at
  the viewport's top edge (`Host::probe`: a name and its y in the viewport)
  and, at the end of the next, where that content is now (`Host::locate`).
  Drift is the difference, less what the harness itself scrolled in that
  pass. A reader within 36 pt of the end follows it; then drift is the
  change in the distance from the end. `stability.drift` counts the passes
  the harness caused nothing in; `stability.input_drift` the passes it
  scrolled, resized or opened a session in. A pass drifts beyond 0.5 pt.
  A reader the harness scrolls away from the end has nothing to compare
  in that pass. `lost` counts probes the renderer could not find again.
  The branch's probe names a line by its row, the paragraph's offset in the
  message and where the line starts in it, and by the row's place from the
  end and a hash of what it says, so a row the transcript replaced is still
  found.
- **Pop-in.** `Host::viewport_gaps` counts rows in the viewport with no view
  (unparsed, unmade, or laid out at another width) and diagrams or formulas
  drawn as placeholders. `stability.pop_in` counts the passes showing any.
- **Scroll bar.** At the end of each pass the vertical scroller's thumb is
  read: its proportion (`knobProportion`) and its top on screen, the
  scroller's value times the viewport height less the thumb's length. In
  the passes the harness did not scroll, resize or open anything in,
  `stability.scroll_bar` gives the largest one-pass jump of the thumb
  (`max_jump_pt`), the passes it moved more than 1 and 2 pt, the total
  movement, and how often its proportion changed; `measurements_only`
  counts the passes in which the transcript's rows and text stayed the
  same (`Host::content_extent`), `content_changed` those in which a page of
  history or a streamed delta arrived. `scrolled_excess` looks at the
  passes the harness scrolled in: how far the thumb moved beyond what that
  scroll moves it (the scroll times the viewport over the document's
  height), which is the rows made on the way correcting their guesses.

`OMPEROR_MARKDOWN_BENCH_FRAMES=<dir>` also captures the viewport at the end
of every pass (slow; timings are then meaningless) and writes the frames
before and after the first six passes of each phase that drift or pop in.

## Transcript check

`--transcript-check <session.oms> <out-dir>` (`src/transcript_check.rs`)
opens a copy of a session's journal in the real transcript, in the same
scratch home, and fails (exit 1) on any step where what the reader would see
is wrong:

```sh
H="$(mktemp -d)"; OMPEROR_MARKDOWN_BENCH_ORDER_IN=1 HOME="$H" CFFIXED_USER_HOME="$H" \
  target-bench/release/omperor --transcript-check copy-of-session.oms out/
```

It checks every frame of the open (the reader at the end, history loading
above), then scrolls top to bottom and back in 240 pt steps and in a
1 800 pt-a-frame fling. At each step: every made row's view is at its row's
place and height, an activity run's text fills its row, no two row views
overlap, no row view is left that no row owns; and the viewport as the
layers keep it — each view's pixels from however many partial draws AppKit
made — equals the viewport after every view in sight drew all of itself
again from a whole layout. A difference is stale drawing; the two captures
and a diff (red) go to `out/` as PNGs, beside `report.json`.

`OMPEROR_CHECK_LOOK=modern:dark` sets the chat style and appearance,
`OMPEROR_CHECK_WINDOW=1000x800` the window, and `OMPEROR_CHECK_EARLY=1`
scrolls up from the end while the history still loads. Order the window in
(off screen, as above) so AppKit keeps layer contents as it does on screen.
A session journal is the user's data: keep the copy and every output in a
scratch directory.

The open is also measured: the first frame showing history, the first with
every row in sight made, and every move of the scroll bar's thumb while
nobody scrolls (`report.json` → `open`). `OMPEROR_CHECK_OPEN_ONLY=1` stops
after the open (and a pause in which the app saves the session's heights);
`OMPEROR_CHECK_PIXELS=0` skips the pixel check so the open is timed finely.
Run twice with the same `HOME` to measure a reopen at saved heights.

## Dump format

The dump writes two files for each capture: `<input>.<style>.<appearance>.png`
and `.json`. `style` is `modern`, `editorial`, or `terminal`; `appearance` is
`light` or `dark`. It also writes an `index.json` with the build, the inputs,
and the isolation record.

The harness sends the input as one assistant message, then sets the style
and appearance with `Host::set_look`. It captures the message every 0.6 s
until two consecutive captures are identical, or until 12 s pass, when
`settled` is false.

**PNG**: the message's rect in the transcript's document view. The rect spans
the full width of the document view and the full height of the message,
not only the viewport. The harness draws it at 2 px per point with
`cacheDisplayInRect:toBitmapImageRep:`. The capture goes over the canvas
colour: light rgb(253, 252, 249) and dark rgb(25, 25, 29).

**JSON**: fields a renderer must produce from **what its layout
displays**, not from its parser or its storage:

| field | definition |
| --- | --- |
| `visible_text` | The message's text as displayed, in reading order, as a reader would select and copy it. Paragraphs and lines are separated by `\n`. Everything drawn as text is included: the author label, list markers, code-block language labels, table cells (TextKit 1 separates them with tabs or newlines as laid out), and image captions. Each attachment appears as U+FFFC. Text the layout does not show is left out. With TextKit 2, read the layout fragments' text, not a storage string that holds hidden markup or a folded `<details>` body. |
| `links` | `[{text, url}]` in order. A link is a maximal run of displayed characters with the same target. `url` is the absolute string. |
| `attachments` | `{count, items: [{kind, offset, width_pt, height_pt}]}`, with `offset` in UTF-16 units into the message. `kind` is `image` when the attachment is drawn taller than 24 pt and `icon` otherwise, such as a symbol or a checkbox. |
| `code_runs` | Maximal runs of characters drawn in a monospaced font, split at paragraph separators (`\n`, `\r`, U+2028, U+2029). Text is kept verbatim, indentation included. Runs of only whitespace are dropped. A monospaced font is fixed-pitch or has the monospace symbolic trait. In the Terminal style, nearly everything is a code run. |
| `bold_runs` | The same, for characters whose font weight is at least `NSFontWeightSemibold` (0.3) or that carry the bold symbolic trait. |
| `italic_runs` | The same, for the italic symbolic trait. |
| `height_pt` | The height of the message's drawn area: the union of its line fragments, rounded up. |
| `input`, `style`, `appearance`, `width_pt`, `png`, `png_pixels`, `settled` | Added by the harness. |

TextKit 1's implementation is `src/transcript_semantics.rs`
(`describe(view, range) -> Displayed`). It reads the text storage, whose
string is exactly what TextKit 1 lays out, and it reads fonts and link and
attachment attributes per run.

## What another renderer's branch must provide

The harness and its hooks stay unchanged. The branch keeps these five calls:

- `markdown_bench::prepare()` first in `main`
- `hide_application(&app)` after `event_spans::shared_application`
- `present(&window)` in `build_window`, instead of `center` and
  `makeKeyAndOrderFront`
- `launched(Rc::new(self.retain()))` in `applicationDidFinishLaunching:`,
  before the activation policy is set
- `after_tick()` at the end of `pollOmpRuntime:`

It also implements `markdown_bench::Host` for `Retained<AppDelegate>`.
Only two methods know the renderer:

- `transcript_scroll_view()` returns the scroll view whose document view is
  the transcript. That document view is what gets drawn, scrolled,
  captured, and watched for invalidation.
- `displayed_answer()` returns the last assistant message's rect in the
  document view's coordinates, plus the fields above.
- `probe()`, `locate()` and `viewport_gaps()` name the content at the
  viewport's edge, find it again, and count what the viewport shows
  unfinished, for the stability phases.

The mock scenario (`/mock scenario=markdown …`, `file=`) and the stress
document are shared code and need nothing else.
