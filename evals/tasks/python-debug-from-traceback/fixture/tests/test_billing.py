import datetime
import unittest

from invoicer.billing import build_invoice, line_amount
from invoicer.loader import Entry


def entry(day, client="Northwind", project="Website", minutes=60):
    return Entry(datetime.date.fromisoformat(day), client, project, minutes, "work")


class BillingTest(unittest.TestCase):
    def test_line_amount_rounds_to_cents(self):
        self.assertEqual(line_amount(90, 15000), 22500)
        self.assertEqual(line_amount(20, 9500), 3167)

    def test_build_invoice_filters_client_and_month(self):
        rates = {("Northwind", "Website"): 15000}
        invoice = build_invoice(
            [entry("2024-09-30"), entry("2024-10-01"), entry("2024-10-02", client="Contoso")],
            rates, "Northwind", "2024-10",
        )
        self.assertEqual([line.entry.day.day for line in invoice.lines], [1])
        self.assertEqual(invoice.total_cents, 15000)

    def test_missing_rate_is_an_error(self):
        with self.assertRaises(ValueError):
            build_invoice([entry("2024-10-01", project="Secret")], {}, "Northwind", "2024-10")
