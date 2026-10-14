"""Hidden grader tests, run against the tree of the merge commit (HEAD)."""

import unittest

from slugkit import slugify


class HiddenMergedBehaviourTest(unittest.TestCase):
    def test_main_side_transliteration(self):
        self.assertEqual(slugify("Crème Brûlée, Again!"), "creme-brulee-again")
        self.assertEqual(slugify("Ça va très bien"), "ca-va-tres-bien")
        self.assertEqual(slugify("Ñandú über Café"), "nandu-uber-cafe")

    def test_feature_side_max_length(self):
        self.assertEqual(slugify("The quick brown fox", max_length=12), "the-quick")
        self.assertEqual(slugify("The quick brown fox", max_length=9), "the-quick")
        self.assertEqual(slugify("Supercalifragilistic expialidocious", max_length=10), "supercalif")
        self.assertEqual(slugify("Short", max_length=50), "short")
        self.assertEqual(slugify("The quick brown fox"), "the-quick-brown-fox")

    def test_transliteration_happens_before_the_length_limit(self):
        self.assertEqual(slugify("Crème Brûlée à la mode", max_length=12), "creme-brulee")
        self.assertEqual(slugify("Ça va très bien", max_length=8), "ca-va")
        self.assertEqual(slugify("Élan vital", max_length=4), "elan")

    def test_unchanged_basics(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")
        self.assertEqual(slugify("  a -- b  "), "a-b")


if __name__ == "__main__":
    unittest.main()
