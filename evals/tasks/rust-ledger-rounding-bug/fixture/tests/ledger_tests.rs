use ledger::{Kind, Ledger, Money, Order};

fn usd(text: &str) -> Money {
    text.parse().unwrap()
}

#[test]
fn parses_and_displays_amounts() {
    assert_eq!(usd("12").cents(), 1200);
    assert_eq!(usd("12.3").cents(), 1230);
    assert_eq!(usd("$12.34").cents(), 1234);
    assert_eq!(usd("-0.35").cents(), -35);
    assert_eq!(usd("-12.50").to_string(), "-12.50");
    assert_eq!(Money::from_cents(-5).to_string(), "-0.05");
    assert!("12.345".parse::<Money>().is_err());
    assert!("1,000".parse::<Money>().is_err());
    assert!("".parse::<Money>().is_err());
}

#[test]
fn percent_rounds_to_the_nearest_cent() {
    assert_eq!(usd("10.00").percent(825), usd("0.83")); // 82.5 cents
    assert_eq!(usd("19.99").percent(1000), usd("2.00")); // 199.9 cents
    assert_eq!(usd("0.25").percent(1000), usd("0.03")); // 2.5 cents
    assert_eq!(usd("4.00").percent(0), Money::ZERO);
}

#[test]
fn split_gives_leftover_cents_to_the_first_parts() {
    assert_eq!(usd("1.00").split(3), vec![usd("0.34"), usd("0.33"), usd("0.33")]);
    assert_eq!(usd("10.00").split(4), vec![usd("2.50"); 4]);
    assert_eq!(usd("-10.00").split(2), vec![usd("-5.00"); 2]);
    assert_eq!(usd("0.02").split(3), vec![usd("0.01"), usd("0.01"), Money::ZERO]);
}

#[test]
fn charge_posts_subtotal_and_tax() {
    let order = Order::new(7).with_line("Notebook", usd("4.50"), 2).with_line("Pen", usd("1.00"), 1);
    let mut ledger = Ledger::new(825);
    assert_eq!(ledger.charge(&order), usd("10.83"));
    assert_eq!(ledger.balance(), usd("10.83"));
    let kinds: Vec<Kind> = ledger.entries().iter().map(|entry| entry.kind).collect();
    assert_eq!(kinds, [Kind::Charge, Kind::Tax]);
}

#[test]
fn full_refund_clears_the_balance() {
    let order = Order::new(8).with_line("Mug", usd("10.00"), 1);
    let mut ledger = Ledger::new(1000);
    ledger.charge(&order);
    assert_eq!(ledger.refund(&order), usd("-11.00"));
    assert_eq!(ledger.balance(), Money::ZERO);
}

#[test]
fn refund_is_split_across_cards() {
    let order = Order::new(9).with_line("Lamp", usd("20.00"), 1);
    let mut ledger = Ledger::new(0);
    ledger.charge(&order);
    assert_eq!(ledger.refund_to_cards(&order, 2), vec![usd("-10.00"); 2]);
    assert_eq!(ledger.balance(), Money::ZERO);
}
