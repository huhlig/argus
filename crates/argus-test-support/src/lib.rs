//! Shared fixture builders. Production crates must never depend on this crate.

mod documentation;

pub use documentation::{
    SeededDocumentationFixture, SeededDocumentationSource, seeded_documentation_fixture,
};

/// Fixture scenario categories required by the implementation plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureKind {
    Clean,
    Dirty,
    Generated,
    Malformed,
    MultiCrate,
}
