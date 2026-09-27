//! Seed `untested-core-logic`: discount calculation has no tests at all.

/// Applies a percentage discount to a price in cents, capping the discount at 100%.
#[must_use]
pub fn apply_discount(price_cents: u64, percent: u8) -> u64 {
    let percent = u64::from(percent.min(100));
    price_cents - price_cents * percent / 100
}
