"""Turning titles into URL slugs."""

import re

_SEPARATORS = re.compile(r"[^a-z0-9]+")


def slugify(title: str) -> str:
    """Lowercase ``title`` and join its words with hyphens.

    >>> slugify("Hello, World!")
    'hello-world'
    """
    slug = _SEPARATORS.sub("-", title.lower())
    return slug.strip("-")
