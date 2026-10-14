#!/usr/bin/env bash
# The reference solution `omperor eval validate` runs in a fresh workspace:
# it resolves the conflicted merge the way a correct agent would, so the
# grader can be checked against a solved workspace.
set -euo pipefail
cat > slugkit/slug.py <<'PY'
"""Turning titles into URL slugs."""

import re
import unicodedata

_SEPARATORS = re.compile(r"[^a-z0-9]+")


def _ascii(text: str) -> str:
    """Drop accents and other marks: ``Crème Brûlée`` -> ``Creme Brulee``."""
    decomposed = unicodedata.normalize("NFKD", text)
    return decomposed.encode("ascii", "ignore").decode("ascii")


def slugify(title: str, max_length: int | None = None) -> str:
    """Transliterate ``title`` to ASCII, lowercase it, and join its words
    with hyphens.

    With ``max_length``, a longer slug is cut back to the last hyphen that
    keeps it within the limit, so words are not split; a first word that is
    longer than the limit on its own is truncated.

    >>> slugify("Crème Brûlée, Again!")
    'creme-brulee-again'
    >>> slugify("The quick brown fox", max_length=12)
    'the-quick'
    """
    slug = _SEPARATORS.sub("-", _ascii(title).lower()).strip("-")
    if max_length is not None and len(slug) > max_length:
        cut = slug.rfind("-", 0, max_length + 1)
        slug = slug[:cut] if cut > 0 else slug[:max_length]
    return slug
PY
python3 - <<'PY'
import re
from pathlib import Path

def keep_both(path):
    text = Path(path).read_text()
    pattern = re.compile(r"<<<<<<< [^\n]*\n(.*?)(?:\|\|\|\|\|\|\| [^\n]*\n.*?)?=======\n(.*?)>>>>>>> [^\n]*\n", re.S)
    Path(path).write_text(pattern.sub(lambda m: m.group(1) + m.group(2), text))

keep_both("CHANGELOG.md")
keep_both("tests/test_slug.py")
PY
python3 -m unittest discover -s tests > /dev/null 2>&1
git add CHANGELOG.md slugkit/slug.py tests/test_slug.py
git commit --quiet --no-edit
