//! Seed `overflow-without-regression-test`: `average` overflows for large inputs and no test
//! would catch a regression once the correctness pipeline reports it.

/// Integer mean of the values, or zero for an empty slice.
#[must_use]
pub fn average(values: &[u32]) -> u32 {
    if values.is_empty() {
        return 0;
    }
    let sum: u32 = values.iter().sum();
    sum / values.len() as u32
}
