# Target design: an efficient, responsive Linux desktop

A target design for a compositor, a GUI toolkit and a desktop environment for Linux, built for responsiveness and energy efficiency together. It takes its structure from Haiku: one design for the whole stack, with every thread's role defined. It takes its rendering model from Core Animation: retained layers that the display server keeps and animates. The evidence behind each choice is in [UI_TOOLKIT_EFFICIENCY.md](UI_TOOLKIT_EFFICIENCY.md). Nothing here is built yet except the Sidestep slice, which prototypes the toolkit's render path. Numbers marked as targets are goals to measure against, not results.

## 1. Goals and budgets

Responsiveness means:

- input reaches the screen within one refresh;
- the cursor never waits on anything;
- scrolling and animation never wait on an app's main thread;
- an app's main thread is never blocked.

Energy efficiency means:

- nothing wakes when nothing changes;
- a small change costs a small amount of work, end to end;
- the GPU and the display stay in their low-power states whenever they can.

| Area | Target |
|---|---|
| Idle app | Zero wakeups per second from the toolkit itself. |
| Idle compositor | Zero compositions when nothing changes, and no periodic repaint loop. |
| Blinking caret | Zero app wakeups. The compositor composes twice per blink cycle, damaging only the caret's rectangle. |
| Cursor motion | The hardware cursor plane is updated within 1 ms of the input event, independent of composition. |
| Input to screen | p99 within one refresh interval plus scanout for typing in a native app (about 12 ms at 120 Hz). |
| App main thread | p99 pass under 4 ms (the budget Omperor's frame ledger already enforces); never waits on the GPU, a present, an IPC reply, the disk or the network. |
| Scrolling and animation | No per-frame work on the app's main thread, and no dropped frames at display refresh while the app is busy. |
| Energy, workstation | 60 fps spinner and continuous scroll at or below the Sidestep slice: 5.2 W and 5.7 W. |
| Energy, laptop | Targets set after the first integrated-GPU measurements. |
| Memory | A simple native app under 60 MB PSS (the slice measured 44–86 MB). |

## 2. Principles

1. **One owner for the whole stack.** The toolkit, compositor, shell and system policy are designed together, as Haiku's kits, app_server and kernel policy are. That lets the protocol between them carry what Wayland cannot: retained layers, compositor-run animation and asynchronous scrolling.
2. **Every thread has a role, and the role decides its scheduling.** Haiku gives window threads display priority, the cursor 95 and input 103. Linux has better mechanisms than Haiku but no convention for using them, so this design adds one.
3. **Never wake for nothing.**
   - no blink timers in apps;
   - no polling;
   - no frame clock unless there is a frame to draw;
   - timers coalesced with slack.
4. **The server keeps the pixels.** In Haiku, app_server fills exposed areas from a view bitmap without waking the app, and window moves are blits. In Core Animation, the render server keeps every layer. Here the compositor keeps each window's layers and tiles, so exposes, moves, scrolling and animation don't involve the app.
5. **Damage end to end.** Invalidation in a view becomes damage on a layer's tiles, then damage in the composition, then damage clips handed to KMS. Nothing redraws or recomposites pixels that didn't change.
6. **Bounded queues with priority lanes.** Haiku's ports block a writer when the queue is full, and its app_server drops mouse moves older than 100 ms. Here, every queue between processes is bounded, input never waits behind rendering, and stale pointer motion is dropped.
7. **Small updates on the CPU, large motion on the GPU.** On the workstation, CPU rasterization with damage tracking was the cheapest way to make small updates. Full frames drawn on the CPU and uploaded were the most expensive way to make large ones. Scrolling and animation should move cached content on the GPU instead.
8. **No app-global UI bottleneck.** Haiku gives every window its own thread. Here, every window has its own render thread, and native apps can opt into a UI thread per window.

## 3. Architecture

| Process | Threads and roles | Responsibilities |
|---|---|---|
| Linux kernel (stock, 6.12 or later) | – | EEVDF with custom slices, `sched_ext`, uclamp, cgroup v2, atomic KMS with damage clips, dmabuf, memfd, sync objects. No kernel changes required. |
| QoS service (privileged, small) | One thread | Assigns scheduling to declared thread roles. Moves apps between foreground and background cgroups. Applies power profiles. Optionally loads a `sched_ext` policy. |
| Compositor | Input (real-time); a render thread per output (real-time); protocol threads (interactive); shell logic (interactive); workers (utility) | KMS, plane assignment, composition, input, cursor, the standard Wayland protocols, the native layer protocol, window management. |
| Native app (toolkit) | Main/UI (interactive); a render thread per window (interactive); workers (utility, or background when unfocused) | Event handling, view tree, layout, recording drawing, rasterizing damaged tiles, committing layer transactions. |
| Shell apps | Same as any native app | Panel, launcher, notifications, lock screen, settings, file manager. |
| Indexer | Background | File attributes and live queries. |
| Foreign apps (GTK, Qt, Chromium, XWayland) | Their own | Standard Wayland, composited as efficiently as possible. |

## 4. Thread roles and scheduling

Apps never call the scheduler directly. The toolkit declares each thread's role to the QoS service over a socket, and the service applies the policy, as rtkit does for audio but for every role. Real-time roles carry `RLIMIT_RTTIME` and a watchdog, so a runaway real-time thread is demoted instead of locking the machine.

| Role | Examples | Linux mechanism | Notes |
|---|---|---|---|
| Input | libinput reader, cursor updates | `SCHED_FIFO`, high priority, memory locked | Tiny, bounded work. Haiku's equivalents run at 95 and 103. |
| Output | The compositor's per-output render thread | `SCHED_FIFO` below input, or `SCHED_DEADLINE` with a runtime taken from measured composition time | Must finish before the KMS commit deadline. |
| Interactive | Main thread and window render threads of the focused app; compositor protocol threads | Normal class with a short EEVDF slice (`sched_attr.sched_runtime`) and a high `uclamp.min`, so the CPU ramps at once | The equivalent of Haiku's display priorities. |
| Utility | Image decoding, shaping and layout prefetch | Normal class, no clamp | |
| Background | Indexing, sync, workers of unfocused or hidden apps | `SCHED_IDLE` or a low `cpu.weight`, `uclamp.max` around 30%, timer slack of 50–100 ms | Keeps fast cores and high frequencies for foreground work. |
| Audio | PipeWire | Its existing real-time setup | Unchanged. |

Focus drives the policy. When a window gains focus, its app moves to the foreground cgroup and its threads get the interactive role. When an app is hidden or minimized, it moves to the background cgroup, its frame callbacks stop, and its timer slack grows. Apps that opt in can be frozen (`cgroup.freeze`) after a period hidden and thawed when shown, like App Nap on macOS.

An optional `sched_ext` policy can go further than EEVDF can. Haiku's power-saving mode packs threads and interrupts onto one core until it passes 70% load, so the other cores stay in deep sleep. A BPF scheduler can do the same on Linux and read thread roles straight from a map. It is optional because `sched_ext` support varies across distributions; EEVDF with uclamp and energy-aware scheduling is the default.

## 5. Compositor

### 5.1 Base

Built on Smithay. It drives atomic KMS directly and assigns planes the way libliftoff does: direct scanout when a window covers an output, and overlay planes for video and other suitable surfaces. It supports variable refresh, tearing control for games, fractional scaling and color management.

For foreign apps it implements the standard Wayland protocols that matter for efficiency:

- linux-dmabuf with feedback, to steer clients to formats that can be scanned out;
- presentation-time;
- fifo;
- commit-timing;
- tearing-control;
- viewporter;
- fractional-scale;
- content-type;
- the explicit-sync protocol (linux-drm-syncobj).

XWayland runs legacy X11 apps.

### 5.2 Frame scheduling

- **Event-driven.** The compositor composes only when there is new damage on an output: a client commit, a running layer animation, an asynchronous scroll, or a cursor change that couldn't go to the cursor plane. There is no periodic repaint, and frame callbacks go only to clients that asked for one.
- **Late composition.** Each frame starts at the latest safe moment: the next vblank minus the predicted composition time, minus a margin. The prediction comes from recent composition times, as Mutter and KWin do. That keeps the time from input to screen short.
- **Damage-limited composition.** Only damaged regions are composited, using buffer age. The damage goes to KMS as `FB_DAMAGE_CLIPS`, which lets a laptop panel do selective self-refresh (PSR2) and, when nothing changes, lets the display engine and GPU sleep. If every visible surface sits on a plane, the GPU doesn't composite at all.
- **Lower refresh when little moves.** On variable-refresh displays the compositor lets the refresh rate fall when only slow animations run, such as a caret blink. That is where the 5 W "GPU awake at 60 fps" cost measured on the workstation is saved.

### 5.3 Input and cursor

- The input thread reads libinput and moves the hardware cursor plane directly, through the driver's asynchronous cursor path where it exists. The cursor never waits on composition, as in Haiku, where cursor positions go through shared memory to a priority-95 thread.
- Each client gets bounded queues with three priority lanes:
  - key and button events, never dropped;
  - pointer motion and scroll, merged to one per frame per client, with motion older than 100 ms dropped (as Haiku does);
  - everything else.
- Scroll gestures over a native scroll layer are handled in the compositor itself (Section 6.4). The app hears about the new offset afterwards.
- When a client stops responding, its windows keep scrolling and animating, because the compositor owns the layers. After a deadline without acknowledged input (2 s, for example), the window is marked busy.

### 5.4 Concurrency

There is no global lock on the hot path. Protocol threads apply client transactions to a per-window state, and each output's render thread takes an immutable snapshot of the scene, double-buffered or read-copy-update. That avoids the stall Haiku's global lock allows, where a window move can wait about 10 ms behind a busy drawing window. Version 1 is a single process. Moving the render threads into a separate process, as Chromium does with its GPU process, is a later option if crash isolation proves worth the cost.

## 6. The layer protocol

The native protocol between the toolkit and the compositor, modelled on Core Animation. Over standard Wayland it falls back to subsurfaces, as the Sidestep slice already does, so native apps still run on GNOME and KDE in a degraded mode.

### 6.1 Layers and transactions

Each window owns a tree of layers. A layer has:

- geometry: bounds, position and anchor point;
- a 2D affine transform;
- opacity;
- a clip, with an optional corner radius;
- a background colour;
- a border;
- shadow parameters, which the compositor draws;
- contents;
- for scroll layers, a scroll offset.

A client sends only the changes, grouped into transactions that the compositor applies atomically at its next frame. Each window may have a few transactions in flight (three, for example). When that limit is reached the client merges its next changes or waits, which is Haiku's port backpressure applied to rendering.

### 6.2 Contents and tiles

A layer's contents are a grid of tiles, for example 256 or 512 device pixels square. Each tile is a buffer from a pool the client owns: memfd shared memory for CPU-rasterized tiles, and dmabuf for GPU-rasterized ones. To update, the client draws into a free tile buffer and swaps it in with a damage rectangle, and the compositor imports or uploads only that.

- **Integrated GPUs.** These share memory with the CPU, so CPU tiles are imported without copying (udmabuf).
- **Discrete GPUs.** The client rasterizes large damage on the GPU, and uploads only the damaged rectangles of CPU tiles. That avoids the full-frame uploads that cost the workstation's compositor 0.3 of a core and 20 W.

GPU-rendered tiles use explicit sync objects; CPU tiles use buffer-release events.

### 6.3 Animations

A client attaches an animation to a layer property:

- keyframes, a duration, and a timing curve or spring parameters;
- a repeat count;
- a start time on the presentation clock.

The compositor interpolates the property every frame and reports when the animation completes. The app's threads do nothing while it runs. A caret blink is a repeating opacity animation with stepped timing, owned by the compositor, which stops it when the window loses focus. That removes the twice-a-second wakeup Haiku's text views still have, and GPUI's 123 wakeups a second.

### 6.4 Scroll layers

A scroll layer has contents larger than its bounds. The compositor applies scroll input to it directly, with momentum and edge behaviour from settings, and tells the client the new offset afterwards. It works like Firefox's APZ and Chromium's compositor-thread scrolling.

The client keeps tiles rasterized for a margin around the visible area, larger in the direction of motion and scaled by velocity. The Omperor GPUI port's transcript measures 3.5 screens ahead in the scroll direction and 1 behind, as a reference. A tile that isn't ready shows the layer's background colour, and the client's prefetch makes that rare. Content that is virtualized (long lists, transcripts) is materialized by the client from its offset updates, off the main thread where possible.

### 6.5 Where text is rasterized

In version 1 the client rasterizes text into its tiles. Haiku draws text on the server, which saves per-app memory and lets the server re-rasterize at a new scale. But it would put shaping and font handling inside the compositor, a larger trusted process. A shared glyph cache in the compositor is a possible later step if per-app glyph memory proves significant.

## 7. Toolkit

### 7.1 Threads

- **Main (UI) thread.** Handles events, mutates the view tree, runs layout and records drawing into display lists. It is held to the 4 ms budget and has no blocking APIs. Disk, network, IPC replies and GPU work are only ever asynchronous, the rule Omperor's AGENTS.md already applies to its own main thread.
- **Render thread per window.** Turns display lists into rasterized tiles for damaged regions, manages the tile pools, and commits layer transactions.
- **Worker pool.** Image decoding, shaping and layout prefetch for virtualized content. Utility role, or background when the window is unfocused.
- **Per-window UI threads (native API).** Native apps with several heavy windows can opt into a UI thread per window, as Haiku's `BWindow` has. The AppKit-compatible API (Sidestep) keeps AppKit's single main thread, but still gets a render thread per window.

### 7.2 Invalidation, layout and damage

- **Invalidation.** It unions damage per layer.
- **Layout.** Layout is incremental: dirty bits per subtree, with measurements cached by constraint. Text layout is cached by content, width and style, as Omperor's height cache does.
- **Layer backing.** A view gets its own layer when it animates, scrolls, or is expensive to redraw. Otherwise it draws into its nearest layer's tiles, the same split as Core Animation's layer-backed and ordinary views. Each layer costs compositor memory, so their number is kept modest.
- **Drawing and raster.** Drawing is recorded as display lists per view. Rasterization covers only damaged tile regions:
  - a CPU rasterizer (tiny-skia or vello_cpu class) for small damage;
  - a GPU rasterizer (vello on wgpu) for large damage, or for any damage above a size threshold on discrete GPUs.

  The threshold is measured per device class.

### 7.3 Frame pacing and timers

- **At idle.** There is no frame clock at idle. The toolkit requests a frame only when it has work, aims at a presentation time with commit-timing, and uses presentation feedback to stay aligned.
- **Animations.** Animations belong to the compositor. Animations drawn by the app, such as a custom spinner, use frame callbacks and are discouraged in shell components.
- **Timers.** Timers are coalesced with slack. Cross-thread wakeups go through futex or eventfd, never through polling.

### 7.4 Text, accessibility and input methods

- **Text.** Shaping with HarfBuzz (or rustybuzz), with glyph caching and subpixel positioning.
- **Accessibility.** Accessibility goes through AT-SPI on its own thread. The tree is built only when an assistive client connects, so it costs nothing otherwise.
- **Input methods.** Input methods use text-input-v3.

These are designed in from the start, not retrofitted.

### 7.5 API

A Rust-native API with retained views, and the AppKit-compatible API from Sidestep, both on the same layer and render engine. Omperor and Sidestep apps are the first real clients.

## 8. Desktop environment

- **Shell components are ordinary toolkit apps with strict idle budgets.** That covers the panel, launcher, notifications, lock screen, settings and file manager. The panel clock updates on the minute boundary, with slack. Seconds and live widgets are off by default. The wallpaper is static, and is scanned out from its own plane where the hardware allows.
- **App lifecycle.** The focused app runs in the foreground cgroup, and visible but unfocused apps run normally. Hidden or minimized apps go to the background cgroup: low `cpu.weight`, low `uclamp.max`, long timer slack and no frame callbacks. Apps that opt in are frozen after a while hidden and thawed when shown.
- **Files and live queries.** A background indexer driven by fanotify keeps file attributes (extended attributes plus a database). It offers Haiku-style live queries, which push updates when matching files change, and node monitoring for file-manager windows. Nothing in the shell polls the file system.
- **IPC.** Native components use the bounded, prioritized IPC described in Section 9. A D-Bus-compatible bus remains for existing apps.
- **Power profiles.** Performance, balanced and battery profiles set four things together:
  - the QoS policy (the `sched_ext` mode, or energy-aware scheduling settings);
  - the compositor's maximum refresh and use of variable refresh;
  - how much the toolkit animates;
  - how hard background apps are clamped.
- **Wakeup audits.** Every shell component's idle wakeups are measured in CI, and a regression fails the build.

## 9. IPC

Every channel between native processes is a shared-memory ring buffer, with eventfd or futex wakeups and a fixed capacity. Haiku's ports block a writer when the queue is full; the same rule applies here.

- **Lanes.** Each channel has separate lanes for input, layer transactions and everything else, so input never waits behind rendering.
- **Full queues.** When a lane is full, the sender merges its next message where the message type allows (pointer motion, damage, property updates) or waits.
- **Busy channels.** A busy channel makes no system calls, since the reader drains the ring without being woken.
- **Large payloads.** Tiles, images and documents travel as memfd or dmabuf file descriptors, not through the ring, as Haiku moves large link payloads through a transferred area.

## 10. Where each idea comes from

| Idea | Source | Where it lands |
|---|---|---|
| A thread per window; display, urgent and real-time priority classes used system-wide | Haiku | Toolkit render threads, the QoS service |
| Bounded ports with writer backpressure | Haiku | IPC lanes, transaction limits |
| Server-held pixels for exposes and moves; view bitmaps | Haiku | Compositor-owned layers and tiles |
| A real-time cursor thread; dropping stale motion | Haiku | The compositor's input thread and lanes |
| Live queries and node monitoring | Haiku (kernel) | The indexer, in userland |
| Packing threads and interrupts onto one core for power saving | Haiku (kernel) | Optional `sched_ext` policy |
| A retained layer tree in the server; animations run by the server | Core Animation | The layer protocol |
| Layer-backed and ordinary views | AppKit | Toolkit layer policy |
| Tiles, asynchronous scrolling, prefetch to avoid blank tiles | Chromium, WebRender | Scroll layers |
| Render-tree diffing for damage; offloading content to subsurfaces; a frame clock that stops at idle | GTK 4 | Toolkit damage tracking, compositor planes |
| A render thread; skipping empty frames; partial updates | Android HWUI | Toolkit render thread, damage-limited presents |
| CPU raster for small updates; never uploading full CPU frames; fewer frames matter more than cheaper ones | Workstation measurements | Raster thresholds, frame scheduling |

## 11. Measurement and acceptance

The workstation harness is extended and run on two machines: the workstation (discrete GPU) and an integrated-GPU laptop. Scenarios:

- idle;
- a blinking caret;
- typing;
- a 60 fps spinner;
- continuous scrolling;
- a window resize;
- streaming text.

Measurements:

- **Energy:** CPU energy from RAPL through a privileged helper, plus per-process `ri_energy_nj` where available; GPU board power on the workstation.
- **Wakeups:** perf sched or powertop.
- **Frame timing:** presentation feedback.
- **Input to screen:** a photodiode or high-speed camera, plus a software proxy (the presentation timestamp minus the input timestamp).
- **Main-thread passes:** Omperor's frame-budget ledger.
- **Memory:** PSS and VRAM.

Each component has gates that fail a regression:

- the compositor composes nothing at idle;
- the caret costs at most two compositions a second, damaging only the caret's rectangle;
- the spinner and scroll scenarios stay at or below the Sidestep slice baseline;
- GTK, Qt and Chromium apps run at least as efficiently as under sway, KWin or Mutter.

## 12. Phases

1. **Policy and measurement first.** The QoS service and the harness, used with an existing compositor (sway or KWin), to measure how much thread roles alone are worth.
2. **Compositor version 1 on Smithay.** Standard protocols with damage-limited composition, planes, late composition, KMS damage clips and the asynchronous cursor. Host GTK, Qt and Chromium apps, and measure against sway, KWin and Mutter.
3. **The layer protocol and the toolkit render engine,** grown from the Sidestep slice: tiles, compositor-run animations and asynchronous scrolling. Omperor and Sidestep apps are the first clients.
4. **Toolkit breadth:** text, controls, accessibility and input methods. Then the shell components.
5. **The rest:** app lifecycle (foreground and background cgroups, freezing), live queries, and the optional `sched_ext` policy.

## 13. Risks and open questions

- **Compositor complexity.** Compositor-run animation and scrolling put more logic in a privileged process, so a bug there takes down the desktop. Mitigations: a small, declarative layer model, fuzzing of the protocol, and possibly a separate render process later.
- **Native apps only.** The layer protocol helps only native apps; foreign apps get the efficiency of the standard protocols.
- **Tile memory at high resolution.** One full-screen layer at 5K is about 59 MB at 4 bytes per pixel, so tiles need budgets, eviction, and compressed formats for tiles that are off screen.
- **CPU versus GPU raster.** The point where one beats the other differs between integrated and discrete GPUs and has to be measured per device class.
- **Scheduler support.** `sched_ext` maturity and distribution support vary, so EEVDF with uclamp stays the default.
- **Real-time threads.** They need watchdogs and `RLIMIT_RTTIME`, so a bug can't lock up the machine.
- **Driver variation.** Support for asynchronous cursor updates and `FB_DAMAGE_CLIPS` varies by driver, so the compositor needs fallbacks for both.
- **Accessibility and input methods** must work from the first release of the toolkit, since retrofitting them is costly.
