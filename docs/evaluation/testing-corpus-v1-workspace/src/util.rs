//! Known-clean controls: behaviour and edge cases are fully tested.

/// Clamps a signed percentage into `0..=100`.
#[must_use]
pub fn clamp_percent(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 100)).unwrap_or(100)
}

/// Whether a number is even.
#[must_use]
pub const fn is_even(value: i64) -> bool {
    value % 2 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_below_within_and_above_range() {
        assert_eq!(clamp_percent(-5), 0);
        assert_eq!(clamp_percent(0), 0);
        assert_eq!(clamp_percent(42), 42);
        assert_eq!(clamp_percent(100), 100);
        assert_eq!(clamp_percent(250), 100);
        assert_eq!(clamp_percent(i32::MIN), 0);
        assert_eq!(clamp_percent(i32::MAX), 100);
    }

    #[test]
    fn classifies_even_and_odd_including_negatives_and_extremes() {
        assert!(is_even(0));
        assert!(is_even(-2));
        assert!(!is_even(7));
        assert!(!is_even(-7));
        assert!(is_even(i64::MIN));
        assert!(!is_even(i64::MAX));
    }
}
