"""Legacy configuration constants.

These predate :class:`jobq.settings.Settings` and are only used by
:func:`jobq.compat.load_legacy_ini` to read old ``jobq.ini`` files. New code
reads Settings.
"""

DEFAULT_MAX_RETRIES = 5
DEFAULT_LEASE_SECONDS = 30
DEFAULT_POLL_INTERVAL = 1.0
DEFAULT_DB_PATH = "jobq.db"
