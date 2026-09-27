//! Seed `untested-error-path`: only the happy path of `parse_port` is tested.

/// Reasons a port string is rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum PortError {
    Empty,
    NotNumeric,
    OutOfRange,
}

/// Parses a TCP port, rejecting empty, non-numeric, zero, and out-of-range input.
pub fn parse_port(text: &str) -> Result<u16, PortError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(PortError::Empty);
    }
    let value: u32 = trimmed.parse().map_err(|_| PortError::NotNumeric)?;
    match u16::try_from(value) {
        Ok(0) | Err(_) => Err(PortError::OutOfRange),
        Ok(port) => Ok(port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_port() {
        assert_eq!(parse_port("8080"), Ok(8080));
    }
}
