"""A store in SQLite, shared by every process that opens the same file."""

import json
import sqlite3

from ..job import Job, JobStatus

SCHEMA = """
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    handler TEXT NOT NULL,
    args TEXT NOT NULL,
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL,
    max_retries INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    run_at REAL NOT NULL,
    lease_expires_at REAL,
    last_error TEXT,
    seq INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS jobs_ready ON jobs (status, run_at);
"""

COLUMNS = ("id", "handler", "args", "status", "attempts", "max_retries", "priority", "run_at",
           "lease_expires_at", "last_error", "seq")


class SqliteStore:
    def __init__(self, path: str):
        self._db = sqlite3.connect(path, isolation_level=None, timeout=30)
        self._db.executescript(SCHEMA)

    def close(self):
        self._db.close()

    def put(self, job):
        row = (job.id, job.handler, json.dumps(job.args, sort_keys=True), job.status.value, job.attempts,
               job.max_retries, job.priority, job.run_at, job.lease_expires_at, job.last_error, job.seq)
        self._db.execute(f"INSERT OR REPLACE INTO jobs ({', '.join(COLUMNS)}) VALUES ({', '.join('?' * len(COLUMNS))})", row)

    def get(self, job_id):
        row = self._db.execute(f"SELECT {', '.join(COLUMNS)} FROM jobs WHERE id = ?", (job_id,)).fetchone()
        return self._job(row) if row else None

    def next_seq(self):
        (seq,) = self._db.execute("SELECT COALESCE(MAX(seq), 0) + 1 FROM jobs").fetchone()
        return seq

    def next_ready(self, now):
        row = self._db.execute(
            f"""SELECT {', '.join(COLUMNS)} FROM jobs
                WHERE (status = 'queued' AND run_at <= ?)
                   OR (status = 'running' AND lease_expires_at <= ?)
                ORDER BY priority DESC, run_at, seq LIMIT 1""",
            (now, now),
        ).fetchone()
        return self._job(row) if row else None

    def all(self, status=None):
        query = f"SELECT {', '.join(COLUMNS)} FROM jobs"
        rows = self._db.execute(query + (" WHERE status = ? ORDER BY seq" if status else " ORDER BY seq"),
                                (status.value,) if status else ())
        return [self._job(row) for row in rows]

    @staticmethod
    def _job(row):
        values = dict(zip(COLUMNS, row))
        values["args"] = json.loads(values["args"])
        values["status"] = JobStatus(values["status"])
        return Job(**values)
