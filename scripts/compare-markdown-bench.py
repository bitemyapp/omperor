#!/usr/bin/env python3
"""Compare `omperor --markdown-bench` across binaries.

Runs each binary N times (default 3), interleaved and rotated so no binary
always goes first, each run in a fresh scratch HOME the benchmark fills in
itself, then prints the median of every metric side by side with the ratio
to the first binary.

    scripts/compare-markdown-bench.py target-bench/release/omperor \\
        ../omperor-textkit2/target-bench/release/omperor --runs 3

The runs are headless: the app never activates and its window never reaches
a screen. Nothing outside the scratch homes and --out is written.
"""

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# Metrics per phase, as (label, path into the phase's JSON, unit).
PHASE_METRICS = [
    ("passes", ("passes",), "n"),
    ("pass p50", ("pass_us", "p50"), "us"),
    ("pass p90", ("pass_us", "p90"), "us"),
    ("pass p99", ("pass_us", "p99"), "us"),
    ("pass max", ("pass_us", "max"), "us"),
    ("pass mean", ("pass_us", "mean"), "us"),
    ("pass total", ("pass_us", "total"), "us"),
    ("over 4 ms", ("over_4ms",), "n"),
    ("over 8.33 ms", ("over_8_33ms",), "n"),
    ("wall", ("wall_ms",), "ms"),
    ("turn", ("turn_ms",), "ms"),
    ("first draw", ("first_draw_ms",), "ms"),
    ("first complete frame", ("first_complete_frame_ms",), "ms"),
    ("harness draws", ("draws", "harness", "count"), "n"),
    ("harness draw total", ("draws", "harness", "us", "total"), "us"),
    ("appkit draws", ("draws", "appkit", "count"), "n"),
    ("appkit draw total", ("draws", "appkit", "us", "total"), "us"),
    ("invalidated passes", ("invalidated_passes",), "n"),
]

# Spans reported per phase (total µs), when a build has them.
SPANS = [
    "gui.tick",
    "gui.render_chat_transcript",
    "gui.markdown.render_message",
    "gui.markdown.parse",
    "gui.transcript.storage_edit",
    "gui.transcript.ensure_layout",
    "gui.transcript.scroll",
    "gui.bench.draw",
    "gui.bench.appkit_draw",
]

# The environment a run must not inherit: it would point the app at real data
# or change what the benchmark measures.
SCRUBBED = ["OMPEROR_PROJECT", "OMPEROR_PERF_LOG", "OMPEROR_MARKDOWN_BENCH_ORDER_IN", "OMPEROR_MODEL"]


def dig(value, path):
    for key in path:
        if not isinstance(value, dict) or key not in value:
            return None
        value = value[key]
    return value if isinstance(value, (int, float)) else None


def run_once(binary, out_dir, label, index, timeout):
    home = Path(tempfile.mkdtemp(prefix="home-", dir=out_dir / "homes"))
    report = out_dir / f"{label}.run{index}.json"
    log = out_dir / f"{label}.run{index}.log"
    env = {key: value for key, value in os.environ.items() if key not in SCRUBBED}
    env["HOME"] = str(home)
    # HOME alone leaves CFPreferences, and so AppKit's saved window state, in
    # the real ~/Library/Preferences.
    env["CFFIXED_USER_HOME"] = str(home)
    started = time.monotonic()
    with open(log, "w") as stream:
        result = subprocess.run(
            [str(binary), "--markdown-bench", str(report)],
            env=env,
            cwd=home,
            stdout=stream,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
    elapsed = time.monotonic() - started
    shutil.rmtree(home, ignore_errors=True)
    if result.returncode != 0 or not report.exists():
        sys.exit(f"{binary} run {index} failed (exit {result.returncode}); see {log}")
    print(f"  {label} run {index}: {elapsed:.0f} s", flush=True)
    return json.loads(report.read_text())


def label_for(binary, report, taken):
    build = report.get("build") or {}
    commit = (build.get("commit") or "")[:8]
    branch = build.get("branch") or ""
    label = f"{branch}@{commit}" if commit else Path(binary).parent.parent.name
    label = label.replace("/", "-") or "binary"
    base, n = label, 2
    while label in taken:
        label, n = f"{base}-{n}", n + 1
    return label


def fmt(value, unit):
    if value is None:
        return "-"
    if unit == "us":
        return f"{value / 1000:.2f}"
    if unit == "ms":
        return f"{value:.1f}"
    return f"{value:g}" if isinstance(value, float) else str(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("binaries", nargs="+", type=Path, help="omperor executables to compare; the first is the baseline")
    parser.add_argument("--runs", type=int, default=3, help="runs per binary (default 3)")
    parser.add_argument("--out", type=Path, help="directory for reports and logs (default: a new temporary directory)")
    parser.add_argument("--timeout", type=int, default=600, help="seconds a run may take (default 600)")
    args = parser.parse_args()

    binaries = [binary.resolve() for binary in args.binaries]
    for binary in binaries:
        if not os.access(binary, os.X_OK):
            sys.exit(f"not an executable: {binary}")
    out_dir = (args.out or Path(tempfile.mkdtemp(prefix="omperor-markdown-bench-"))).resolve()
    (out_dir / "homes").mkdir(parents=True, exist_ok=True)
    print(f"reports in {out_dir}")

    reports = {index: [] for index in range(len(binaries))}
    for run in range(args.runs):
        # Rotate the order each round so warm-up favours no binary.
        order = [(run + offset) % len(binaries) for offset in range(len(binaries))]
        for index in order:
            reports[index].append(run_once(binaries[index], out_dir, f"b{index}", run, args.timeout))

    labels = []
    for index, binary in enumerate(binaries):
        labels.append(label_for(binary, reports[index][0], labels))

    phases = []
    for report in reports[0]:
        for phase in report.get("phases", []):
            if phase["name"] not in phases:
                phases.append(phase["name"])

    def medians(index, phase_name):
        rows = {}
        per_run = [
            next((phase for phase in report.get("phases", []) if phase["name"] == phase_name), {})
            for report in reports[index]
        ]
        for label, path, unit in PHASE_METRICS:
            values = [value for value in (dig(phase, path) for phase in per_run) if value is not None]
            rows[label] = (statistics.median(values) if values else None, unit)
        for span in SPANS:
            values = [dig(phase, ("spans", span, "total_us")) for phase in per_run]
            values = [value for value in values if value is not None]
            rows[span] = (statistics.median(values) if values else None, "us")
        return rows

    summary = {"binaries": [str(binary) for binary in binaries], "labels": labels, "runs": args.runs, "phases": {}}
    width = max(14, *(len(label) + 2 for label in labels))
    header = f"{'metric':<30}" + "".join(f"{label:>{width}}" for label in labels)
    header += "".join(f"{'÷ ' + labels[0][:8]:>12}" for _ in labels[1:])
    for phase_name in phases:
        table = [medians(index, phase_name) for index in range(len(binaries))]
        summary["phases"][phase_name] = {
            labels[index]: {metric: value for metric, (value, _) in table[index].items()} for index in range(len(binaries))
        }
        print(f"\n{phase_name}  (median of {args.runs}; times in ms)")
        print(header)
        for metric, (base, unit) in table[0].items():
            values = [table[index][metric][0] for index in range(len(binaries))]
            if all(value is None for value in values):
                continue
            line = f"{metric:<30}" + "".join(f"{fmt(value, unit):>{width}}" for value in values)
            for value in values[1:]:
                ratio = "-" if value is None or not base else f"{value / base:.2f}×"
                line += f"{ratio:>12}"
            print(line)
    (out_dir / "compare.json").write_text(json.dumps(summary, indent=2))
    print(f"\nmedians in {out_dir / 'compare.json'}")


if __name__ == "__main__":
    main()
