//! Money amounts in integer cents.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

/// An amount of money in cents. Negative amounts are credits (refunds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money(i64);

impl Money {
    pub const ZERO: Money = Money(0);

    pub const fn from_cents(cents: i64) -> Money {
        Money(cents)
    }

    pub const fn cents(self) -> i64 {
        self.0
    }

    pub fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn abs(self) -> Money {
        Money(self.0.abs())
    }

    /// `bps` basis points of this amount (100 bps = 1 %), rounded to the
    /// nearest cent. Halves round away from zero, so the tax on a refund is
    /// always the exact negation of the tax on the matching charge:
    /// `(-a).percent(bps) == -(a.percent(bps))`.
    pub fn percent(self, bps: i64) -> Money {
        // Add half of the divisor so the division rounds to the nearest cent.
        Money((self.0 * bps + 5_000).div_euclid(10_000))
    }

    /// Splits this amount into `parts` amounts that differ by at most one
    /// cent and always sum to exactly `self`. Earlier parts receive the
    /// leftover cents: `1.00` split three ways is `[0.34, 0.33, 0.33]`.
    /// Splitting a credit mirrors splitting its absolute value, so `-1.00`
    /// split three ways is `[-0.34, -0.33, -0.33]`.
    ///
    /// # Panics
    ///
    /// If `parts` is zero.
    pub fn split(self, parts: usize) -> Vec<Money> {
        assert!(parts > 0, "cannot split an amount into zero parts");
        let count = parts as i64;
        let base = self.0 / count;
        let remainder = self.0 % count;
        let mut out = vec![Money(base); parts];
        for index in 0..remainder {
            out[index as usize].0 += 1;
        }
        out
    }
}

impl Add for Money {
    type Output = Money;
    fn add(self, other: Money) -> Money {
        Money(self.0 + other.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, other: Money) {
        self.0 += other.0;
    }
}

impl Sub for Money {
    type Output = Money;
    fn sub(self, other: Money) -> Money {
        Money(self.0 - other.0)
    }
}

impl SubAssign for Money {
    fn sub_assign(&mut self, other: Money) {
        self.0 -= other.0;
    }
}

impl Neg for Money {
    type Output = Money;
    fn neg(self) -> Money {
        Money(-self.0)
    }
}

impl Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Money {
        iter.fold(Money::ZERO, Add::add)
    }
}

impl<'a> Sum<&'a Money> for Money {
    fn sum<I: Iterator<Item = &'a Money>>(iter: I) -> Money {
        iter.copied().sum()
    }
}

/// Formats as `12.34` or `-0.35`, without a currency symbol.
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let cents = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:02}", cents / 100, cents % 100)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseMoneyError(String);

impl fmt::Display for ParseMoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid amount {:?}", self.0)
    }
}

impl std::error::Error for ParseMoneyError {}

/// Parses `12`, `12.3`, `12.34`, `-0.35`, or `$1250.00`.
impl FromStr for Money {
    type Err = ParseMoneyError;

    fn from_str(text: &str) -> Result<Money, ParseMoneyError> {
        let error = || ParseMoneyError(text.to_owned());
        let (negative, rest) = match text.trim().strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.trim()),
        };
        let rest = rest.strip_prefix('$').unwrap_or(rest);
        let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(whole) || (rest.contains('.') && !digits(fraction)) || fraction.len() > 2 {
            return Err(error());
        }
        let whole: i64 = whole.parse().map_err(|_| error())?;
        let fraction: i64 = match fraction.len() {
            0 => 0,
            1 => fraction.parse::<i64>().map_err(|_| error())? * 10,
            _ => fraction.parse().map_err(|_| error())?,
        };
        let cents = whole
            .checked_mul(100)
            .and_then(|c| c.checked_add(fraction))
            .ok_or_else(error)?;
        Ok(Money(if negative { -cents } else { cents }))
    }
}
