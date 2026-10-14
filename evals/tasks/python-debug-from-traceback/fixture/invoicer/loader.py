"""Reading the time-tracking export and the rate card."""

import csv
import datetime
from dataclasses import dataclass


@dataclass(frozen=True)
class Entry:
    day: datetime.date
    client: str
    project: str
    minutes: int
    description: str


def parse_minutes(text: str) -> int:
    """Durations as the tracker exports them: ``90`` (minutes), ``45m``, ``1.5h``."""
    text = text.strip().lower()
    if text.endswith("h"):
        return round(float(text[:-1]) * 60)
    if text.endswith("m"):
        return int(text[:-1])
    if text.isdigit():
        return int(text)


def load_entries(path) -> list[Entry]:
    """Every row of a time export, in file order."""
    with open(path, newline="", encoding="utf-8") as handle:
        return [
            Entry(
                day=datetime.date.fromisoformat(row["date"].strip()),
                client=row["client"].strip(),
                project=row["project"].strip(),
                minutes=parse_minutes(row["duration"]),
                description=row["description"].strip(),
            )
            for row in csv.DictReader(handle)
        ]


def load_rates(path) -> dict[tuple[str, str], int]:
    """``(client, project) -> hourly rate in cents``."""
    with open(path, newline="", encoding="utf-8") as handle:
        return {
            (row["client"].strip(), row["project"].strip()): int(row["rate_cents_per_hour"])
            for row in csv.DictReader(handle)
        }
