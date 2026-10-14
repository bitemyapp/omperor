import tempfile
import unittest
from pathlib import Path

from invoicer.billing import build_invoice
from invoicer.loader import load_entries, load_rates, parse_minutes

SAMPLES = Path(__file__).resolve().parent.parent / "samples"


class OctoberExportTest(unittest.TestCase):
    def test_clock_durations(self):
        self.assertEqual(parse_minutes("1:30"), 90)
        self.assertEqual(parse_minutes("0:45"), 45)
        self.assertEqual(parse_minutes("10:05"), 605)

    def test_unreadable_duration_is_an_error(self):
        for text in ["tbd", "", "1:xx", "infh"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse_minutes(text)

    def test_error_names_the_line(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.csv"
            path.write_text("date,client,project,duration,description\n"
                            "2024-10-01,Northwind,Website,1:00,a\n2024-10-02,Northwind,Website,tbd,b\n")
            with self.assertRaisesRegex(ValueError, r"line 3: .*'tbd'"):
                load_entries(path)

    def test_october_invoice_total(self):
        invoice = build_invoice(load_entries(SAMPLES / "october.csv"), load_rates(SAMPLES / "rates.csv"),
                                "Northwind", "2024-10")
        self.assertEqual(invoice.total_cents, 143167)


if __name__ == "__main__":
    unittest.main()
