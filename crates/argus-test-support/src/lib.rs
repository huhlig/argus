//! Shared fixture builders. Production crates must never depend on this crate.

/// Fixture scenario categories required by the implementation plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureKind {
    Clean,
    Dirty,
    Generated,
    Malformed,
    MultiCrate,
}
