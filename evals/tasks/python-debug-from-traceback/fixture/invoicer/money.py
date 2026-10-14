"""Formatting integer cents."""


def format_money(cents: int) -> str:
    """``143167`` -> ``$1,431.67``; negative amounts get a leading minus."""
    sign = "-" if cents < 0 else ""
    dollars, remainder = divmod(abs(cents), 100)
    return f"{sign}${dollars:,}.{remainder:02d}"
