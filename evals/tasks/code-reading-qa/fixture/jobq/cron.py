"""Five-field cron expressions: minute hour day-of-month month day-of-week."""

import datetime

_RANGES = ((0, 59), (0, 23), (1, 31), (1, 12), (0, 6))


def _field(text: str, low: int, high: int) -> frozenset:
    values = set()
    for part in text.split(","):
        step = 1
        if "/" in part:
            part, step_text = part.split("/", 1)
            step = int(step_text)
            if step < 1:
                raise ValueError(f"bad step in {text!r}")
        if part == "*":
            start, end = low, high
        elif "-" in part:
            start, end = (int(n) for n in part.split("-", 1))
        else:
            start = end = int(part)
        if not (low <= start <= end <= high):
            raise ValueError(f"{text!r} is outside {low}-{high}")
        values.update(range(start, end + 1, step))
    return frozenset(values)


class Cron:
    def __init__(self, expression: str):
        fields = expression.split()
        if len(fields) != 5:
            raise ValueError(f"cron needs five fields: {expression!r}")
        self.expression = expression
        self.minutes, self.hours, self.days, self.months, self.weekdays = (
            _field(text, low, high) for text, (low, high) in zip(fields, _RANGES))

    def matches(self, moment: datetime.datetime) -> bool:
        weekday = (moment.weekday() + 1) % 7  # cron counts Sunday as 0
        return (moment.minute in self.minutes and moment.hour in self.hours and moment.day in self.days
                and moment.month in self.months and weekday in self.weekdays)

    def next_after(self, moment: datetime.datetime) -> datetime.datetime:
        """The first matching minute strictly after ``moment``."""
        candidate = moment.replace(second=0, microsecond=0) + datetime.timedelta(minutes=1)
        for _ in range(366 * 24 * 60):
            if self.matches(candidate):
                return candidate
            candidate += datetime.timedelta(minutes=1)
        raise ValueError(f"{self.expression!r} never fires")
