//! Seed `hot-path-without-benchmark`: `checksum` runs for every packet and has no benchmark.

/// Checksum computed for every packet on the receive hot path.
#[must_use]
pub fn checksum(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_known_input() {
        assert_eq!(checksum(b"Wikipedia"), 0x11E6_0398);
    }
}
