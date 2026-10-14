import unittest

from slugkit import slugify


class SlugifyTest(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_collapses_separators(self):
        self.assertEqual(slugify("  a -- b  "), "a-b")
        self.assertEqual(slugify("Release 2.0: what's new?"), "release-2-0-what-s-new")

    def test_max_length_cuts_at_word_boundary(self):
        self.assertEqual(slugify("The quick brown fox", max_length=12), "the-quick")
        self.assertEqual(slugify("The quick brown fox", max_length=9), "the-quick")
        self.assertEqual(slugify("Short", max_length=50), "short")

    def test_max_length_truncates_a_long_first_word(self):
        self.assertEqual(slugify("Supercalifragilistic expialidocious", max_length=10), "supercalif")


if __name__ == "__main__":
    unittest.main()
