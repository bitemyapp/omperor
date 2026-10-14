# Linux build with Sidestep

Omperor uses the same AppKit UI on macOS and Linux. Linux links the pinned
`vendor/sidestep` runtime and frameworks. Linux support is experimental:
Sidestep does not match every AppKit behavior.

## Requirements

- x86-64 Linux. The OMP helper embeds a Python build that is fetched only for
  x86-64 Linux (and Apple silicon macOS).
- A Wayland session and system fonts (found through fontconfig) to run the
  app. X11 is not supported by Sidestep yet. Tested on Hyprland and sway.
- Network access for the first build: Cargo downloads crates, the overlay
  step fetches the objc2 crates it patches, and the helper build fetches the
  Python archive from GitHub and one package from PyPI.

**NixOS** is the tested setup. The repository's flake provides the pinned
Rust toolchain and every tool the build uses; run the build inside its dev
shell. NixOS has no global library path, so executables find their
libraries by run path: the shell has rustc link through the Nix linker
wrapper, which records the libraries a binary links (the OMP helper's
libopus), rather than with rustc's bundled LLD, and adds fontconfig, which
Sidestep loads at run time to find fonts. A build outside the shell draws no
text, and its helper doesn't start.

**Other distributions** need no Nix: Ubuntu 24.04 is tested. Install
[rustup](https://rustup.rs) (it installs the nightly toolchain that
`rust-toolchain.toml` pins on first use), a C compiler, `just`, Python 3,
Git, `flock` (util-linux), Perl (for `shasum`), curl, zstd, uv, pkg-config,
CMake, Ninja, and fontconfig. libopus's development files are optional:
without them the helper build compiles opus from source. patchelf is needed
only on NixOS.

## Build

```sh
git submodule update --init --recursive
nix develop -c just linux   # on NixOS
just linux                  # elsewhere
```

`just linux` builds the OMP helper, then the app. The executable is
`target/release/omperor`. `scripts/build-omp-helper.sh` stages the pinned
`vendor/oh-my-pi` with the patches in `patches/oh-my-pi/` (0031–0034 are the
Linux ones: the embedded Python, `mv` extended attributes, the desktop and
sandbox crates, and lockable directory handles), fetches its embedded Python
(on x86-64 Linux, the free-threaded debug build serves release helpers too),
and installs `target/omperor-helpers/omp`, rebuilding only when its inputs
change. The first build takes tens of minutes. On NixOS the fetched
interpreter is pointed at the toolchain's dynamic loader, as the standard
one doesn't exist there. Without a helper, a conversation fails with "OMP
RPC startup failed: … No such file or directory". To select another helper:

```sh
OMPEROR_OMP_EXECUTABLE=/absolute/path/to/omp target/release/omperor
```

State lives in `$XDG_DATA_HOME/omperor-dev` (`~/.local/share/omperor-dev` by
default) for local builds; see [state isolation](STATE_ISOLATION.md). Git is
found on `PATH`. "Open in" and "Open Terminal Here" start the first editor or
terminal found on `PATH` from `src/launchers.rs`'s list (Zed, VS Code, Kate,
…; Ghostty, kitty, foot, Konsole, …, then `xdg-terminal-exec`), or the one
chosen in Settings; links and "Show in Files" go through
xdg-desktop-portal. Menu shortcuts written with ⌘ are Super on Linux.

Check, lint or test through the same wrapper the build uses (`just
test-linux` runs the tests):

```sh
scripts/linux-cargo check --locked
scripts/linux-cargo clippy --locked
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
Setting a field's value keeps its editor and the keyboard, as in AppKit, so
the composer takes typing right after launch and after each send, and shows
its placeholder while empty. Sidestep draws every SF Symbol name Omperor and
Upleft ask for (the crown, row icons, status marks, callout and code-block
glyphs), in a symbol configuration's hierarchical or palette colours. Text
drawn under a scaled transform scales with it, so Mermaid labels, which
Upleft renders into a bitmap at the screen's scale, land in their boxes.
Each window's own Wayland surface is a frame that starts at the top of the
menu bar, with the content under it, so compositors that place a window by
its surface rather than its window geometry (Hyprland) keep the menu bar
inside the window instead of cutting it off at the top. Every commit of a
scroll tile brings a buffer: Hyprland drops a subsurface's buffer on a
commit without one, which left scrolled text missing in bands and whole
tiles.
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
