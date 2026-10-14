# Omperor agent guidelines

## Never block the GUI main thread

Do not perform non-trivial compute, I/O (including OMP RPC interactions), or shell commands on the AppKit main thread. Anything that can take longer than a frame — git and other subprocesses, filesystem walks, network requests, waiting on the OMP child — runs on a background worker (the job pool, the runtime thread, or a dedicated discovery thread) and reports back through a channel that the main-thread timer polls. While such work is in flight, the interface must show an active indicator that work is being done and keep the window responsive.
