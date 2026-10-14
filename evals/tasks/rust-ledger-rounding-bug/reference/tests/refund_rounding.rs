use ledger::{Ledger, Money, Order};

#[test]
fn refund_of_half_cent_tax_zeroes_the_balance() {
    let order = Order::new(1042).with_line("Desk lamp", Money::from_cents(1235), 1);
    let mut ledger = Ledger::new(1000);
    assert_eq!(ledger.charge(&order), Money::from_cents(1359));
    let cards = ledger.refund_to_cards(&order, 2);
    assert_eq!(cards, vec![Money::from_cents(-680), Money::from_cents(-679)]);
    assert_eq!(ledger.balance(), Money::ZERO);
}

#[test]
fn credits_mirror_charges() {
    for cents in -500..=500 {
        assert_eq!(Money::from_cents(-cents).percent(1000), -Money::from_cents(cents).percent(1000));
        let mirrored: Vec<Money> = Money::from_cents(-cents).split(3).into_iter().map(|m| -m).collect();
        assert_eq!(mirrored, Money::from_cents(cents).split(3));
    }
}
