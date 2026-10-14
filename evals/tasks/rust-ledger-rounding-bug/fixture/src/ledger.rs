//! A customer's ledger: charges and refunds for orders.

use crate::money::Money;
use crate::order::Order;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Charge,
    Tax,
    Refund,
    RefundTax,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub order_id: u64,
    pub kind: Kind,
    pub amount: Money,
}

/// Entries posted for one customer, with a single sales-tax rate.
#[derive(Clone, Debug)]
pub struct Ledger {
    tax_bps: i64,
    entries: Vec<Entry>,
}

impl Ledger {
    /// A ledger charging `tax_bps` basis points of sales tax (825 = 8.25 %).
    pub fn new(tax_bps: i64) -> Ledger {
        Ledger { tax_bps, entries: Vec::new() }
    }

    pub fn tax_bps(&self) -> i64 {
        self.tax_bps
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// What the customer owes: positive after a charge, zero after the
    /// charge is fully refunded.
    pub fn balance(&self) -> Money {
        self.entries.iter().map(|entry| entry.amount).sum()
    }

    /// Posts the order's subtotal and tax; returns the total charged.
    pub fn charge(&mut self, order: &Order) -> Money {
        let subtotal = order.subtotal();
        self.post(order.id, Kind::Charge, subtotal) + self.post(order.id, Kind::Tax, subtotal.percent(self.tax_bps))
    }

    /// Posts a full refund of the order (negative subtotal and tax); returns
    /// the total refunded, which is negative.
    pub fn refund(&mut self, order: &Order) -> Money {
        let credit = -order.subtotal();
        self.post(order.id, Kind::Refund, credit) + self.post(order.id, Kind::RefundTax, credit.percent(self.tax_bps))
    }

    /// Refunds the order and divides the refund across the `cards` payment
    /// cards that paid for it. The card amounts are negative and sum to the
    /// total refunded.
    pub fn refund_to_cards(&mut self, order: &Order, cards: usize) -> Vec<Money> {
        self.refund(order).split(cards)
    }

    fn post(&mut self, order_id: u64, kind: Kind, amount: Money) -> Money {
        self.entries.push(Entry { order_id, kind, amount });
        amount
    }
}
