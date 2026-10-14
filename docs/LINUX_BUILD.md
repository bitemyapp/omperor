# Linux build with Sidestep

Omperor uses the same AppKit UI on macOS and Linux. Linux links the pinned
`vendor/sidestep` runtime and frameworks. A Wayland session and system fonts
are needed to run the app; X11 is not supported by Sidestep yet.

Initialize the submodules, then build with the repository's Rust toolchain:

```sh
git submodule update --init --recursive
scripts/linux-cargo build --release --locked
# or: just linux
```

The executable is `target/release/omperor`. To select an OMP helper:

```sh
OMPEROR_OMP_EXECUTABLE=/absolute/path/to/omp target/release/omperor
```

Check or test through the same wrapper:

```sh
scripts/linux-cargo check --locked
scripts/linux-cargo test --locked
```

The wrapper requires Python 3, Git, and `flock`. It generates Sidestep's objc2
overlay and stages the app, Sidestep, and Upleft under the ignored `.objc2-overlay/`
directory. The small Upleft patches in `patches/upleft/` map atomic file
operations to Linux, watch theme directories with inotify, use ICU sentence
boundaries in place of Apple's NaturalLanguage tokenizer, and give Mermaid's
numeric parser a valid C locale. macOS continues using the original tokenizer.
`patches/sidestep/` fixes live text-field values and
overlapping text in the composer and transparent popup controls, and adds the
desktop font fallback stack for symbols missing from the interface font.
`animator()` interpolates geometry and opacity on the render thread, and
animation completions run during mouse tracking. Scrolling draws TextKit 2
text into every scroll tile it paints: Sidestep keeps the tiles it draws
ahead of a scroll, and a text view drew only the fragments of its visible
viewport, so tiles drawn ahead stayed blank until something else redrew
them (the transcript going blank for seconds while scrolling). A text view
now takes what a tile asks for into its viewport as overdraw, as AppKit
does. Only the clip views that scrolled get `viewWillDraw` and have their
bounded layer canvases checked before the frame. The render thread
rasterizes scroll tiles between frames, in 2 ms slices, instead of as each
paint arrives, so a tile drawn ahead of a scroll no longer delays the next
frame; a present first finishes the tiles it shows. `SIDESTEP_TRACE_FRAMES=1`
timestamps each pass and present and reports the render thread's time,
including what presents spent finishing tiles; `SIDESTEP_TRACE_FRAMES=tiles`
also shows each tile drawn and how much text it holds, each tile a present
had to finish, and text views damaging themselves after layout. A view's
drawing shows in the bounds its layer presents while an animation changes
its frame, so the Conversation / Observatory switch's sweep no longer
leaves trails, and soft shadows and composited views draw several times
faster (a surface switch's redraw went from about 200 ms to about 25). Both sets of patches apply to cached staging checkouts; the
original vendors stay clean.

`Cargo.linux.lock` records the Linux overlay dependencies separately from the
normal `Cargo.lock`, so the macOS build continues using published objc2 crates.
Sidestep is experimental; a successful build does not guarantee that every
AppKit behavior matches macOS.

The composer diagnostic types through the real field editor, submits with
Return, and checks that the prompt reaches the transcript and the input clears:

```sh
SIDESTEP_BACKEND=null target/release/omperor --composer-input-smoke
OMPEROR_COMPOSER_SMOKE_SUBMIT=editor SIDESTEP_BACKEND=null target/release/omperor --composer-input-smoke
OMPEROR_COMPOSER_SMOKE_SUBMIT=button SIDESTEP_BACKEND=null target/release/omperor --composer-input-smoke
```

It uses an isolated in-memory mock session, loads no saved sessions, and starts
no inference child. The null backend keeps it off the desktop while native
bitmap drawing produces `/tmp/omperor-composer-input.png` for visual review.

The markdown scrolling regression uses the same staged runtime and Upleft
renderer as the app, with no saved sessions or inference child. It checks
large jumps in both directions and small moves within a cached tile without
explicitly forcing TextKit layout, input during an active touchpad gesture,
that small steps down and back up never show a stretch of the document no
text was painted into, and timed position/size/opacity interpolation. It
reports the markdown fixture's main-thread display times:

```sh
scripts/linux-cargo test --locked --test linux_display
scripts/linux-cargo test --release --locked --test linux_display
```

The Sidestep patch includes native scroll and animation tests. In the
Sidestep checkout:

```sh
cargo test -p sidestep-appkit --test linux_scroll --test linux_layers
```

These check newly exposed text paints, text in tiles drawn ahead of a
scroll, replacement of canvases outside their cached extent,
position/size/opacity interpolation, cached drawing reuse across animation
frames, and completion during mouse tracking.

The scroll diagnostic runs the real app on the desktop: it streams the
mock showcase into a fresh conversation, then scrolls the transcript
through Sidestep's input path on a fixed real-time schedule (touchpad
gestures, wheel detents and a fling), and reports how late input was
handled and how long main-thread turns took. It refuses your real home:

```sh
HOME="$(mktemp -d)" SIDESTEP_TRACE_FRAMES=1 target/release/omperor --scroll-smoke
```

`OMPEROR_SCROLL_SMOKE_PROMPT` replaces the streamed prompt (for example
`/mock scenario=showcase rate=2000`), and `OMPEROR_SCROLL_SMOKE_HOLD=1`
pauses input halfway through and at the end of each gesture, announced on
stderr, for screenshots of what a reader sees.

`--switch-smoke` clicks the Conversation / Observatory switch back and
forth instead, reporting how soon each click is answered. On the null
backend, `OMPEROR_SWITCH_SMOKE_FRAMES=dir` renders at 2× and writes the
switch's pixels around each click, so its animation can be looked at frame
by frame without a desktop. The render thread captures a crop and a worker
encodes and writes it; the GUI only queues requests and polls completion.
Only one capture/write is in flight, so busy ticks skip sampling. Filenames
record the requested frame time. The worker creates the output directory,
and the diagnostic waits for the last write before quitting. Capture or
write failures are reported on stderr and exit with status 1:

```sh
HOME="$(mktemp -d)" SIDESTEP_TRACE_FRAMES=tiles target/release/omperor --switch-smoke
HOME="$(mktemp -d)" SIDESTEP_BACKEND=null OMPEROR_SWITCH_SMOKE_FRAMES=/tmp/switch target/release/omperor --switch-smoke
```
