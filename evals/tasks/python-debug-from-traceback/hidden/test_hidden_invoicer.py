"""Hidden grader tests: root-cause fix for H:MM durations, no silent zeros."""

import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from invoicer.billing import build_invoice, line_amount
from invoicer.loader import load_entries, load_rates, parse_minutes

WORKSPACE = Path(os.environ["INVOICER_WORKSPACE"])
SAMPLES = WORKSPACE / "samples"
HEADER = "date,client,project,duration,description\n"


def cli(*argv, cwd=None):
    return subprocess.run([sys.executable, "-m", "invoicer", *argv], cwd=cwd or WORKSPACE,
                          env=dict(os.environ, PYTHONPATH=str(WORKSPACE)), capture_output=True, text=True, timeout=30)


class HiddenParseTest(unittest.TestCase):
    def test_clock_durations(self):
        for text, minutes in {"1:30": 90, "0:45": 45, "2:15": 135, "3:05": 185, "10:00": 600, " 1:30 ": 90,
                              "0:00": 0, "12:59": 779}.items():
            with self.subTest(text=text):
                self.assertEqual(parse_minutes(text), minutes)

    def test_old_formats_still_work(self):
        for text, minutes in {"90": 90, "45m": 45, "1.5h": 90, "2.5h": 150, "1.25h": 75, " 2H ": 120}.items():
            with self.subTest(text=text):
                self.assertEqual(parse_minutes(text), minutes)

    def test_unreadable_durations_are_never_a_number(self):
        for text in ["tbd", "abc", "", "  ", "1:xx", "n/a", "?"]:
            with self.subTest(text=text):
                try:
                    result = parse_minutes(text)
                except ValueError:
                    continue
                self.assertIsNone(result, f"{text!r} parsed as {result!r}; unreadable input must not be billed")


class HiddenInvoiceTest(unittest.TestCase):
    def test_october_invoice_is_priced_correctly(self):
        invoice = build_invoice(load_entries(SAMPLES / "october.csv"), load_rates(SAMPLES / "rates.csv"),
                                "Northwind", "2024-10")
        self.assertEqual([line.entry.minutes for line in invoice.lines], [90, 135, 50, 185, 20, 60, 40])
        self.assertEqual([line.amount_cents for line in invoice.lines],
                         [22500, 33750, 12500, 46250, 3167, 15000, 10000])
        self.assertEqual(invoice.total_cents, 143167)

    def test_line_amount_unchanged(self):
        self.assertEqual(line_amount(90, 15000), 22500)
        self.assertEqual(line_amount(20, 9500), 3167)

    def test_cli_october(self):
        result = cli("render", "samples/october.csv", "--rates", "samples/rates.csv",
                     "--client", "Northwind", "--month", "2024-10")
        self.assertEqual(result.returncode, 0, result.stderr)
        rows = [line for line in result.stdout.splitlines() if line.startswith("2024-10-")]
        self.assertEqual(len(rows), 7, result.stdout)
        self.assertTrue(result.stdout.rstrip().endswith("$1,431.67"), result.stdout)
        self.assertIn("0:20", result.stdout)

    def test_cli_september_unchanged(self):
        result = cli("render", "samples/september.csv", "--rates", "samples/rates.csv",
                     "--client", "Northwind", "--month", "2024-09")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(result.stdout.rstrip().endswith("$1,022.50"), result.stdout)

    def test_cli_names_the_bad_line_and_value(self):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "rates.csv").write_text((SAMPLES / "rates.csv").read_text())
            Path(directory, "export.csv").write_text(
                HEADER
                + "2024-10-01,Northwind,Website,1:30,Kickoff\n"
                + "2024-10-02,Northwind,Website,0:45,Notes\n"
                + "2024-10-03,Northwind,Website,tbd,Ask Sam how long this took\n"
                + "2024-10-05,Northwind,Website,2:00,Build\n"
            )
            result = cli("render", "export.csv", "--rates", "rates.csv", "--client", "Northwind",
                         "--month", "2024-10", cwd=directory)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertNotIn("Traceback", result.stderr)
        self.assertNotIn("$", result.stdout, "an invoice was printed despite an unreadable duration")
        self.assertIn("tbd", result.stderr)
        self.assertRegex(result.stderr, r"(?<!\d)4(?!\d)", "the error must name line 4 of the CSV file")


if __name__ == "__main__":
    unittest.main()
