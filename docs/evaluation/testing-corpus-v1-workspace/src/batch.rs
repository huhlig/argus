//! Seed `untested-boundary`: `chunk` is tested only with an exact multiple of `size`.

/// Splits items into consecutive chunks of at most `size` elements.
///
/// # Panics
///
/// Panics when `size` is zero.
pub fn chunk<T: Clone>(items: &[T], size: usize) -> Vec<Vec<T>> {
    assert!(size > 0, "chunk size must be positive");
    items.chunks(size).map(<[T]>::to_vec).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_an_exact_multiple() {
        assert_eq!(chunk(&[1, 2, 3, 4], 2), vec![vec![1, 2], vec![3, 4]]);
    }
}
