"""Parsing human-written durations such as ``1h30m`` or ``2 days 4h``."""

from .errors import DurationError


def parse_duration(text: str) -> int:
    """Return the number of seconds ``text`` describes (see issue #41)."""
    raise NotImplementedError("parse_duration is not implemented yet (#41)")
