"""Errors raised by handlers and by the queue."""


class JobError(Exception):
    """Base class for errors a handler raises on purpose."""


class TransientError(JobError):
    """Worth retrying later: a timeout, a rate limit, a dependency restarting."""


class PermanentError(JobError):
    """Never worth retrying: bad arguments, a missing record."""


class UnknownHandler(PermanentError):
    """No handler is registered under the job's name."""


class LeaseLost(Exception):
    """The worker's lease ran out before it reported the job's outcome."""
