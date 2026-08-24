//! Immutable evidence storage and bounded review-context construction.

mod context;
mod package;
mod request;
mod store;

pub use package::{
    CandidateAvailability, EvidenceBudget, EvidenceCandidate, EvidenceDisposition, EvidencePackage,
    EvidencePackageBuilder, EvidencePackageItem, PackageArtifact, PolicyEvidenceRequirements,
};
pub use request::{
    AuthorizedEvidenceExpansion, EvidenceExpansionPolicy, EvidenceRequest,
    EvidenceRequestAuthorizer, ExpansionDenial, ExpansionDenialReason, ExpansionUsage,
};
pub use store::{DataClassification, EvidenceEnvelope, EvidenceStore, StoredEvidence};

pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;
pub use context::{
    ContextArtifact, FramedEvidence, ReviewContextBuilder, ReviewContextFrame, TrustedControl,
};
