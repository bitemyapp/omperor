"""Durations in configuration: ``45`` (seconds), ``500ms``, ``30s``, ``2m``, ``1.5h``."""

import re

_UNITS = {"ms": 0.001, "s": 1.0, "m": 60.0, "h": 3600.0}
_PATTERN = re.compile(r"\s*([0-9]+(?:\.[0-9]+)?)\s*(ms|s|m|h)?\s*")


def parse_duration(text: str) -> float:
    """Seconds in ``text``; a bare number means seconds."""
    match = _PATTERN.fullmatch(text)
    if not match:
        raise ValueError(f"invalid duration {text!r}")
    number, unit = match.groups()
    return float(number) * _UNITS[unit or "s"]


def format_duration(seconds: float) -> str:
    if seconds >= 3600 and seconds % 3600 == 0:
        return f"{int(seconds // 3600)}h"
    if seconds >= 60 and seconds % 60 == 0:
        return f"{int(seconds // 60)}m"
    return f"{seconds:g}s"
