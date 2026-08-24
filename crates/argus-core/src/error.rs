use std::{error::Error, fmt};

/// Stable, machine-readable category for an Argus error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    InvalidInput,
    Unsupported,
    InvariantViolation,
    Io,
}

/// An error with a stable category and human-readable context.
#[derive(Debug)]
pub struct ArgusError {
    code: ErrorCode,
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl ArgusError {
    #[must_use]
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }

    #[must_use]
    pub fn invariant(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvariantViolation, message)
    }

    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    #[must_use]
    pub fn with_source(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }
}

impl fmt::Display for ArgusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for ArgusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}
