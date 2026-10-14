"""Hidden grader tests for issue #41. Run with the workspace copy on sys.path."""

import os
import subprocess
import sys
import unittest

from timespan import DurationError, format_duration, parse_duration
from timespan.cli import main

WORKSPACE = os.environ["TIMESPAN_WORKSPACE"]

VALID = {
    "90s": 90, "90": 90, "0": 0, "0s": 0, "  45m  ": 2700,
    "1h30m": 5400, "1h 30m": 5400, "2d 4h": 187200, "2d4h": 187200, "1w": 604800,
    "1w2d3h4m5s": 788645, "1w 2d 3h 4m 5s": 788645, "1H30M": 5400, "1h0m": 3600, "0h5m": 300,
    "2 hours": 7200, "1 hour 30 minutes": 5400, "3 days": 259200, "1 day": 86400, "1 week": 604800,
    "2 weeks": 1209600, "2 wks": 1209600, "1 wk": 604800, "10 secs": 10, "1 sec": 1, "1 second": 1,
    "5 mins": 300, "1 min": 60, "1 minute": 60, "2 hrs": 7200, "1 hr": 3600, "3 Days 2 Hours": 266400,
    "1.5h": 5400, "1.5m": 90, "0.5m": 30, "1.25m": 75, "0.1h": 360, "1.15h": 4140, "2.5d": 216000,
    "0.75 hours": 2700, "1.5w": 907200, "0.3m": 18,
    "-1h30m": -5400, "-90": -90, "-0.5m": -30, "-2d 4h": -187200,
    "100000w": 60480000000, "123456789s": 123456789,
}

INVALID = [
    "", "   ", "-", "h", "1x", "1h30", "1 h 30", "30m1h", "1h1h", "1h 1hour", "1m 2h", "1s1m",
    "1.h", ".5h", "1..5h", "1.5.5h", "1e3s", "1E3s", "1_000s", "1,5h", "１h", "٣h", "²h",
    "+5m", "--5m", "5m-", "1h-30m", "5 m s", "1.5", "0.5s", "0.001h", "1.1s", "abc", "1h30m!",
    "5ms", "1hour30", "inf h", "nan s", "infinity", "1 hours 2 hours", "-", "- ", "1h,30m", "0x10s",
]


class HiddenParseTest(unittest.TestCase):
    def test_valid_inputs(self):
        for text, seconds in VALID.items():
            with self.subTest(text=text):
                result = parse_duration(text)
                self.assertEqual(result, seconds)
                self.assertIs(type(result), int)

    def test_invalid_inputs_raise_duration_error(self):
        self.assertTrue(issubclass(DurationError, ValueError))
        for text in INVALID:
            with self.subTest(text=text):
                try:
                    result = parse_duration(text)
                except DurationError:
                    continue
                except Exception as error:  # noqa: BLE001 - the type is the point
                    self.fail(f"{text!r} raised {type(error).__name__}, not DurationError: {error}")
                self.fail(f"{text!r} was accepted as {result!r}")

    def test_error_message_names_the_input(self):
        for text in ["30m1h", "5 parsecs", "0.5s"]:
            with self.subTest(text=text):
                with self.assertRaises(DurationError) as caught:
                    parse_duration(text)
                self.assertIn(text, str(caught.exception))

    def test_round_trips_format_duration(self):
        values = list(range(-100000, 100000, 37)) + [604799, 604800, 788645, 10**12 + 7, -(10**9)]
        for seconds in values:
            self.assertEqual(parse_duration(format_duration(seconds)), seconds, format_duration(seconds))

    def test_format_duration_unchanged(self):
        self.assertEqual(format_duration(5400), "1h30m")
        self.assertEqual(format_duration(-90), "-1m30s")
        self.assertEqual(format_duration(0), "0s")


def cli(*argv):
    return subprocess.run([sys.executable, "-m", "timespan", *argv], cwd=WORKSPACE,
                          capture_output=True, text=True, timeout=30)


class HiddenCliTest(unittest.TestCase):
    def test_parse_prints_seconds(self):
        for argv, expected in [(["parse", "1h30m"], "5400\n"), (["parse", "2 days 4h"], "187200\n"),
                               (["parse", "--", "-1h30m"], "-5400\n"), (["parse", "0.1h"], "360\n")]:
            with self.subTest(argv=argv):
                result = cli(*argv)
                self.assertEqual((result.returncode, result.stdout), (0, expected), result.stderr)

    def test_parse_errors_follow_the_cli_convention(self):
        for text in ["30m1h", "0.5s", "soon"]:
            with self.subTest(text=text):
                result = cli("parse", text)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertTrue(result.stderr.startswith("timespan: error:"), result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_format_still_works(self):
        result = cli("format", "788645")
        self.assertEqual((result.returncode, result.stdout), (0, "1w2d3h4m5s\n"))

    def test_main_is_importable_and_returns_status(self):
        import contextlib
        import io
        with contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(main(["parse", "90"]), 0)
        self.assertEqual(out.getvalue(), "90\n")


if __name__ == "__main__":
    unittest.main()
