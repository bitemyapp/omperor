"""jobq: a small persistent job queue."""

from .errors import PermanentError, TransientError
from .job import Job, JobStatus
from .queue import Queue
from .registry import task
from .settings import Settings
from .worker import Worker

__all__ = ["Job", "JobStatus", "PermanentError", "Queue", "Settings", "TransientError", "Worker", "task"]
__version__ = "0.9.2"
