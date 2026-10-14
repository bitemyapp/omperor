"""Submitting jobs on cron schedules."""

import datetime
from dataclasses import dataclass, field

from .cron import Cron


@dataclass
class Schedule:
    name: str
    cron: Cron
    handler: str
    args: dict = field(default_factory=dict)
    next_run: datetime.datetime | None = None


class Scheduler:
    """Each tick submits the jobs whose schedule came due since the last tick.
    A schedule that was missed several times (the scheduler was down) fires
    once, not once per missed slot."""

    def __init__(self, queue):
        self.queue = queue
        self.schedules: dict[str, Schedule] = {}

    def add(self, name: str, expression: str, handler: str, args: dict | None = None) -> Schedule:
        schedule = Schedule(name, Cron(expression), handler, dict(args or {}))
        self.schedules[name] = schedule
        return schedule

    def tick(self) -> list:
        now = datetime.datetime.fromtimestamp(self.queue.clock.now())
        submitted = []
        for schedule in self.schedules.values():
            if schedule.next_run is None:
                schedule.next_run = schedule.cron.next_after(now)
                continue
            if schedule.next_run <= now:
                submitted.append(self.queue.submit(schedule.handler, schedule.args))
                schedule.next_run = schedule.cron.next_after(now)
        return submitted
