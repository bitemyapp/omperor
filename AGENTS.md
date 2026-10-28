# Omperor agent guidelines

## Never block the interface thread

Do not perform non-trivial compute, I/O (including OMP RPC interactions), or shell commands on the interface thread, where the kit runs `update` and the views. Anything that can take longer than a frame — git and other subprocesses, filesystem walks or even a single file check, network requests, waiting on the OMP child — runs on a background worker (`cx.spawn`, the runtime thread, or a dedicated thread) and reports back with a message. While such work is in flight, the interface must show an active indicator that work is being done and keep the window responsive.
