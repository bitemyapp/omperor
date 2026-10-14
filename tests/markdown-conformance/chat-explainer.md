# How the frame budget works

Every pass of the main run loop is timed, from wake to wait. A pass over **4 ms** (one frame at 240 Hz) is written down with its longest spans.

## What counts

- [x] Timers, sources, and Core Animation's commit
- [x] Event dispatch, named by type
- [ ] Work on other threads — it never blocks a frame

### The arithmetic

At 240 Hz a frame lasts $1000 / 240 \approx 4.17$ ms; the budget rounds down to 4 ms.[^budget]

![The crowned gate](markdown-stress.png)

Keys: <kbd>⌘</kbd>+<kbd>,</kbd> opens Settings › Developer.  
The readout sits above the composer. 🚀

[^budget]: A 120 Hz display would allow 8.33 ms.
