//! Portable domain vocabulary shared by every Argus subsystem.

mod audit;
mod error;
mod evidence;
mod id;
mod lifecycle;
mod source;
mod target;

pub use error::{ArgusError, ErrorCode};
pub use evidence::{
    EvidenceKind, EvidenceOrigin, EvidenceProvenance, EvidenceRecord, ResolutionQuality,
};
pub use id::{
    AssessmentId, AttemptId, ConfigurationId, EvidenceId, FindingId, PolicyId, RelationId, RunId,
    SnapshotId, TargetId, WorkItemId, WorkflowId,
};
pub use lifecycle::{
    AdjudicationState, ApplicabilityState, AssessmentState, AuditState, ExecutionState,
    InventoryState, VerificationState,
};
pub use source::{ByteSpan, ContentHash, LineColumn, SourceLocation, SourcePath};
pub use target::{Capability, CapabilityStatus, PortableTargetKind, TargetKind, TargetVisibility};

/// Schema version for records introduced by the core crate.
pub const CORE_SCHEMA_VERSION: u32 = 1;

/// Serialization envelope required for every portable persisted core record.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Versioned<T> {
    pub schema_version: u32,
    pub record: T,
}

impl<T> Versioned<T> {
    #[must_use]
    pub const fn current(record: T) -> Self {
        Self {
            schema_version: CORE_SCHEMA_VERSION,
            record,
        }
    }

    pub fn into_current(self) -> Result<T, ArgusError> {
        if self.schema_version != CORE_SCHEMA_VERSION {
            return Err(ArgusError::unsupported(format!(
                "unsupported core schema version {}",
                self.schema_version
            )));
        }
        Ok(self.record)
    }
}
pub use audit::{
    Assessment, Attempt, AuditModel, Confidence, Finding, HumanAdjudication, InventoryCoverage,
    Recommendation, Relation, RelationProvenance, Severity, Target, WorkItem,
};
