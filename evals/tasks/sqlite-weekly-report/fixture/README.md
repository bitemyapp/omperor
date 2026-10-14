# shop-analytics

Ad-hoc reporting over exports of the shop database.

- `db/schema.sql` — the schema (timestamps are UTC).
- `db/seed.sql` — the current export's data.
- `shop.db` — the export as a SQLite database, built from the two files above:

```sh
python3 -c "import sqlite3; c = sqlite3.connect('shop.db'); c.executescript(open('db/schema.sql').read() + open('db/seed.sql').read())"
```

Reports are plain Python 3 scripts using only the standard library.
