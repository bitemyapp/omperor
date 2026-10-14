"""Reading the time-tracking export and the rate card."""

import csv
import datetime
import re
from dataclasses import dataclass


@dataclass(frozen=True)
class Entry:
    day: datetime.date
    client: str
    project: str
    minutes: int
    description: str


_CLOCK = re.compile(r"(\d+):([0-5]\d)")


def parse_minutes(text: str) -> int:
    """Durations as the tracker exports them: ``90`` (minutes), ``45m``, ``1.5h``,
    and, since the October 2024 tracker update, ``1:30`` (hours:minutes).

    Raises ValueError for anything else rather than guessing.
    """
    cleaned = text.strip().lower()
    clock = _CLOCK.fullmatch(cleaned)
    if clock:
        return int(clock.group(1)) * 60 + int(clock.group(2))
    try:
        if cleaned.endswith("h"):
            return round(float(cleaned[:-1]) * 60)
        if cleaned.endswith("m"):
            return int(cleaned[:-1])
        if cleaned.isascii() and cleaned.isdigit():
            return int(cleaned)
    except (ValueError, OverflowError):
        pass
    raise ValueError(f"unreadable duration {text!r}")


def load_entries(path) -> list[Entry]:
    """Every row of a time export, in file order. A row that cannot be read is
    a ValueError naming the file and its line number."""
    entries = []
    with open(path, newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            try:
                entries.append(
                    Entry(
                        day=datetime.date.fromisoformat(row["date"].strip()),
                        client=row["client"].strip(),
                        project=row["project"].strip(),
                        minutes=parse_minutes(row["duration"]),
                        description=row["description"].strip(),
                    )
                )
            except ValueError as error:
                raise ValueError(f"{path}, line {reader.line_num}: {error}") from None
    return entries


def load_rates(path) -> dict[tuple[str, str], int]:
    """``(client, project) -> hourly rate in cents``."""
    with open(path, newline="", encoding="utf-8") as handle:
        return {
            (row["client"].strip(), row["project"].strip()): int(row["rate_cents_per_hour"])
            for row in csv.DictReader(handle)
        }
