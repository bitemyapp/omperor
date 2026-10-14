import contextlib
import io
import unittest

from timespan.cli import main


def run(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        status = main(list(argv))
    return status, out.getvalue(), err.getvalue()


class FormatCommandTest(unittest.TestCase):
    def test_formats_seconds(self):
        self.assertEqual(run("format", "5400"), (0, "1h30m\n", ""))

    def test_rejects_non_numbers(self):
        status, out, err = run("format", "1.5")
        self.assertEqual((status, out), (2, ""))
        self.assertTrue(err.startswith("timespan: error:"))


if __name__ == "__main__":
    unittest.main()
