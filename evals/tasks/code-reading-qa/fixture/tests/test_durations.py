import unittest

from jobq.durations import format_duration, parse_duration


class DurationTest(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(parse_duration("45"), 45)
        self.assertEqual(parse_duration("2m"), 120)
        self.assertEqual(parse_duration("500ms"), 0.5)
        with self.assertRaises(ValueError):
            parse_duration("soon")

    def test_format(self):
        self.assertEqual(format_duration(120), "2m")
        self.assertEqual(format_duration(7200), "2h")
        self.assertEqual(format_duration(1.5), "1.5s")
