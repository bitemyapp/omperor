-- Shop database schema. All timestamps are UTC, formatted 'YYYY-MM-DD HH:MM:SS'.

CREATE TABLE customers (
    id       INTEGER PRIMARY KEY,
    email    TEXT    NOT NULL UNIQUE,
    is_test  INTEGER NOT NULL DEFAULT 0     -- 1 for internal QA accounts
);

CREATE TABLE orders (
    id           INTEGER PRIMARY KEY,
    customer_id  INTEGER NOT NULL REFERENCES customers (id),
    created_at   TEXT    NOT NULL,          -- UTC
    status       TEXT    NOT NULL CHECK (status IN ('pending', 'paid', 'cancelled')),
    total_cents  INTEGER NOT NULL CHECK (total_cents > 0)
);

-- An order can be refunded in several parts. Refunding does not change the
-- order's status.
CREATE TABLE refunds (
    id            INTEGER PRIMARY KEY,
    order_id      INTEGER NOT NULL REFERENCES orders (id),
    refunded_at   TEXT    NOT NULL,         -- UTC
    amount_cents  INTEGER NOT NULL CHECK (amount_cents > 0)
);

CREATE INDEX orders_created_at ON orders (created_at);
CREATE INDEX refunds_order_id ON refunds (order_id);
