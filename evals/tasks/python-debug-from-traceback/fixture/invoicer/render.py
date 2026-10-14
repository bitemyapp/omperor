"""Plain-text invoice layout."""

from .billing import Invoice
from .money import format_money


def format_hours(minutes: int) -> str:
    hours, rest = divmod(minutes, 60)
    return f"{hours}:{rest:02d}"


def render_invoice(invoice: Invoice) -> str:
    rows = [f"Invoice for {invoice.client}, {invoice.month}", ""]
    for line in invoice.lines:
        entry = line.entry
        rows.append(
            f"{entry.day.isoformat()}  {entry.project:<10} {entry.description[:34]:<34} "
            f"{format_hours(entry.minutes):>6} {format_money(line.amount_cents):>11}"
        )
    rows += ["", f"{'Total:':>63} {format_money(invoice.total_cents):>11}"]
    return "\n".join(rows)
