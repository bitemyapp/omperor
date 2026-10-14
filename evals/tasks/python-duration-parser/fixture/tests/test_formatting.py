import unittest

from timespan import format_duration


class FormatDurationTest(unittest.TestCase):
    def test_largest_units_first(self):
        self.assertEqual(format_duration(5400), "1h30m")
        self.assertEqual(format_duration(788645), "1w2d3h4m5s")

    def test_zero_and_negative(self):
        self.assertEqual(format_duration(0), "0s")
        self.assertEqual(format_duration(-90), "-1m30s")

    def test_rejects_non_integers(self):
        with self.assertRaises(TypeError):
            format_duration(1.5)
        with self.assertRaises(TypeError):
            format_duration(True)


if __name__ == "__main__":
    unittest.main()
