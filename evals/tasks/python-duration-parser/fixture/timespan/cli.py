"""Command line: ``python3 -m timespan COMMAND ...``."""

import argparse
import sys

from .formatting import format_duration

PROG = "timespan"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog=PROG, description="Convert between durations and seconds.")
    commands = parser.add_subparsers(dest="command", required=True)
    format_command = commands.add_parser("format", help="format seconds as a duration, e.g. 5400 -> 1h30m")
    format_command.add_argument("seconds", help="a whole number of seconds")
    # TODO(#41): a `parse` command, the inverse of `format`.
    return parser


def fail(message: str) -> int:
    print(f"{PROG}: error: {message}", file=sys.stderr)
    return 2


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "format":
        text = args.seconds.strip()
        digits = text[1:] if text.startswith("-") else text
        if not (digits.isascii() and digits.isdigit()):
            return fail(f"not a whole number of seconds: {args.seconds!r}")
        print(format_duration(int(text)))
        return 0
    return fail(f"unknown command {args.command!r}")
