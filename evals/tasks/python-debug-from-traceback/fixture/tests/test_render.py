import unittest
from pathlib import Path

from invoicer.billing import build_invoice
from invoicer.loader import load_entries, load_rates
from invoicer.money import format_money
from invoicer.render import render_invoice

SAMPLES = Path(__file__).resolve().parent.parent / "samples"


class RenderTest(unittest.TestCase):
    def test_money(self):
        self.assertEqual(format_money(143167), "$1,431.67")
        self.assertEqual(format_money(5), "$0.05")

    def test_september_invoice(self):
        invoice = build_invoice(load_entries(SAMPLES / "september.csv"), load_rates(SAMPLES / "rates.csv"),
                                "Northwind", "2024-09")
        text = render_invoice(invoice)
        self.assertIn("Invoice for Northwind, 2024-09", text)
        self.assertTrue(text.rstrip().endswith("$1,022.50"), text)
