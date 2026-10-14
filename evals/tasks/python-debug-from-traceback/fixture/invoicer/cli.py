"""``python3 -m invoicer render ENTRIES --rates RATES --client NAME --month YYYY-MM``."""

import argparse
import sys

from .billing import build_invoice
from .loader import load_entries, load_rates
from .render import render_invoice


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="invoicer")
    commands = parser.add_subparsers(dest="command", required=True)
    render = commands.add_parser("render", help="print one client's invoice for a month")
    render.add_argument("entries", help="time export (CSV)")
    render.add_argument("--rates", required=True, help="rate card (CSV)")
    render.add_argument("--client", required=True)
    render.add_argument("--month", required=True, help="YYYY-MM")
    return parser


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        entries = load_entries(args.entries)
        rates = load_rates(args.rates)
        invoice = build_invoice(entries, rates, args.client, args.month)
    except (OSError, ValueError) as error:
        print(f"invoicer: error: {error}", file=sys.stderr)
        return 1
    print(render_invoice(invoice))
    return 0
