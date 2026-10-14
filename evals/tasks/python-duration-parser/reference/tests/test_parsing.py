import unittest

from timespan import DurationError, format_duration, parse_duration


class ParseDurationTest(unittest.TestCase):
    def test_examples(self):
        for text, seconds in [("1h30m", 5400), ("90s", 90), ("90", 90), ("2d 4h", 187200),
                              ("1.5h", 5400), ("2 hours", 7200), ("-1h30m", -5400), ("0.1h", 360)]:
            with self.subTest(text=text):
                self.assertEqual(parse_duration(text), seconds)

    def test_rejects(self):
        for text in ["", "30m1h", "1h1h", "1h30", "0.5s", "1_000s", "１h", "1e3s", ".5h"]:
            with self.subTest(text=text), self.assertRaises(DurationError):
                parse_duration(text)

    def test_round_trip(self):
        for seconds in range(-5000, 5000, 13):
            self.assertEqual(parse_duration(format_duration(seconds)), seconds)


if __name__ == "__main__":
    unittest.main()
