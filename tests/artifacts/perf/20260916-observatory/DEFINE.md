# Observatory performance check

Goal: remove idle wakeups and repeated native view construction without changing task identity, accessible labels, ordering, pinned evidence, saved scroll anchors, or typography. Scope: the native 84-task fixture plus code inspection of live runtime polling and journal loading. No inference or production user data is used.

Budgets: no periodic preview work after background jobs settle; native scroll callback p95 below 8 ms; only entering rows constructed when scrolling. Each comparison uses 20 UI actions and 20 one-second idle samples. Native span duration excludes tool transport latency. Profile logging is enabled for both interaction runs and disabled for both idle runs.

CPU counters use the host Mach timebase, verified against a process CPU-time loop; wakeups are libproc counters. These are energy proxies, not battery-life measurements. Other apps and agents remain running; no system power settings are changed.
