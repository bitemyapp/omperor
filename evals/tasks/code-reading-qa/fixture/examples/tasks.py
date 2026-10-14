"""Example handlers: ``jobq worker --handlers examples.tasks``."""

import random

from jobq import PermanentError, TransientError, task


@task()
def send_invoice(invoice_id: str) -> None:
    if not invoice_id.isdigit():
        raise PermanentError(f"bad invoice id {invoice_id!r}")
    if random.random() < 0.2:
        raise TransientError("mail server busy")
    print(f"sent invoice {invoice_id}")


@task()
def flaky_export() -> None:
    raise TransientError("warehouse API timed out")
