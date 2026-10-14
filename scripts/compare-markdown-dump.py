#!/usr/bin/env python3
"""Compare two `omperor --markdown-dump` directories.

    scripts/compare-markdown-dump.py DUMP_A DUMP_B REPORT_DIR

For every capture both dumps hold (`<input>.<style>.<appearance>.json`), it
compares what a reader sees — the visible text as a word sequence, the link
set, the code text, and the bold and italic text — after normalizing
whitespace and the glyphs a renderer draws for structure rather than content
(list bullets, checkboxes, rules, image placeholders). It lists the
differences per capture and writes REPORT_DIR/report.html: a contact sheet
of the two PNGs side by side (A | B) with the differences under each pair.
The format is described in docs/MARKDOWN_CONFORMANCE.md.
"""

import argparse
import collections
import difflib
import html
import json
import os
import re
import sys
import unicodedata
from pathlib import Path

# Drawn for structure, not content: bullets, checkboxes, rules, the object
# replacement character an attachment occupies, zero-width marks.
GLYPHS = set(
    "•◦▪▫‣⁃∙●○■□▢◆◇▸▹►▻"  # list bullets and disclosure marks
    "☐☑☒✓✔✗✘◻◼◽◾"  # task checkboxes
    "─━―═│┃"  # rules and quote bars
    "￼​‌‍⁠﻿­"  # attachments, zero-width, soft hyphen
)
RULE = re.compile(r"^[-_*=─━―═]{3,}$")


def normalize(text):
    text = unicodedata.normalize("NFC", text or "")
    text = "".join(" " if char in GLYPHS else char for char in text)
    return re.sub(r"\s+", " ", text).strip()


def words(text):
    return [word for word in normalize(text).split(" ") if word and not RULE.match(word)]


def runs(values):
    return [run for run in (normalize(value) for value in values or []) if run]


def excerpt(tokens, limit=12):
    text = " ".join(tokens)
    return text if len(tokens) <= limit else " ".join(tokens[:limit]) + " …"


def sequence_differences(kind, a, b, joiner=" "):
    differences = []
    matcher = difflib.SequenceMatcher(a=a, b=b, autojunk=False)
    for tag, a0, a1, b0, b1 in matcher.get_opcodes():
        if tag == "equal":
            continue
        context = a[max(0, a0 - 4):a0]
        before = f"after “{excerpt(context)}”: " if context else ""
        left = joiner.join(a[a0:a1]) if joiner else excerpt(a[a0:a1])
        right = joiner.join(b[b0:b1]) if joiner else excerpt(b[b0:b1])
        if tag == "replace":
            differences.append(f"{kind}: {before}A “{shorten(left)}” | B “{shorten(right)}”")
        elif tag == "delete":
            differences.append(f"{kind}: {before}only A “{shorten(left)}”")
        else:
            differences.append(f"{kind}: {before}only B “{shorten(right)}”")
    return differences


def shorten(text, limit=160):
    return text if len(text) <= limit else text[: limit - 1] + "…"


def multiset_differences(kind, a, b):
    a, b = collections.Counter(a), collections.Counter(b)
    differences = []
    for value in sorted((a - b).elements()):
        differences.append(f"{kind}: only A “{shorten(value)}”")
    for value in sorted((b - a).elements()):
        differences.append(f"{kind}: only B “{shorten(value)}”")
    return differences


def compare(a, b):
    """Differences between two captures of the same input, style, and look."""
    differences = []
    differences += sequence_differences("text", words(a.get("visible_text")), words(b.get("visible_text")))
    links_a = [f"{normalize(link.get('text'))} → {link.get('url')}" for link in a.get("links", [])]
    links_b = [f"{normalize(link.get('text'))} → {link.get('url')}" for link in b.get("links", [])]
    differences += multiset_differences("link", links_a, links_b)
    differences += sequence_differences("code", runs(a.get("code_runs")), runs(b.get("code_runs")), joiner=" ⏎ ")
    differences += multiset_differences("bold", runs(a.get("bold_runs")), runs(b.get("bold_runs")))
    differences += multiset_differences("italic", runs(a.get("italic_runs")), runs(b.get("italic_runs")))
    kinds_a = collections.Counter(item.get("kind") for item in a.get("attachments", {}).get("items", []))
    kinds_b = collections.Counter(item.get("kind") for item in b.get("attachments", {}).get("items", []))
    if kinds_a != kinds_b:
        differences.append(f"attachments: A {dict(kinds_a)} | B {dict(kinds_b)}")
    return differences


def captures(directory):
    return {
        path.name: path
        for path in sorted(directory.glob("*.json"))
        if path.name != "index.json" and path.name.count(".") >= 3
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("dump_a", type=Path)
    parser.add_argument("dump_b", type=Path)
    parser.add_argument("report_dir", type=Path)
    parser.add_argument("--label-a", default=None)
    parser.add_argument("--label-b", default=None)
    args = parser.parse_args()

    label_a = args.label_a or args.dump_a.name
    label_b = args.label_b or args.dump_b.name
    a_files, b_files = captures(args.dump_a), captures(args.dump_b)
    if not a_files and not b_files:
        sys.exit("no captures in either directory")
    args.report_dir.mkdir(parents=True, exist_ok=True)

    rows = []
    for name in sorted(set(a_files) | set(b_files)):
        stem = name[: -len(".json")]
        if name not in a_files or name not in b_files:
            rows.append((stem, None, None, [f"missing in {'A' if name not in a_files else 'B'}"]))
            continue
        a = json.loads(a_files[name].read_text())
        b = json.loads(b_files[name].read_text())
        differences = compare(a, b)
        height_a, height_b = a.get("height_pt") or 0, b.get("height_pt") or 0
        if height_a and height_b and abs(height_b / height_a - 1) > 0.05:
            differences.append(f"height: A {height_a:.0f} pt | B {height_b:.0f} pt ({height_b / height_a:.2f}×)")
        rows.append((stem, a, b, differences))

    # Stdout: one line a capture, then the differences of those that differ.
    for stem, _, _, differences in rows:
        print(f"{'same' if not differences else f'{len(differences)} differences':>16}  {stem}")
    for stem, _, _, differences in rows:
        if differences:
            print(f"\n{stem}")
            for difference in differences[:40]:
                print(f"  {difference}")
            if len(differences) > 40:
                print(f"  … {len(differences) - 40} more in the report")

    def image(directory, stem):
        path = directory / f"{stem}.png"
        if not path.exists():
            return "<div class=missing>no image</div>"
        source = os.path.relpath(path.resolve(), args.report_dir.resolve())
        return f'<a href="{html.escape(source)}"><img loading=lazy src="{html.escape(source)}" alt=""></a>'

    sections = []
    for stem, _, _, differences in rows:
        listed = "".join(f"<li>{html.escape(difference)}</li>" for difference in differences)
        verdict = "same text, links, code and emphasis" if not differences else f"{len(differences)} differences"
        sections.append(
            f'<section id="{html.escape(stem)}"><h2>{html.escape(stem)} <small>{verdict}</small></h2>'
            f'<div class=pair><figure>{image(args.dump_a, stem)}<figcaption>A · {html.escape(label_a)}</figcaption></figure>'
            f'<figure>{image(args.dump_b, stem)}<figcaption>B · {html.escape(label_b)}</figcaption></figure></div>'
            f"<ul>{listed}</ul></section>"
        )
    index = "".join(
        f'<tr><td><a href="#{html.escape(stem)}">{html.escape(stem)}</a></td><td>{len(differences) or "—"}</td></tr>'
        for stem, _, _, differences in rows
    )
    report = f"""<!doctype html>
<html lang=en><head><meta charset=utf-8><title>Markdown conformance</title>
<meta name=viewport content="width=device-width, initial-scale=1">
<style>
:root {{ color-scheme: light dark; --ink: #1d1b20; --muted: #6b6570; --rule: #d9d4cc; --paper: #fdfcf9; }}
@media (prefers-color-scheme: dark) {{ :root {{ --ink: #ece8e1; --muted: #a39da8; --rule: #3a3640; --paper: #19191d; }} }}
body {{ margin: 0 auto; max-width: 1600px; padding: 24px 16px; background: var(--paper); color: var(--ink);
       font: 15px/1.45 -apple-system, "SF Pro Text", system-ui, sans-serif; }}
h1 {{ font-size: 22px; margin: 0 0 4px; }} h2 {{ font-size: 16px; margin: 32px 0 8px; }}
h2 small, figcaption, .meta {{ color: var(--muted); font-weight: normal; font-size: 13px; }}
table {{ border-collapse: collapse; margin: 16px 0; }} td {{ padding: 2px 16px 2px 0; border-bottom: 1px solid var(--rule); }}
.pair {{ display: grid; grid-template-columns: 1fr 1fr; gap: 12px; align-items: start; }}
figure {{ margin: 0; }} img {{ width: 100%; height: auto; border: 1px solid var(--rule); }}
ul {{ font: 13px/1.5 ui-monospace, "SF Mono", monospace; padding-left: 18px; }}
section {{ border-top: 1px solid var(--rule); }}
@media (max-width: 700px) {{ .pair {{ grid-template-columns: 1fr; }} }}
</style></head><body>
<h1>Markdown conformance</h1>
<p class=meta>A: {html.escape(str(args.dump_a))}<br>B: {html.escape(str(args.dump_b))}</p>
<table><tr><td><b>capture</b></td><td><b>differences</b></td></tr>{index}</table>
{''.join(sections)}
</body></html>
"""
    (args.report_dir / "report.html").write_text(report)
    print(f"\nreport: {args.report_dir / 'report.html'}")


if __name__ == "__main__":
    main()
