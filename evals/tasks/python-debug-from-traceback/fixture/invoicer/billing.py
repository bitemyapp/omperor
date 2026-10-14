"""Pricing time entries."""

from dataclasses import dataclass

from .loader import Entry


@dataclass(frozen=True)
class InvoiceLine:
    entry: Entry
    rate_cents: int
    amount_cents: int


@dataclass(frozen=True)
class Invoice:
    client: str
    month: str
    lines: list[InvoiceLine]

    @property
    def total_cents(self) -> int:
        return sum(line.amount_cents for line in self.lines)


def line_amount(minutes: int, rate_cents: int) -> int:
    """Price of ``minutes`` at an hourly rate, rounded to the nearest cent."""
    return (minutes * rate_cents + 30) // 60


def build_invoice(entries, rates, client: str, month: str) -> Invoice:
    """The client's entries dated in ``month`` (``YYYY-MM``), priced by project."""
    lines = []
    for entry in entries:
        if entry.client != client or entry.day.strftime("%Y-%m") != month:
            continue
        try:
            rate = rates[(entry.client, entry.project)]
        except KeyError:
            raise ValueError(f"no rate for {entry.client} / {entry.project}") from None
        lines.append(InvoiceLine(entry, rate, line_amount(entry.minutes, rate)))
    if not lines:
        raise ValueError(f"no entries for {client} in {month}")
    return Invoice(client, month, lines)
