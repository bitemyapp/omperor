//! Orders and their lines.

use crate::money::Money;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub description: String,
    pub unit_price: Money,
    pub quantity: u32,
}

impl Line {
    pub fn amount(&self) -> Money {
        Money::from_cents(self.unit_price.cents() * i64::from(self.quantity))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    pub id: u64,
    pub lines: Vec<Line>,
}

impl Order {
    pub fn new(id: u64) -> Order {
        Order { id, lines: Vec::new() }
    }

    pub fn with_line(mut self, description: &str, unit_price: Money, quantity: u32) -> Order {
        self.lines.push(Line {
            description: description.to_owned(),
            unit_price,
            quantity,
        });
        self
    }

    /// The sum of the line amounts, before tax.
    pub fn subtotal(&self) -> Money {
        self.lines.iter().map(Line::amount).sum()
    }
}
