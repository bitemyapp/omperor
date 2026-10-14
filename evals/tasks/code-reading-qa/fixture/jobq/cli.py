"""``jobq`` command line."""

import argparse
import dataclasses
import logging
import os
import sys

from .durations import parse_duration
from .job import JobStatus
from .queue import Queue
from .registry import default_registry
from .settings import Settings
from .store import SqliteStore
from .worker import Worker


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="jobq")
    parser.add_argument("--db", help="database file (default: JOBQ_DB or jobq.db)")
    commands = parser.add_subparsers(dest="command", required=True)

    submit = commands.add_parser("submit", help="queue a job")
    submit.add_argument("handler")
    submit.add_argument("--arg", action="append", default=[], metavar="KEY=VALUE")
    submit.add_argument("--retries", type=int, default=None, help="retry budget (default: from settings)")
    submit.add_argument("--priority", type=int, default=0)
    submit.add_argument("--delay", type=parse_duration, default=0.0, help="e.g. 30s, 5m")

    worker = commands.add_parser("worker", help="run jobs")
    worker.add_argument("--handlers", default="tasks", help="module that registers handlers (default: tasks)")
    worker.add_argument("--lease", type=parse_duration, default=None, help="lease per job (default: JOBQ_LEASE or 45s)")
    worker.add_argument("--max-jobs", type=int, default=None)
    worker.add_argument("--stats", action="store_true")

    status = commands.add_parser("status", help="show one job")
    status.add_argument("job_id")

    listing = commands.add_parser("list", help="list jobs")
    listing.add_argument("--status", choices=[s.value for s in JobStatus])

    cancel = commands.add_parser("cancel", help="cancel a queued job")
    cancel.add_argument("job_id")
    return parser


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    settings = Settings.from_env(os.environ)
    if args.db:
        settings = dataclasses.replace(settings, db_path=args.db)
    queue = Queue(SqliteStore(settings.db_path), settings)

    if args.command == "submit":
        job_args = dict(item.split("=", 1) for item in args.arg)
        job = queue.submit(args.handler, job_args, max_retries=args.retries, priority=args.priority,
                           delay=args.delay)
        print(job.id)
    elif args.command == "worker":
        logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
        if args.lease is not None:
            settings = dataclasses.replace(settings, lease_seconds=args.lease)
        default_registry.load(args.handlers)
        worker = Worker(queue, default_registry, settings)
        try:
            worker.run(args.max_jobs)
        except KeyboardInterrupt:
            pass
        if args.stats:
            print(queue.metrics.snapshot())
    elif args.command == "status":
        job = queue.store.get(args.job_id)
        if job is None:
            print(f"jobq: no job {args.job_id}", file=sys.stderr)
            return 1
        print(f"{job.id} {job.handler} {job.status.value} attempts={job.attempts} last_error={job.last_error}")
    elif args.command == "list":
        for job in queue.store.all(JobStatus(args.status) if args.status else None):
            print(f"{job.id} {job.handler:<20} {job.status.value:<9} attempts={job.attempts}")
    elif args.command == "cancel":
        if not queue.cancel(args.job_id):
            print(f"jobq: cannot cancel {args.job_id}", file=sys.stderr)
            return 1
    return 0
