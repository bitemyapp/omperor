#!/usr/bin/env python3
"""Read per-process macOS CPU/wakeup counters; no power settings are changed."""
import argparse
import ctypes
import json
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("pid", type=int)
parser.add_argument("--samples", type=int, default=20)
parser.add_argument("--interval", type=float, default=1.0)
args = parser.parse_args()

class Usage(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_ubyte * 16)] + [(name, ctypes.c_uint64) for name in (
        "user_ticks", "system_ticks", "idle_wakeups", "interrupt_wakeups", "pageins",
        "wired_bytes", "resident_bytes", "footprint_bytes", "start", "exit")]

lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
lib.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
lib.proc_pid_rusage.restype = ctypes.c_int

class Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]

timebase = Timebase()
ctypes.CDLL("/usr/lib/libSystem.B.dylib").mach_timebase_info(ctypes.byref(timebase))
tick_ns = timebase.numer / timebase.denom

def read():
    value = Usage()
    if lib.proc_pid_rusage(args.pid, 0, ctypes.byref(value)) != 0:
        raise OSError(ctypes.get_errno(), "proc_pid_rusage failed")
    return time.monotonic(), value

previous_time, previous = read()
for sample in range(args.samples):
    time.sleep(args.interval)
    now, value = read()
    seconds = now - previous_time
    print(json.dumps({
        "sample": sample, "seconds": seconds,
        "cpu_ms": (value.user_ticks + value.system_ticks - previous.user_ticks - previous.system_ticks) * tick_ns / 1e6,
        "idle_wakeups": value.idle_wakeups - previous.idle_wakeups,
        "interrupt_wakeups": value.interrupt_wakeups - previous.interrupt_wakeups,
        "footprint_bytes": value.footprint_bytes,
    }), flush=True)
    previous_time, previous = now, value
