# ledger

Money arithmetic for the billing service. Every amount is an integer number of
cents (`Money`); negative amounts are credits such as refunds.

- `money` — `Money`, parsing and display, tax percentages, splitting an amount
  across several payment cards.
- `order` — orders and their lines.
- `ledger` — posting charges and refunds for orders and keeping the balance.

Run the tests with `cargo test`.
