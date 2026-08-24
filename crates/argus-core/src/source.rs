use serde::{Deserialize, Serialize};

/// A validated, repository-relative path using `/` separators.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourcePath(String);

impl SourcePath {
    pub fn new(value: impl Into<String>) -> Result<Self, crate::ArgusError> {
        let value = value.into().replace('\\', "/");
        let invalid = value.is_empty()
            || value.starts_with('/')
            || value
                .split('/')
                .any(|part| part.is_empty() || part == ".." || part == ".")
            || value.contains('\0');
        if invalid {
            return Err(crate::ArgusError::invalid_input(
                "source path must be a normalized repository-relative path",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Half-open byte range in captured source content.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ByteSpan {
    pub start: u64,
    pub end: u64,
}

impl ByteSpan {
    pub fn new(start: u64, end: u64) -> Result<Self, crate::ArgusError> {
        if start > end {
            return Err(crate::ArgusError::invariant(
                "source span start exceeds end",
            ));
        }
        Ok(Self { start, end })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub path: SourcePath,
    pub bytes: ByteSpan,
    pub start: Option<LineColumn>,
    pub end: Option<LineColumn>,
}

/// BLAKE3 hash of immutable source bytes.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, crate::ArgusError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(crate::ArgusError::invalid_input("invalid content hash"));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
}
