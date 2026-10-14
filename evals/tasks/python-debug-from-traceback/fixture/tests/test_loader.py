import tempfile
import unittest
from pathlib import Path

from invoicer.loader import load_entries, load_rates, parse_minutes

SAMPLES = Path(__file__).resolve().parent.parent / "samples"


class ParseMinutesTest(unittest.TestCase):
    def test_tracker_formats(self):
        self.assertEqual(parse_minutes("90"), 90)
        self.assertEqual(parse_minutes("45m"), 45)
        self.assertEqual(parse_minutes("1.5h"), 90)
        self.assertEqual(parse_minutes(" 2H "), 120)


class LoadTest(unittest.TestCase):
    def test_september_export(self):
        entries = load_entries(SAMPLES / "september.csv")
        self.assertEqual(len(entries), 6)
        self.assertEqual(entries[1].minutes, 150)
        self.assertEqual(entries[2].client, "Contoso")

    def test_rates(self):
        rates = load_rates(SAMPLES / "rates.csv")
        self.assertEqual(rates[("Northwind", "Support")], 9500)

    def test_strips_cells(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "x.csv"
            path.write_text("date,client,project,duration,description\n2024-09-01, Northwind ,Website, 30m ,x\n")
            [entry] = load_entries(path)
            self.assertEqual((entry.client, entry.minutes), ("Northwind", 30))
