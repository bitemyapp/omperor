# invoicer

Turns time-tracking exports into monthly client invoices.

```console
$ python3 -m invoicer render samples/september.csv --rates samples/rates.csv --client Northwind --month 2024-09
```

- `invoicer/loader.py` reads the time export and the rate card (CSV).
- `invoicer/billing.py` prices each entry (hourly rate per client and project).
- `invoicer/render.py` lays out the invoice text.

Tests: `python3 -m unittest discover -s tests -t .`
