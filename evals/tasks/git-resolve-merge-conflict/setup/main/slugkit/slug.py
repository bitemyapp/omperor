"""Turning titles into URL slugs."""

import re
import unicodedata

_SEPARATORS = re.compile(r"[^a-z0-9]+")


def _ascii(text: str) -> str:
    """Drop accents and other marks: ``Crème Brûlée`` -> ``Creme Brulee``."""
    decomposed = unicodedata.normalize("NFKD", text)
    return decomposed.encode("ascii", "ignore").decode("ascii")


def slugify(title: str) -> str:
    """Transliterate ``title`` to ASCII, lowercase it, and join its words
    with hyphens.

    >>> slugify("Crème Brûlée, Again!")
    'creme-brulee-again'
    """
    slug = _SEPARATORS.sub("-", _ascii(title).lower())
    return slug.strip("-")
