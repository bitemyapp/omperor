#!/usr/bin/env python3
"""Weekly sales summary for finance.

    python3 report.py PATH_TO_DB > weekly.csv

Weeks run Monday to Sunday in New York time. Only paid orders from real
customers count; fully refunded orders are left out, and partial refunds are
subtracted in the week the order was placed. Every week between the first and
the last counted order gets a row, oldest first.
"""

import csv
import datetime
import sqlite3
import sys
from pathlib import Path
from zoneinfo import ZoneInfo

NEW_YORK = ZoneInfo("America/New_York")
HEADER = ["week_start", "orders", "customers", "gross", "refunds", "net"]

QUERY = """
SELECT o.customer_id, o.created_at, o.total_cents, COALESCE(r.refunded, 0)
FROM orders AS o
JOIN customers AS c ON c.id = o.customer_id
LEFT JOIN (SELECT order_id, SUM(amount_cents) AS refunded FROM refunds GROUP BY order_id) AS r
    ON r.order_id = o.id
WHERE o.status = 'paid' AND c.is_test = 0
"""


def week_start(created_at: str) -> datetime.date:
    """The New York Monday of the week containing a UTC timestamp."""
    utc = datetime.datetime.strptime(created_at, "%Y-%m-%d %H:%M:%S").replace(tzinfo=datetime.timezone.utc)
    local = utc.astimezone(NEW_YORK).date()
    return local - datetime.timedelta(days=local.weekday())


def dollars(cents: int) -> str:
    sign = "-" if cents < 0 else ""
    return f"{sign}{abs(cents) // 100}.{abs(cents) % 100:02d}"


def summarize(connection):
    weeks = {}
    for customer, created_at, total, refunded in connection.execute(QUERY):
        if refunded >= total:
            continue
        week = weeks.setdefault(week_start(created_at), {"orders": 0, "customers": set(), "gross": 0, "refunds": 0})
        week["orders"] += 1
        week["customers"].add(customer)
        week["gross"] += total
        week["refunds"] += refunded
    rows = []
    if weeks:
        day, last = min(weeks), max(weeks)
        while day <= last:
            week = weeks.get(day, {"orders": 0, "customers": (), "gross": 0, "refunds": 0})
            rows.append([day.isoformat(), week["orders"], len(week["customers"]), dollars(week["gross"]),
                         dollars(week["refunds"]), dollars(week["gross"] - week["refunds"])])
            day += datetime.timedelta(weeks=1)
    return rows


def main(argv) -> int:
    if len(argv) != 2:
        print("usage: report.py PATH_TO_DB", file=sys.stderr)
        return 2
    path = Path(argv[1])
    if not path.is_file():
        print(f"report.py: no such database: {path}", file=sys.stderr)
        return 1
    connection = sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True)
    try:
        rows = summarize(connection)
    finally:
        connection.close()
    writer = csv.writer(sys.stdout, lineterminator="\n")
    writer.writerow(HEADER)
    writer.writerows(rows)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
