# UI toolkit efficiency: architecture, measurements, and Haiku

As of 2026-09-30. The question here is which GUI architectures are most efficient for responsiveness (low input latency, no main-thread stalls) and energy (few wakeups, minimal redraw). It covers AppKit, Haiku, Qt, GTK, the browsers, Android, Flutter, Slint, EFL, iced, egui, GPUI, the GPUI Fast fork, and the Sidestep prototype. It feeds a decision: reuse an existing toolkit for a Linux desktop, or build a more efficient toolkit, compositor and desktop environment. The design that follows from it is in [LINUX_DESKTOP_DESIGN.md](LINUX_DESKTOP_DESIGN.md).

## How this was put together

The architecture tables come from three research passes against current source code and official documentation, made on 2026-09-30. Anything that could not be confirmed from a primary source is marked unconfirmed. The measurements come from two places:

- the workstation benchmark of 2026-09-25: 19 variants on an NVIDIA RTX PRO 6000 under headless sway, with a later Sidestep / Qt Quick / GPUI / Zed run;
- the on-screen comparison of the AppKit Omperor with its GPUI port on an M5 Max with a 5K display (2026-09-26, branch `gpui-port`, `gpui/COMPARISON.md`).

**GPUI Fast** is [longbridge/gpui-fast](https://github.com/longbridge/gpui-fast), an experimental, Apache-2.0 fork of Zed's GPUI that keeps upstream's public API. It retains views whose dependencies haven't changed, tracking those dependencies automatically. It splices changed descendants into a retained parent, and reuses layout nodes, shaped text, glyph and path work, and batching. It adds no GPU damage tracking, partial presents, render threads or idle changes. A first port of Omperor's GPUI app to it was measured on 2026-09-29, in a separate worktree (branch `codex/gpui-fast-comparison`, `gpui/FAST_COMPARISON.md`); section 4 reports it.

## 1. Rendering pipeline

| | Retained scene | Deferred rendering | Damage tracking (CPU side) | Incremental re-rendering (GPU side) | Tells the compositor only what changed |
|---|---|---|---|---|---|
| **AppKit + Core Animation** (reference) | Layer tree | Yes, to a render server in WindowServer | Yes (`drawRect:` gets dirty rects) | Yes: layers stay on the GPU and only changed layers re-render | It is the compositor |
| **Haiku** | Server keeps each window's pixels, plus per-view bitmaps | Yes: views send drawing commands that app_server rasterizes | Yes: region clipping, one merged update per window | No GPU: the CPU copies dirty regions to the screen | Dirty regions only, without vsync |
| **Qt Widgets** | Widgets and backing-store pixels | No | Yes | CPU only | Yes, and it scrolls pixels in place |
| **Qt Quick** (GPU backends) | Scene graph; geometry kept on the GPU | Yes, on a render thread | No (the software backend does) | Keeps geometry, not pixels; redraws the whole window. Partial updates were declined in 2020 (QTBUG-82860) | No |
| **GTK 3** | Widgets and pixels | No | Yes | CPU (Cairo) | Yes |
| **GTK 4** (Vulkan/GL) | Widgets and a cached render tree per widget | Records a render tree, renders it on the same thread | Yes, by diffing render trees | Yes: renders only the damaged area; caches glyphs and textures, not whole subtrees | Yes |
| **Chromium** (cc + viz) | Layer trees | Yes: compositor thread plus a GPU process | Yes, per tile | Yes: tiles kept as textures | Supported; unconfirmed whether it is on under Wayland |
| **Firefox WebRender** | Display lists and cached tiles | Yes: several threads plus a GPU process | Yes, per tile | Yes: tiles persist between frames | Yes on Wayland (since Firefox 82) |
| **Android HWUI** (not desktop) | Per-view display lists | Yes, on a render thread | Yes | Yes | Yes |
| **Flutter** (Linux) | Framework trees; new layer tree every frame | Yes, on a raster thread | The engine supports it; the Linux shell ignores it | No: whole frame; its raster cache is off under Impeller | No |
| **Slint** | Item tree with dependency tracking | No | Yes (dirty regions) | Software renderer: yes. Skia on the GPU: opt-in. FemtoVG, the default: no | Software renderer only |
| **EFL / Evas** | Canvas | Optional render thread (shared-memory backend only) | Yes | Yes (GL backend) | Yes |
| **iced 0.14** | View rebuilt after each update; canvas geometry cached | No | tiny-skia backend: yes. wgpu backend: no | wgpu: clears the whole window each frame but keeps cached meshes on the GPU | No |
| **egui** | Immediate mode | No | No | No: clears and rewrites buffers every frame | No |
| **GPUI 0.2.2** (measured) | Element tree rebuilt every frame; `.cached()` views replayed | No: the main thread builds, sorts and draws | Per-view caching only | No: clears and re-uploads everything every frame | No |
| **GPUI upstream** (Sept 2026) | Same | No | Same | No (the Linux renderer is now wgpu) | No |
| **GPUI Fast** (longbridge fork) | Views retained while their tracked dependencies are unchanged; changed descendants spliced in; layout nodes kept | No | Yes, per view: unchanged subtrees, layout, text and batching are reused | No: still uploads the whole scene, clears it and redraws every batch | No |
| **Sidestep slice** (prototype) | Layers | Yes: `drawRect:` records, a render thread rasterizes | Yes | Tiles on separate compositor surfaces; scrolling just moves them | Yes |

## 2. Threads, idle behaviour and the compositor

| | Render thread | Scrolling off the main thread | Animation off the main thread | At idle | Handing content to the compositor |
|---|---|---|---|---|---|
| **AppKit + Core Animation** | Out of process | Partly (NSScrollView's responsive scrolling) | Yes, in the render server | Frame loop stops. Measured 0.08% CPU, 4.5 wakeups/s | Native: IOSurface, overlays |
| **Haiku** | A server thread per window | Likely: the server copies existing pixels and the app redraws only the new strip (not verified) | No | No periodic kernel tick; text fields still blink twice a second when their window is inactive | No compositor; hardware overlays for video |
| **Qt Widgets** | No | No | No | Paints only on request | Shared-memory buffers; no GPU-buffer or presentation-timing support |
| **Qt Quick** | Yes, the default | No (Flickable scrolls on the UI thread) | Its Animator types only | Renders only on request | Same Wayland client as Qt Widgets |
| **GTK 3** | No | No | No | Stops | Shared-memory buffers |
| **GTK 4** | No | No | No | Frame clock stops | GPU buffers on subsurfaces (4.14+), presentation timing, viewporter, color management |
| **Chromium** | Compositor thread plus GPU process | Yes | Yes | Stops frames, but background timers cost 11 ms of CPU per second (measured) | Overlays on subsurfaces; full delegation is ChromeOS-only |
| **Firefox WebRender** | Yes | Yes (APZ) | Yes, for transform and opacity | Unconfirmed | Native subsurface compositor behind a pref (HDR); video frames go straight to the compositor |
| **Android HWUI** | Yes | Probably not (unconfirmed) | Some view properties | Skips frames with nothing dirty | SurfaceFlinger with hardware overlays |
| **Flutter** (Linux) | Raster thread | No | No | Renders on request | GL into a subsurface (master branch) |
| **Slint** | No | No | No | Stops | Presents with damage |
| **EFL** | Optional | No | No | Stops | Damage, subsurfaces, GPU buffers |
| **iced 0.14** | No | No | No | Stops ("reactive rendering") | Plain surface |
| **egui** | No | No | No | Stops | Plain surface |
| **GPUI 0.2.2** | No | No | No | Keeps waking: macOS every refresh while visible (123/s measured); X11 a refresh-rate timer; Wayland keeps asking for frames. Also re-presents every refresh for 1 s after input | One surface, no damage hints |
| **GPUI upstream** | No | No | No | Wayland now parks; macOS and X11 unchanged; inactive windows capped at 30 fps | Same |
| **GPUI Fast** | No | No | No | Unchanged from its upstream base: macOS still wakes every refresh while the window is visible; Wayland parks. Drawable acquisition is still synchronous | One surface, no damage hints; native views such as a WebView can be composited inside the window |
| **Sidestep slice** | Yes | Yes (the compositor moves tiles) | Planned | Zero | Subsurfaces |

## 3. Per-toolkit notes

### AppKit and Core Animation

Views are backed by layers. `drawRect:` is called only for dirty rectangles, and what it draws becomes a layer's contents. A Core Animation transaction commits the layer tree to the render server in WindowServer, which keeps every layer on the GPU and composites only what changed. Implicit and explicit animations run in the render server, so they don't need the app's main thread. On this Mac, the AppKit Omperor used 0.08% CPU and 4.5 wakeups per second at idle and while a caret blinked, with no slow main-thread passes.

### Haiku

Each `BWindow` is a thread in the app, with a matching window thread in app_server. Views send drawing commands over a port to app_server, which rasterizes them on the CPU with AGG into one back buffer in system RAM and copies dirty regions to the screen. Updates are merged: each window has at most one update outstanding, and new damage piles into a pending region. Window moves are copied, and only the uncovered part is redrawn. Newly exposed areas are filled from the view colour or a server-held view bitmap without waking the app. There is no window compositing, no GPU acceleration in app_server, and no wait for vertical retrace, so tearing is likely (inferred from the code, not tested). Section 5 covers the details.

### Qt Widgets

Widgets paint with QPainter's raster engine straight into a backing store on the GUI thread. Only regions passed to `update()` are repainted. On Wayland, the shared-memory backing store tracks dirty regions per buffer, copies clean areas forward from the previous buffer, and scrolls pixels in place (not at fractional scales). Buffers are memfd-backed with a 256-byte stride so the compositor can import them as udmabuf, and damage is sent with `damage_buffer` when the surface version allows. The Qt Wayland client implements no presentation-time, fifo, commit-timing or linux-dmabuf protocol.

### Qt Quick (Qt 6)

A retained scene graph whose batches and vertex buffers stay on the GPU between frames. With the threaded render loop, the GUI thread is blocked while it syncs state into the scene graph, then the render thread draws it. The threaded loop is the default on Linux except with Mesa's llvmpipe, and is always used with Vulkan; it falls back to the basic loop in some cases, such as several windows or broken vsync. On GPU backends the whole window is redrawn each frame: geometry is retained, pixels are not. The OpenGL partial-update request (QTBUG-82860) was closed "Won't Do" in 2020. The software adaptation does partial updates, except at fractional scale. Animator types run on the render thread; Flickable scrolling runs on the GUI thread. Qt 6 renders through RHI, with OpenGL as the Linux default and Vulkan selectable through `QSG_RHI_BACKEND`.

### GTK 3

Cairo image surfaces on the CPU. Only invalid GdkWindow regions are painted, and on Wayland the unchanged area is copied back from the committed buffer. GL areas use EGL buffer age and swap-with-damage. Damage goes out with `wl_surface.damage` in surface coordinates (not `damage_buffer`). Presentation times are estimated from frame callbacks.

### GTK 4 (GSK)

Each widget caches its render node until it is queued for redraw. A snapshot is recorded into a render-node tree and then rendered on the same thread; there is no render thread. Damage comes from diffing the previous and new render trees (`gsk_render_node_diff`), and `GSK_DEBUG=full-redraw` turns that off. Rendering is limited to the damage plus buffer-age history: `EGL_EXT_buffer_age` and `eglSwapBuffersWithDamage` on GL, and a region per swapchain image with `VK_KHR_incremental_present` on Vulkan. The GPU cache holds textures, tiles, glyphs and path masks; no cache of rendered subtrees was found (unconfirmed absence). The frame clock runs only while phases are requested, so it stops at idle. GtkGraphicsOffload (4.14) puts dmabuf content on subsurfaces; 4.16 added black backgrounds and passes transforms to the compositor. GTK also uses `damage_buffer`, wp_presentation, color management, color representation and viewporter, but not fifo or commit-timing. The renderer history:

- ngl became the default in 4.13.6;
- 4.16 made Vulkan the default on Wayland, with ngl elsewhere;
- 4.18 removed the old GL renderer.

### Chromium (cc + viz)

Three layer trees are retained: main, pending and active. The main thread records paint and commits to the compositor thread, while rasterization and the viz display compositor run in the GPU process. Only invalidated tiles are rasterized again, and tiles are kept as GPU textures. viz computes a root damage rectangle and uses partial swap where it can; whether that is enabled on Linux Wayland is unconfirmed. Scrolling and compositor animations run off the main thread. When no frame is needed, the scheduler stops listening for frames, but the workstation benchmark measured 11 ms of CPU per second of background timers at idle. On Ozone Wayland, overlay candidates go to subsurfaces with GBM buffers and damage is sent with `wl_surface_damage`. Full delegated compositing is ChromeOS/LaCrOS only, and `WaylandOverlayDelegation` is disabled on Linux.

### Firefox WebRender

The content main thread builds the display list. Scene-builder and render-backend threads build frames, and a single render thread talks to the GPU. Picture-cache tiles persist, damage is limited to the tiles that changed, and partial present works on Wayland since Firefox 82 (bug 1648872). APZ scrolls off the main thread, and transform and opacity animations run there too. A native compositor using subsurfaces exists but is pref-only, for HDR. Decoded video frames are shown directly as wl_buffers. Firefox 149 reworked the Wayland painting pipeline.

### Android HWUI

The UI thread records RenderNode display lists when they are invalidated, and the RenderThread draws them. A damage accumulator tracks dirty regions, and frames with nothing dirty are skipped. The renderer uses buffer age and `EGL_KHR_partial_update`, and requires swap-buffers-with-damage. RenderNode animations run on the RenderThread. Whether scrolling stays on the UI thread, and whether vsync is requested only on demand, is unconfirmed. SurfaceFlinger composites with hardware overlays (general knowledge; not fetched).

### Flutter (Linux)

The UI thread builds a layer tree and the raster thread renders it. Linux got a separate raster thread in March 2025, with the final blit on the GTK thread, and the UI and platform threads merged in 3.39 (stable in 3.41). The engine can diff damage, but forces a full repaint whenever an external view embedder is used, and the Linux shell never reads the paint region. Every frame is drawn whole, and the raster cache is disabled under Impeller. Scrolling and animation run in Dart on the UI thread (general knowledge). On master, Wayland renders GL into a `wl_subsurface`; which stable release has this is unconfirmed, and a Vulkan swapchain subsurface PR is open. Flutter 3.47 (2026-08-12) made Impeller the default on Linux. The release blog says Vulkan, but a 3.47 bug report shows "OpenGLESSDF" and the Vulkan path is unmerged, so it is most likely GLES.

### Slint

A retained item tree with property dependency tracking, rendered on the event-loop thread. Dirty regions can be several rectangles, and since 1.18 a removed element repaints only its own area. The software renderer has three modes: full redraw, reused buffer, and swapped buffers with buffer-age history. Skia's partial rendering is on by default only for its software surface; on GL, Vulkan and WGPU it is opt-in through `SLINT_SKIA_PARTIAL_RENDERING`. FemtoVG, the default Rust renderer, has no damage code. The winit loop waits for events and only polls while an animation or redraw is pending.

### EFL / Evas

A retained canvas that "draws only the necessary changes". Rendering happens in the main loop's idle-enterer step. An async render thread exists, which the `wayland_shm` engine can use but `wayland_egl` cannot. The EGL engine uses partial swaps and buffer age. EFL ticks on Wayland frame callbacks, renders only when something changed, sends `wl_surface_damage_buffer`, and supports subsurfaces and linux-dmabuf buffers.

### iced 0.14

`view()` runs after each update, and the UI is rebuilt when it is out of date. Canvas `Cache` keeps geometry until it is cleared or the canvas resizes. The tiny-skia backend diffs layers and draws only damaged rectangles, and reuses buffers by buffer age, but presents without damage. The wgpu backend clears every frame, while cached meshes keep their GPU uploads. Since 0.14's "reactive rendering", iced redraws only on an update or when a widget asks; the `unconditional-rendering` feature restores the old always-redraw behaviour.

### egui

Immediate mode: layout is redone every frame, then shapes are tessellated and painted on the same thread. The renderer clears the window and writes vertex and index buffers every frame. It repaints only on input, animation or `request_repaint`, so it is idle-clean, but every repaint is a full one.

### GPUI 0.2.2 (crates.io)

The element tree is rebuilt each frame, except views marked `.cached()`, whose recorded primitives are replayed. The scene is rebuilt, re-sorted in `Scene::finish`, re-uploaded and redrawn in full every frame; the blade renderer clears the whole window. Frame-clock behaviour depends on the platform:

- **macOS:** a display link runs whenever the window is visible (`window_did_change_occlusion_state` → `start_display_link`).
- **X11:** a refresh-rate timer runs while the window is visible.
- **Wayland:** contrary to earlier notes, 0.2.2 does not park. Every frame callback requests another `wl_surface.frame` and commits, so the loop runs at whatever pace the compositor sends callbacks, and only skips drawing.
- **After input:** for one second after any input while the window is active, it re-runs the whole scene on the GPU every refresh.

The Wayland surface sends no damage (only the cursor surface does).

### GPUI upstream (Zed, commit 8e7fbcc, 2026-09-30) and GPUI-CE

The pipeline is the same, still with `cached()` views replayed and the scene re-sorted every frame. The Linux renderer is now wgpu; blade was removed in PR #46758. It still clears every frame and re-uploads with `write_buffer`. Idle behaviour has changed:

- **Wayland:** now parks (`FrameLoop::Parked`, woken by `schedule_frame`).
- **macOS:** still subscribes to a shared per-display CVDisplayLink while the window is visible.
- **X11:** still has its refresh loop.
- **Frame caps:** inactive windows are capped at about 30 fps, and thermal pressure caps at 60 fps.
- **After input:** the one-second sustain now needs fast input, at least 60 events per second over 100 ms.

No damage tracking or partial present was found. GPUI-CE (the community edition, commit 98e9aac) is the same design; its scissor is used only for blur filters.

### GPUI Fast (longbridge/gpui-fast)

An experimental fork of Zed's GPUI that explores retained mode. It keeps upstream's public API and changes as little of GPUI's own code as it can.

- **Retained views.** Each view's reads of entities, globals and list or scroll state are tracked, and a view whose dependencies haven't changed is not re-rendered. Changed descendants are spliced into a retained parent. State GPUI can't observe (`Rc<RefCell<…>>`, atomics, time) still needs `cx.notify()`. Retention is turned off while accessibility is active, and full refreshes, dragging and inspector picking bypass it.
- **Reused work.** Taffy layout nodes, shaped text, glyph and font work, path triangulations, and batching and sorting are all reused.
- **Composition.** It can compose native views, such as a WebView, inside a GPUI window, with GPUI's UI drawn above them.
- **What it leaves alone:**
  - the Metal renderer still uploads the complete scene, clears the drawable and draws every batch, so there is no GPU-side damage tracking or partial present;
  - the macOS display-link subscription is unchanged, so a visible window still wakes every refresh;
  - drawable acquisition and the transactional present wait are still synchronous on the main thread.
- **Benchmarks.** The fork's own Linux benchmarks claim 94% less CPU on static frames, 88% less during animation and 87% less while scrolling, against upstream. Those use many independently changing panels and aren't projections for Omperor.

Against the 2026-09-25 GPUI profile, retention removes the element-tree rebuild (about 24 ms/s for the spinner over a page) and some of the scene work. The upload, the full GPU redraw, and the per-frame floor remain. On the main thread it should land near GTK 4 for small updates. It stays a main-thread, full-redraw renderer with an always-on display link on macOS, so it doesn't reach Core Animation or Sidestep.

### Sidestep slice

The AppKit-on-Linux prototype in the Core Animation model. `drawRect:` only records drawing commands on the main thread. A render thread owns the Wayland connection and rasterizes on the CPU, only in damaged areas. Scrolled content lives in tiles on Wayland subsurfaces, so scrolling just moves them and nothing is re-uploaded per frame.

## 4. Measurements

### Linux workstation, 2026-09-25

The setup:

- an NVIDIA RTX PRO 6000 under a real Wayland compositor (headless sway), with GPU rendering and zero-copy buffers;
- 19 variants across three scenarios, one run each;
- GPU watts are measured board power above idle, good to about ±0.5 W;
- CPU watts are estimated as CPU time × 8.5 J per CPU-second, the median of the runs heavy enough to measure directly. Other jobs were sharing the reserved cores, so per-core energy only resolved heavy loads.

Idle is left out because every variant drew essentially nothing. The notable idle CPU costs were Chromium's background timers (11 ms/s) and 4–5 ms/s for every Vulkan-based app, apparently the NVIDIA Vulkan driver.

Small 60 fps animation (a 16 px spinner):

| | CPU ms/s (app + compositor) | GPU W | Estimated total W | VRAM | PSS |
|---|---|---|---|---|---|
| Slint software (CPU) | 18 + 58 | 4.5 | 5.2 | 0 | 21 MB |
| Qt Quick software (CPU) | 7 + 42 | 4.8 | 5.2 | 0 | 61 MB |
| Qt Widgets / GTK Cairo / raw damage-only (CPU) | 7–12 + 41–57 | 5.2–5.3 | 5.7–5.9 | 0 | 26–35 MB |
| Qt Quick GL, Chromium, Firefox, GTK GL (GPU) | 13–36 + 10–38 | 6.0–6.6 | 6.5–6.8 | 66–155 MB | 150–575 MB |
| GPUI | 70 + 9 | 6.7 | 7.4 | 221 MB | 325 MB |
| egui / iced wgpu | 18–55 + 53–54 | 7.5–8.0 | 8.2–8.9 | 78–88 MB | 173–288 MB |
| CPU full redraw (raw, Firefox software) | 89–140 + ~290 | 19.5–20.2 | 22.8–23.9 | 0 | 31–481 MB |

Continuous scroll (the whole window changes every frame):

| | CPU ms/s (app + compositor) | GPU W | Estimated total W |
|---|---|---|---|
| Qt Quick GL / Vulkan | 16–18 + 11–46 | 6.0–6.3 | 6.5 |
| GPUI | 72 + 9 | 6.1 | 6.8 |
| Chromium, Firefox, egui, GTK GL/Vulkan (GPU) | 19–106 + 10–53 | 5.7–7.4 | 6.9–8.0 |
| GTK Cairo / Qt Quick software (CPU) | 77–88 + 131–146 | 9.8–10.0 | 11.7–11.8 |
| iced wgpu (no virtualized list) | 393 + 46 | 8.2 | 11.9 |
| CPU full-frame (raw either mode, Chromium/Firefox software, Qt Widgets) | 42–212 + ~300 | ~20 | 22.7–24.2 |
| iced tiny-skia | 1006 + 313 | 20.7 | 31.8 |

Sidestep slice against the best Qt Quick variant and GPUI (later run, same machine):

| Scenario | Sidestep slice | Qt Quick, best variant | GPUI |
|---|---|---|---|
| 60 fps spinner | 5.2 W | 6.3 W | 7.3 W |
| Scrolling list | 5.7 W | 6.5 W | 7.0 W |
| 2 Hz caret | 4.9 W | 4.3 W | 4.5 W |
| Memory (PSS) | 44–86 MB | 118–211 MB | 324 MB |

When scrolling, the slice's app uses 4 ms of CPU per second against GPUI's 72, because the compositor just moves tiles. Zed burns 23 ms of CPU per second even when idle. What the workstation runs showed:

- **Frames, not cost per frame.** Any 60 fps activity costs about 5 W on this card, whatever renders it, because the GPU leaves its idle state; rendering fewer frames matters more than rendering cheaper ones.
- **CPU-drawn full frames are the worst case.** The compositor uploads about 8 MB per frame to the discrete GPU, costing about 0.3 of a core and +20 W.
- **CPU rendering with damage tracking wins small updates.**
- **Qt Quick was the best all-round reusable toolkit,** with GTK 4 GL close behind.

### GPUI CPU profile, 2026-09-25

A 60 fps spinner over a 60-line page, in ms of CPU per second at about 62 fps:

| Case | Total | Layout | Prepaint | Paint (of which glyphs) | Cached replay | Scene sort | Present + upload |
|---|---|---|---|---|---|---|---|
| Spinner alone | 10 | 0.5 | 1.9 | 1.1 | – | – | 0.5 |
| Page cached + spinner | 44 | – | 1.5 | ≈ replay | 12.3 | 12.9 | 15.9 |
| Page rebuilt + spinner | 72 | 5.4 | 9.2 | 31.4 (18.9) | – | 8.7 | 21.1 |
| Page rebuilt + quad spinner | 68 | 8.4 | 16.5 | 24.7 (15.7) | – | 8.2 | 14.9 |
| Scroll | 72 | 7.4 | 11.3 | 29.6 (19.6) | – | 9.0 | 19.9 |
| Page rebuilt, 4K | 78 | 6.3 | 10.4 | 33.3 (19.7) | – | 12.4 | 19.9 |

The biggest single hotspot (about 23% of samples) is `memcpy` into write-combined VRAM: about 6,000 CPU-to-VRAM writes per frame whenever the page is on screen, cached or not. GPU time per frame is only 44–48 µs at 1080p. GPUI's cost is almost entirely CPU-side scene reconstruction and re-upload.

### On screen on a Mac, 2026-09-26

The AppKit Omperor against its GPUI port: one run each, a 5K display, each app frontmost, streaming a 200,000-character mock answer. Full write-up in `gpui/COMPARISON.md` on branch `gpui-port`.

| Measure | AppKit (original) | GPUI (port) |
|---|---|---|
| Idle: CPU, wakeups/s, CPU energy (20 s) | 0.08%, 4.5, 0.01 J | 0.58%, 123, 0.24 J |
| Streaming: CPU, app GPU busy, CPU energy (52 s) | 30.7%, 0.3%, 23 J | 21.7%, 13.0%, 48 J |
| Streaming: main-thread passes over 4 ms | 1,149 (22/s) | 5,853 (113/s) |
| Streaming: slow-pass p50 / p90 / p99 / max | 6.8 / 10.5 / 16.0 / 37 ms | 8.3 / 8.4 / 16.5 / 39 ms |
| Streaming: main thread inside a slow pass | 16% of the time | 96% of the time |
| Streaming: mean extra wait for a keypress | 0.7 ms | 4.2 ms |
| Caret: CPU, wakeups/s, CPU energy (30 s) | 0.08%, 5, 0.05 J | 0.87%, 124, 0.63 J |

Off screen the GPUI port had looked better on every timing and on energy, because its display link never ran for an occluded window. On screen it redraws the whole 5K window every refresh while anything changes, and each frame holds the main thread about one refresh interval (8.3 ms).

### GPUI 0.2.2 against GPUI Fast in Omperor, 2026-09-29

A separate session ported the Omperor GPUI app to GPUI Fast at commit `7ab23f4` and compared it with 0.2.2 on the same Mac. Setup:

- the same app source, base `ed154fe`;
- foreground 1440 × 900 windows on the 5K display at 120 Hz;
- a 20-second idle, the full stress stream, a 20-second idle on the finished answer, then 600 scroll inputs.

Both produced the identical answer. Only the 0.2.2 run passed validation. GPUI Fast's window was frontmost for only 45% of the stream, and current GPUI throttles inactive windows, so its power samples were rejected and its timing figures are provisional.

| Measure | GPUI 0.2.2 | GPUI Fast |
|---|---|---|
| Full stream, wall time | 50.52 s | 50.50 s |
| Streaming main-loop p95 | 12.2 ms | 8.4 ms (provisional) |
| Streaming passes over 4 ms, per second | 111 | 51 (provisional) |
| 600 scroll inputs, wall time | 13.6 s | 10.5 s (provisional) |
| Scrolling main-loop p95 | 8.5 ms | 1.6 ms (provisional) |
| Streaming CPU / CPU energy / GPU active | 27.9% / 26.4 J / 11.3% | not accepted |
| Idle wakeups (empty / finished answer) | 125 / 124 per second | not accepted |

The scrolling and streaming main-thread figures are promising, but they are not yet a power or efficiency result. Both backends still exceed the 4 ms budget while streaming. A valid comparison needs both bundles rebuilt from the same source and both windows frontmost throughout.

## 5. Haiku: kernel versus GUI stack

Almost everything here comes from Haiku's source on `master`, the Haiku blog and the Be Book. The Linux equivalents are general knowledge and were not re-checked.

### Kernel

| Feature | What Haiku does | Why it helps | Linux equivalent |
|---|---|---|---|
| Scheduler modes | Per-core run queues, cache- and SMT-aware, with two modes. Low latency (the default) prefers idle cores and packages. Power saving packs threads onto one "small task core" and spills over only past 70% load, and sends all interrupts to that core. Cached core affinity expires after 100 ms. | Latency mode spreads work out; power mode keeps other cores in deep sleep. | Energy-aware scheduling, uclamp, EEVDF slices and schedutil are more capable, but userland rarely sets them per thread. |
| Load tracking | Load is estimated as if each thread were alone on the machine, so balancing works when every core is busy. An idle CPU arms one load-decay timer and then stays quiet. | Placement decisions stay accurate. | PELT does this. Nothing missing. |
| Priority classes | 15 display, 20 urgent display, 100 real-time display, 110 urgent, 120 real-time. Threads at 100 or above never get a penalty. The GUI stack uses them: window threads 15, app_server threads 15, app_server event loop 90, cursor loop 95, input_server main 20 and event loop 103, audio 115, live audio 110, video playback 20. | Input and cursor handling always preempt drawing and applications, with no configuration. | `SCHED_FIFO`/`SCHED_RR`, `SCHED_DEADLINE`, rtkit, EEVDF latency and nice exist. Nothing assigns them to compositor input and cursor threads or UI threads; the gap is policy. |
| CPU idle and frequency | `x86_cstates` (MWAIT) idle driver and `intel_pstates`/`amd_pstates` frequency drivers in the base install. The scheduler asks for performance relative to load. | Deep idle states and frequency scaled to demand. | cpuidle/intel_idle, intel_pstate and amd-pstate are more mature. |
| Ports | Bounded message queues (the input_server's holds 100). `write_port` blocks when full and `write_port_etc` takes a timeout; messages are capped at 256 KB. | Backpressure is built in: a stalled reader can't grow memory without limit. | Unix sockets are bounded by bytes, not messages; Wayland queues are unbounded and disconnect a client on overflow. No equivalent. |
| Areas | `create_area`/`clone_area`. app_server gives each client a pool of areas it clones, and large link payloads go through a transferred area. | Zero-copy bitmaps and large messages. | `memfd` + `mmap`, dmabuf. |
| Locks | Benaphores: an atomic counter plus a semaphore, so an uncontended lock never enters the kernel. There is also a kernel user mutex. | Cheap locking. | futex. |
| Threads | One thread per window on both sides, so threads are assumed cheap. No measured creation cost found (unconfirmed). | | Linux threads are cheap. |
| File watching | `watch_node`/`watch_volume` send messages to a looper. Interim stat updates during writes go only to listeners that ask. Live queries push updates for indexed attributes. | Nothing polls, and write storms are muted by default. | inotify/fanotify exist; there is no kernel live query (it would need an indexing daemon). |
| Timers | Each CPU keeps a sorted event list and programs a one-shot hardware timer for the next event, so there is no periodic tick. A quantum timer is armed only for non-idle threads. No timer slack or coalescing found. | Tickless idle. | `NO_HZ_IDLE`/`NO_HZ_FULL`, `timerfd` and `PR_SET_TIMERSLACK`: Linux is ahead, but toolkits rarely set slack. |

### GUI stack

| Feature | What Haiku does | Why it helps | What a new Linux desktop would need |
|---|---|---|---|
| Threading | Each `BWindow` is a `BLooper` thread at priority 15. app_server runs a matching window thread, plus one per app. A global `MultiLocker`: drawing takes the read lock, window management takes the write lock, and a window thread handles at most 70 messages or 10 ms per read-lock hold. | One slow window can't freeze the others, and the app's main thread is never the UI thread. | GTK and Qt are single-threaded. Copy the per-window thread; the compositor needs a similar lock policy. |
| Drawing model | Server-side: `BView` commands go over a port link to app_server, which rasterizes them on the CPU with AGG into one back buffer in system RAM (always allocated). Dirty regions are copied to the front buffer by the CPU. | Only damaged pixels are drawn and copied. | Wayland is client-side drawing; compositor-owned retained layers are the modern equivalent. |
| Compositing and tearing | No per-window compositing; transparency is limited to 1-bit regions. The back-to-front copy never waits for vertical retrace (the `WaitForRetrace` call is commented out), so tearing is likely (inferred, not tested). | Lowest memory use and no full-frame recomposites. | A compositor with damage-limited, vsynced atomic KMS commits and plane offload. |
| Damage and updates | At most one outstanding `_UPDATE_` per window; new damage piles into a pending region and is copied to the screen once in `EndUpdate`. Exposed areas are filled immediately with the view colour or bitmap. Window moves are blitted and only the uncovered part is redrawn. | Coalescing is automatic and paced by the client; moves feel instant. | Wayland frame callbacks plus damage: the same idea. |
| Retained content | `SetViewBitmap` lets the server paint a view's background on its own. `B_BITMAP_ACCEPTS_VIEWS` gives offscreen views; `B_BITMAP_WILL_OVERLAY` requests a hardware overlay. | Exposed areas are repainted without waking the client, and video bypasses the rasterizer. | Subsurfaces, `wp_viewporter`, KMS planes. |
| GPU and HiDPI | app_server uses no 2D acceleration hooks; copies are in software. 3D is out of tree (RADV with Zink; NVIDIA-Haiku with NVK/Zink for Turing and later). The UI scales with font size, which allows fractional scales. | CPU rendering is predictable. | Render with the GPU; this part can't be copied. |
| Input path | Cursor positions go through a shared area plus a semaphore to the cursor thread at priority 95. Events travel over a port to the event loop at priority 90, then as messages to the window. Mouse moves older than 100 ms are dropped. | The cursor never waits on drawing, and stale moves are discarded. | A dedicated real-time input and cursor thread and a hardware cursor plane. |
| Idle wakeups | `SetPulseRate` starts a timer only when a view sets `B_PULSE_NEEDED`. `BTextView` always sets it at 500 ms, and `Pulse()` runs even when its window is inactive. | Idle cost is low but not zero. | Blink timers that stop when a window loses focus, plus timer slack. |

### Haiku's weaknesses on these goals

- All drawing is on the CPU in system RAM, with no vsync and no compositing. That costs energy on large or HiDPI screens and allows tearing.
- Window moves and resizes (the write lock) can wait about 10 ms behind a busy drawing window.
- There is no bounded-queue policy for events whose delivery fails; it is a TODO in the source.
- Every window with a text view wakes twice a second. No measurement of idle-desktop wakeups was found.
- Power-saving mode is not the default, and where users switch it is unconfirmed.
- There is no timer-slack or deadline-class API.

### Kernel or GUI stack?

Haiku's responsiveness comes mostly from userland policy and from a GUI stack designed as one piece, not from its kernel. The kernel's real contributions are three:

- priority classes that the whole system uses consistently;
- bounded ports with backpressure;
- live queries.

Linux has mechanisms for the first, often better ones (EAS, uclamp, EEVDF slices, sched_ext), but no convention that applies them to UI and input threads. It has no bounded, prioritized message-queue primitive, though one can be built in userland over shared memory. It has no live queries without an indexing daemon. The GUI-stack advantages carry over to any design that owns both the toolkit and the display server:

- per-window threads;
- merged updates paced by the client;
- exposes and moves handled without waking clients;
- a real-time cursor thread;
- dropping stale input;
- overlays.

## 6. Reuse or build

Build, if the project includes the compositor. The largest efficiency win is the Core Animation split:

- the app records layers and commits only the changes;
- the compositor keeps the layers and runs scrolling and animations itself;
- small updates are rasterized on the CPU in the damaged area only;
- cached tiles are composited on the GPU;
- nothing wakes at idle;
- presents are damage-limited, with KMS overlay planes.

That needs a protocol between toolkit and compositor that Wayland does not have, since subsurfaces only get part of the way there, and owning both sides is what makes it possible. On the workstation the Sidestep slice already beat the best reusable toolkit, Qt Quick, by about 1 W on the spinner and on scrolling, at a third to a half of its memory.

Components worth reusing:

- Smithay for the compositor;
- WebRender's tile caching, or vello or tiny-skia, for rasterizing;
- GTK 4's render-tree diffing as the model for damage tracking;
- Haiku's thread-priority policy, bounded queues, and dropping stale input.

If reusing a whole toolkit instead:

- **GTK 4** is the most efficient for mostly-static interfaces: damage on both CPU and GPU, GPU buffers handed to the compositor, and zero idle work. It lacks a render thread and scrolling off the main thread.
- **Qt Quick** suits animation-heavy interfaces better: a render thread and geometry kept on the GPU, but a whole-window redraw.
- **GPUI, iced and egui** redraw everything on every frame. GPUI Fast stops rebuilding unchanged views, but still uploads and redraws the whole scene on the main thread, and on macOS still wakes every refresh while visible.

Whatever the toolkit, the compositor matters for every app on the desktop, GTK and Qt included: damage-limited composition, direct scanout, overlay planes and variable refresh rate. On a discrete GPU, any 60 fps activity costs about 5 W just to keep the card awake, so fewer frames matter more than cheaper frames.

## Sources

Qt:

- https://github.com/qt/qtbase/blob/dev/src/plugins/platforms/wayland/qwaylandshmbackingstore.cpp
- https://github.com/qt/qtbase/blob/dev/src/plugins/platforms/wayland/qwaylandwindow.cpp
- https://github.com/qt/qtbase/blob/dev/src/plugins/platforms/wayland/CMakeLists.txt
- https://doc.qt.io/qt-6/qtquick-visualcanvas-scenegraph.html
- https://doc.qt.io/qt-6/qtquick-visualcanvas-scenegraph-renderer.html
- https://bugreports.qt.io/browse/QTBUG-82860
- https://doc.qt.io/qt-6/qtquick-visualcanvas-adaptations-software.html
- https://github.com/qt/qtdeclarative/blob/dev/src/quick/items/qquickflickable.cpp
- https://github.com/qt/qtdeclarative/blob/dev/src/quick/scenegraph/qsgthreadedrenderloop.cpp

GTK:

- https://gitlab.gnome.org/GNOME/gtk/-/blob/gtk-3-24/gdk/wayland/gdkwindow-wayland.c
- https://gitlab.gnome.org/GNOME/gtk/-/blob/main/gtk/gtkwidget.c
- https://gitlab.gnome.org/GNOME/gtk/-/blob/main/gsk/gskrenderer.c
- https://gitlab.gnome.org/GNOME/gtk/-/blob/main/gsk/gpu/gskgpurenderer.c
- https://gitlab.gnome.org/GNOME/gtk/-/blob/main/gdk/gdkvulkancontext.c
- https://gitlab.gnome.org/GNOME/gtk/-/blob/main/gdk/gdkframeclockidle.c
- https://docs.gtk.org/gtk4/class.GraphicsOffload.html
- https://docs.gtk.org/gtk4/running.html
- https://blog.gtk.org/2024/01/28/new-renderers-for-gtk/
- https://www.phoronix.com/news/GTK-4.16-Released
- https://blog.gtk.org/2025/02/01/whats-new-in-gtk-winter-2025-edition/

Slint:

- https://github.com/slint-ui/slint/blob/master/CHANGELOG.md
- https://github.com/slint-ui/slint (`internal/renderers/software`, `internal/renderers/skia`, `internal/renderers/femtovg`, `internal/backends/winit`)

Flutter:

- https://github.com/flutter/flutter/pull/161879
- https://docs.flutter.dev/release/breaking-changes/linux-merged-threads
- https://github.com/flutter/flutter (`engine/src/flutter/shell/common/rasterizer.cc`, `engine/src/flutter/shell/platform/linux/`)
- https://github.com/flutter/flutter/pull/189584
- https://flutter.dev/blog/whats-new-in-flutter-3-47
- https://docs.flutter.dev/perf/impeller
- https://github.com/flutter/flutter/issues/192915

GPUI:

- https://github.com/zed-industries/zed
- https://github.com/zed-industries/zed/pull/46758
- https://github.com/gpui-ce/gpui-ce
- https://gpui-ce.github.io/
- https://crates.io/crates/gpui
- https://github.com/longbridge/gpui-fast (evaluated at commit 7ab23f46f2ba3a040ceb27d387383a2896bc5ae1: `crates/gpui/src/fast/retained.rs`, `docs/architecture.md`, `crates/gpui_apple/src/metal_renderer.rs`, `crates/gpui_macos/src/window.rs`)
- the local crate `gpui-0.2.2` (`src/window.rs`, `src/platform/mac/window.rs`, `src/platform/linux/wayland/window.rs`)

iced and egui:

- https://github.com/iced-rs/iced/releases/tag/0.14.0
- https://github.com/iced-rs/iced/pull/2662
- https://github.com/iced-rs/iced
- https://github.com/emilk/egui

Chromium:

- https://chromium.googlesource.com/chromium/src/+/HEAD/docs/how_cc_works.md
- https://github.com/chromium/chromium (`cc/scheduler/scheduler.cc`, `components/viz/service/display/direct_renderer.cc`, `ui/ozone/platform/wayland/`)
- https://groups.google.com/a/chromium.org/g/ozone-reviews/c/-3IYmgHuW1A
- https://archive.fosdem.org/2024/events/attachments/fosdem-2024-3177-delegated-compositing-utilizing-wayland-protocols-for-chromium-on-chromeos/slides/22799/Delegated_Compositing_b3OqHfM.pdf

Firefox:

- https://firefox-source-docs.mozilla.org/gfx/RenderingOverview.html
- https://github.com/servo/webrender
- https://bugzilla.mozilla.org/show_bug.cgi?id=1648872
- https://bugzilla.mozilla.org/show_bug.cgi?id=1699985
- https://mastransky.wordpress.com/2026/01/23/firefox-linux-in-2025/

Android:

- https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/libs/hwui
- https://developer.android.com/topic/performance/rendering/profile-gpu

EFL:

- https://www.enlightenment.org/about-wayland
- https://www.enlightenment.org/program_guide/evas/ui_rendering_mode
- https://github.com/Enlightenment/efl

Haiku:

- https://github.com/haiku/haiku (`src/system/kernel/scheduler/`, `src/system/kernel/port.cpp`, `src/system/kernel/timer.cpp`, `src/servers/app/`, `src/servers/input/InputServer.cpp`, `src/kits/interface/`, `headers/os/kernel/OS.h`)
- https://www.haiku-os.org/blog/pawe%C5%82_dziepak/2013-12-20_haiku_meets_9th_processor/
- https://www.haiku-os.org/blog/pawe%C5%82_dziepak/2014-02-18_new_scheduler_merged/
- https://www.haiku-os.org/legacy-docs/bebook/TheKernelKit_Ports.html
- https://www.haiku-os.org/legacy-docs/bebook/BQuery.html
- https://www.haiku-os.org/legacy-docs/benewsletter/Issue1-26.html
- https://discuss.haiku-os.org/t/how-to-transform-the-app-server-code-to-use-compositing-haiku-project/9230?page=2
- https://www.phoronix.com/news/Haiku-OS-Zink-RADV
- https://www.phoronix.com/news/NVIDIA-Haiku-0.0.1
- https://www.haiku-os.org/get-haiku/r1beta4/release-notes/
