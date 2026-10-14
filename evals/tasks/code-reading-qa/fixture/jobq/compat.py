"""Reading the ``jobq.ini`` files used before 0.8."""

import configparser

from . import config
from .settings import Settings


def load_legacy_ini(path) -> Settings:
    """Settings from an old ini file; missing keys use the old defaults."""
    parser = configparser.ConfigParser()
    with open(path, encoding="utf-8") as handle:
        parser.read_file(handle)
    section = parser["jobq"] if parser.has_section("jobq") else parser["DEFAULT"]
    return Settings(
        db_path=section.get("database", config.DEFAULT_DB_PATH),
        max_retries=section.getint("retries", config.DEFAULT_MAX_RETRIES),
        lease_seconds=section.getfloat("lease", config.DEFAULT_LEASE_SECONDS),
        poll_interval=section.getfloat("poll", config.DEFAULT_POLL_INTERVAL),
    )
