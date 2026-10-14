"""Parsing human-written durations such as ``1h30m`` or ``2 days 4h``."""

import re
from fractions import Fraction

from .errors import DurationError

# Units in the order they must appear, with their accepted spellings.
_UNITS = (
    (7 * 24 * 3600, ("w", "wk", "wks", "week", "weeks")),
    (24 * 3600, ("d", "day", "days")),
    (3600, ("h", "hr", "hrs", "hour", "hours")),
    (60, ("m", "min", "mins", "minute", "minutes")),
    (1, ("s", "sec", "secs", "second", "seconds")),
)
_SPELLINGS = {spelling: (rank, size) for rank, (size, spellings) in enumerate(_UNITS) for spelling in spellings}
_COMPONENT = re.compile(r"\s*([0-9]+(?:\.[0-9]+)?)\s*([A-Za-z]+)")
_BARE_SECONDS = re.compile(r"[0-9]+")


def parse_duration(text: str) -> int:
    """Return the number of seconds ``text`` describes (see issue #41)."""
    if not isinstance(text, str):
        raise TypeError(f"duration must be a str, not {type(text).__name__}")

    def error(reason):
        return DurationError(f"invalid duration {text!r}: {reason}")

    body = text.strip()
    negative = body.startswith("-")
    if negative:
        body = body[1:]
    if not body.strip():
        raise error("empty")
    sign = -1 if negative else 1
    if _BARE_SECONDS.fullmatch(body):
        return sign * int(body)
    total = Fraction(0)
    position, last_rank = 0, -1
    while position < len(body):
        match = _COMPONENT.match(body, position)
        if match is None:
            raise error(f"unexpected {body[position:].strip()!r}")
        number, unit = match.groups()
        if unit.lower() not in _SPELLINGS:
            raise error(f"unknown unit {unit!r}")
        rank, size = _SPELLINGS[unit.lower()]
        if rank <= last_rank:
            raise error("units must be in descending order (w, d, h, m, s), each at most once")
        last_rank = rank
        total += Fraction(number) * size
        position = match.end()
    if total.denominator != 1:
        raise error("not a whole number of seconds")
    return sign * int(total)
