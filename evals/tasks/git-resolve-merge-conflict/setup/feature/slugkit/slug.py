"""Turning titles into URL slugs."""

import re

_SEPARATORS = re.compile(r"[^a-z0-9]+")


def slugify(title: str, max_length: int | None = None) -> str:
    """Lowercase ``title`` and join its words with hyphens.

    With ``max_length``, a longer slug is cut back to the last hyphen that
    keeps it within the limit, so words are not split; a first word that is
    longer than the limit on its own is truncated.

    >>> slugify("Hello, World!")
    'hello-world'
    >>> slugify("The quick brown fox", max_length=12)
    'the-quick'
    """
    slug = _SEPARATORS.sub("-", title.lower()).strip("-")
    if max_length is not None and len(slug) > max_length:
        cut = slug.rfind("-", 0, max_length + 1)
        slug = slug[:cut] if cut > 0 else slug[:max_length]
    return slug
