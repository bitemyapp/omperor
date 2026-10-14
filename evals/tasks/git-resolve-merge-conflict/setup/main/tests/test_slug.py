import unittest

from slugkit import slugify


class SlugifyTest(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_collapses_separators(self):
        self.assertEqual(slugify("  a -- b  "), "a-b")
        self.assertEqual(slugify("Release 2.0: what's new?"), "release-2-0-what-s-new")

    def test_transliterates_accents(self):
        self.assertEqual(slugify("Crème Brûlée, Again!"), "creme-brulee-again")
        self.assertEqual(slugify("Ça va très bien"), "ca-va-tres-bien")


if __name__ == "__main__":
    unittest.main()
