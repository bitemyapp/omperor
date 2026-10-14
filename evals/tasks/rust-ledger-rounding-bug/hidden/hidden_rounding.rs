//! Hidden grader tests: copied into the crate's tests/ directory by grade.py.
use ledger::{Kind, Ledger, Money, Order};

fn usd(text: &str) -> Money {
    text.parse().unwrap()
}

/// Round half away from zero, computed independently in i128.
fn expected_percent(cents: i64, bps: i64) -> i64 {
    let scaled = i128::from(cents) * i128::from(bps);
    let quotient = scaled / 10_000;
    let remainder = scaled % 10_000;
    let rounded = if remainder.abs() * 2 >= 10_000 { quotient + scaled.signum() } else { quotient };
    rounded as i64
}

#[test]
fn hidden_percent_rounds_half_away_from_zero_for_credits() {
    assert_eq!(usd("12.35").percent(1000), usd("1.24"));
    assert_eq!(usd("-12.35").percent(1000), usd("-1.24"));
    assert_eq!(usd("-0.25").percent(1000), usd("-0.03"));
    assert_eq!(usd("-10.00").percent(825), usd("-0.83"));
    assert_eq!(usd("-10.00").percent(1000), usd("-1.00"));
    assert_eq!(usd("-0.04").percent(1000), Money::ZERO);
}

#[test]
fn hidden_percent_is_symmetric_and_exact() {
    for bps in [0, 1, 7, 250, 600, 825, 1000, 1250, 3333, 10_000] {
        for cents in -20_000..=20_000 {
            let got = Money::from_cents(cents).percent(bps).cents();
            assert_eq!(got, expected_percent(cents, bps), "{cents} cents at {bps} bps");
            assert_eq!(
                Money::from_cents(-cents).percent(bps),
                -Money::from_cents(cents).percent(bps),
                "{cents} cents at {bps} bps is not symmetric"
            );
        }
    }
}

#[test]
fn hidden_split_sums_and_mirrors() {
    for cents in -3_000..=3_000 {
        for parts in 1..=7usize {
            let amount = Money::from_cents(cents);
            let split = amount.split(parts);
            assert_eq!(split.len(), parts);
            assert_eq!(split.iter().sum::<Money>(), amount, "{cents} into {parts}");
            let magnitudes: Vec<i64> = split.iter().map(|part| part.cents().abs()).collect();
            assert!(magnitudes.windows(2).all(|w| w[0] >= w[1]), "{cents} into {parts}: {split:?}");
            assert!(magnitudes[0] - magnitudes[parts - 1] <= 1, "{cents} into {parts}: {split:?}");
            let mirrored: Vec<Money> = Money::from_cents(-cents).split(parts).into_iter().map(|m| -m).collect();
            assert_eq!(mirrored, split, "{cents} into {parts} does not mirror");
        }
    }
    assert_eq!(usd("-1.00").split(3), vec![usd("-0.34"), usd("-0.33"), usd("-0.33")]);
    assert_eq!(usd("-0.02").split(3), vec![usd("-0.01"), usd("-0.01"), Money::ZERO]);
}

#[test]
fn hidden_reported_order_refund_zeroes_the_balance() {
    let order = Order::new(1042).with_line("Desk lamp", usd("12.35"), 1);
    let mut ledger = Ledger::new(1000);
    assert_eq!(ledger.charge(&order), usd("13.59"));
    let cards = ledger.refund_to_cards(&order, 2);
    assert_eq!(cards, vec![usd("-6.80"), usd("-6.79")]);
    assert_eq!(cards.iter().sum::<Money>(), usd("-13.59"));
    assert_eq!(ledger.balance(), Money::ZERO);
    let kinds: Vec<Kind> = ledger.entries().iter().map(|entry| entry.kind).collect();
    assert_eq!(kinds, [Kind::Charge, Kind::Tax, Kind::Refund, Kind::RefundTax]);
    assert_eq!(ledger.entries()[3].amount, usd("-1.24"));
}

#[test]
fn hidden_every_full_refund_zeroes_the_balance() {
    for bps in [0, 600, 825, 1000, 1250] {
        for price in 1..=2_000 {
            let order = Order::new(1).with_line("Item", Money::from_cents(price), 1 + (price % 3) as u32);
            let mut ledger = Ledger::new(bps);
            let charged = ledger.charge(&order);
            let cards = ledger.refund_to_cards(&order, 1 + (price as usize % 4));
            assert_eq!(cards.iter().sum::<Money>(), -charged, "{price} at {bps}");
            assert_eq!(ledger.balance(), Money::ZERO, "{price} at {bps}");
        }
    }
}

#[test]
fn hidden_existing_behaviour_is_kept() {
    assert_eq!(usd("10.00").percent(825), usd("0.83"));
    assert_eq!(usd("0.25").percent(1000), usd("0.03"));
    assert_eq!(usd("19.99").percent(1000), usd("2.00"));
    assert_eq!(usd("1.00").split(3), vec![usd("0.34"), usd("0.33"), usd("0.33")]);
    assert_eq!(usd("-0.35").to_string(), "-0.35");
    assert_eq!(usd("$1250.5").cents(), 125_050);
    assert!("12.345".parse::<Money>().is_err());
    assert_eq!(Ledger::new(825).tax_bps(), 825);
}
