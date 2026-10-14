"""Formatting a number of seconds as a compact duration such as ``1h30m``."""

UNITS = (("w", 7 * 24 * 3600), ("d", 24 * 3600), ("h", 3600), ("m", 60), ("s", 1))


def format_duration(seconds: int) -> str:
    """Format ``seconds`` with the largest units first, skipping zero units.

    >>> format_duration(5400)
    '1h30m'
    >>> format_duration(-90)
    '-1m30s'
    >>> format_duration(0)
    '0s'
    """
    if isinstance(seconds, bool) or not isinstance(seconds, int):
        raise TypeError(f"seconds must be an int, not {type(seconds).__name__}")
    if seconds == 0:
        return "0s"
    sign = "-" if seconds < 0 else ""
    remaining = abs(seconds)
    parts = []
    for suffix, size in UNITS:
        count, remaining = divmod(remaining, size)
        if count:
            parts.append(f"{count}{suffix}")
    return sign + "".join(parts)
