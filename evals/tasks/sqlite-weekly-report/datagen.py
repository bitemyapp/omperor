"""Deterministic shop data. Produces the fixture's db/seed.sql (spring 2024,
across the DST start) and the grader's hidden database (autumn 2024 into
2025, across the DST end and the new year)."""

import datetime
import random
import sqlite3
from zoneinfo import ZoneInfo

NEW_YORK = ZoneInfo("America/New_York")
UTC = datetime.timezone.utc
FORMAT = "%Y-%m-%d %H:%M:%S"

FIXTURE = dict(
    seed=20240212,
    first_monday=datetime.date(2024, 2, 12),
    weeks=9,
    empty_weeks=[datetime.date(2024, 3, 25)],           # shop closed for a migration
    uncounted_weeks=[datetime.date(2024, 4, 1)],        # only cancelled/pending/test orders
    boundary=[
        "2024-03-04 04:30:00",  # Sun 23:30 EST -> week of 02-26 (UTC says 03-04)
        "2024-03-11 03:30:00",  # Sun 23:30 EDT -> week of 03-04
        "2024-03-11 04:30:00",  # Mon 00:30 EDT -> week of 03-11 (fixed EST says 03-04)
        "2024-02-26 05:00:00",  # Mon 00:00:00 EST -> week of 02-26
        "2024-02-26 04:59:59",  # Sun 23:59:59 EST -> week of 02-19
        "2024-03-18 03:59:59",  # Sun 23:59:59 EDT -> week of 03-11
        "2024-03-18 04:00:00",  # Mon 00:00:00 EDT -> week of 03-18
        "2024-03-10 06:59:59",  # Sun 01:59:59 EST, just before the gap -> week of 03-04
        "2024-03-10 07:00:00",  # Sun 03:00:00 EDT, just after the gap -> week of 03-04
    ],
    outside=[("2024-02-05 15:00:00", "paid", True), ("2024-04-16 15:00:00", "cancelled", False),
             ("2024-02-11 20:00:00", "pending", False)],
)

HIDDEN = dict(
    seed=1103,
    first_monday=datetime.date(2024, 10, 14),
    weeks=14,
    empty_weeks=[datetime.date(2024, 11, 25)],
    uncounted_weeks=[datetime.date(2024, 12, 16)],
    boundary=[
        "2024-11-04 04:30:00",  # Sun 23:30 EST -> week of 10-28 (fixed EDT says 11-04)
        "2024-10-28 03:30:00",  # Sun 23:30 EDT -> week of 10-21 (UTC says 10-28)
        "2024-10-21 04:15:00",  # Mon 00:15 EDT -> week of 10-21 (fixed EST says 10-14)
        "2024-11-03 05:30:00",  # Sun 01:30 EDT (first 01:30) -> week of 10-28
        "2024-11-03 06:30:00",  # Sun 01:30 EST (second 01:30) -> week of 10-28
        "2024-12-30 05:00:00",  # Mon 00:00 EST -> week of 12-30
        "2025-01-01 12:00:00",  # Wed, new year -> still week of 12-30
        "2025-01-06 04:59:59",  # Sun 23:59:59 -> week of 12-30
        "2025-01-06 05:00:00",  # Mon 00:00 -> week of 2025-01-06
        "2024-12-30 04:59:59",  # Sun 23:59:59 -> week of 12-23
    ],
    outside=[("2024-10-07 12:00:00", "paid", True), ("2025-01-21 12:00:00", "pending", False),
             ("2025-01-22 12:00:00", "paid", True)],
)

TEST_CUSTOMERS = {7, 19, 33}


def local_midnight_utc(day):
    return datetime.datetime.combine(day, datetime.time(), NEW_YORK).astimezone(UTC)


def week_of(created_at):
    local = datetime.datetime.strptime(created_at, FORMAT).replace(tzinfo=UTC).astimezone(NEW_YORK).date()
    return local - datetime.timedelta(days=local.weekday())


def generate(seed, first_monday, weeks, empty_weeks, uncounted_weeks, boundary, outside):
    """Returns (customers, orders, refunds) as lists of tuples."""
    rng = random.Random(seed)
    customers = [(i, f"customer{i:03d}@example.com", int(i in TEST_CUSTOMERS)) for i in range(1, 41)]
    real = [c[0] for c in customers if not c[2]]
    start = local_midnight_utc(first_monday)
    end = local_midnight_utc(first_monday + datetime.timedelta(weeks=weeks))
    span = int((end - start).total_seconds())
    orders = []

    def add(created_at, status, customer, total):
        orders.append((len(orders) + 1, customer, created_at, status, total))

    while len(orders) < 34 * weeks:
        created = (start + datetime.timedelta(seconds=rng.randrange(span))).strftime(FORMAT)
        week = week_of(created)
        if week in empty_weeks:
            continue
        status = rng.choices(["paid", "pending", "cancelled"], [80, 8, 12])[0]
        customer = rng.choice(real) if rng.random() > 0.06 else rng.choice(sorted(TEST_CUSTOMERS))
        if week in uncounted_weeks and status == "paid" and customer not in TEST_CUSTOMERS:
            status = rng.choice(["pending", "cancelled"])
        add(created, status, customer, rng.randint(500, 25000))
    for created in boundary:
        add(created, "paid", rng.choice(real), rng.randint(500, 25000))
    for created, status, test in outside:
        add(created, status, rng.choice(sorted(TEST_CUSTOMERS)) if test else rng.choice(real), rng.randint(500, 25000))
    orders.sort(key=lambda order: (order[2], order[0]))
    orders = [(i + 1,) + order[1:] for i, order in enumerate(orders)]

    refunds = []

    def refund(order, cents, days):
        stamp = datetime.datetime.strptime(order[2], FORMAT) + datetime.timedelta(days=days, minutes=rng.randrange(600))
        refunds.append((len(refunds) + 1, order[0], stamp.strftime(FORMAT), cents))

    boundary_ids = {order[0] for order in orders if order[2] in boundary}
    for order in orders:
        if order[0] in boundary_ids:
            continue
        roll = rng.random()
        total = order[4]
        if roll < 0.10:                       # one partial refund
            refund(order, rng.randint(1, total - 1), rng.randint(0, 20))
        elif roll < 0.13:                     # two partial refunds, still short of the total
            first = rng.randint(1, total // 3)
            refund(order, first, rng.randint(0, 5))
            refund(order, rng.randint(1, total - first - 1), rng.randint(6, 25))
        elif roll < 0.17:                     # refunded in full at once
            refund(order, total, rng.randint(0, 20))
        elif roll < 0.20:                     # refunded in full in two parts
            first = rng.randint(1, total - 1)
            refund(order, first, rng.randint(0, 5))
            refund(order, total - first, rng.randint(6, 25))
        elif roll < 0.21:                     # over-refunded (goodwill credit)
            refund(order, total + rng.randint(1, 500), rng.randint(0, 20))
    return customers, orders, refunds


def to_sql(customers, orders, refunds):
    lines = ["-- Generated shop data (UTC timestamps).", "BEGIN;"]
    lines += [f"INSERT INTO customers VALUES ({i}, '{email}', {test});" for i, email, test in customers]
    lines += [f"INSERT INTO orders VALUES ({i}, {c}, '{t}', '{s}', {cents});" for i, c, t, s, cents in orders]
    lines += [f"INSERT INTO refunds VALUES ({i}, {o}, '{t}', {cents});" for i, o, t, cents in refunds]
    lines.append("COMMIT;")
    return "\n".join(lines) + "\n"


def build_database(path, schema_sql, params):
    connection = sqlite3.connect(path)
    connection.executescript(schema_sql + to_sql(*generate(**params)))
    connection.commit()
    connection.close()


def expected_report(path):
    """The summary, computed independently of the agent's code."""
    connection = sqlite3.connect(path)
    test_ids = {row[0] for row in connection.execute("SELECT id FROM customers WHERE is_test != 0")}
    refunded = {}
    for order_id, cents in connection.execute("SELECT order_id, amount_cents FROM refunds"):
        refunded[order_id] = refunded.get(order_id, 0) + cents
    weeks = {}
    for order_id, customer, created, status, total in connection.execute(
            "SELECT id, customer_id, created_at, status, total_cents FROM orders"):
        if status != "paid" or customer in test_ids or refunded.get(order_id, 0) >= total:
            continue
        bucket = weeks.setdefault(week_of(created), {"orders": 0, "customers": set(), "gross": 0, "refunds": 0})
        bucket["orders"] += 1
        bucket["customers"].add(customer)
        bucket["gross"] += total
        bucket["refunds"] += refunded.get(order_id, 0)
    connection.close()

    def dollars(cents):
        return f"{cents // 100}.{cents % 100:02d}"

    rows = [["week_start", "orders", "customers", "gross", "refunds", "net"]]
    day, last = min(weeks), max(weeks)
    while day <= last:
        bucket = weeks.get(day, {"orders": 0, "customers": set(), "gross": 0, "refunds": 0})
        rows.append([day.isoformat(), str(bucket["orders"]), str(len(bucket["customers"])), dollars(bucket["gross"]),
                     dollars(bucket["refunds"]), dollars(bucket["gross"] - bucket["refunds"])])
        day += datetime.timedelta(weeks=1)
    return rows


if __name__ == "__main__":
    import sys
    from pathlib import Path
    target = Path(sys.argv[1])
    target.write_text(to_sql(*generate(**FIXTURE)))
    print(f"wrote {target}")
