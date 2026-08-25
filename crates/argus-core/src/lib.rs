// Copyright 2026 Hans W. Uhlig
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
    SnapshotId, SourceTreeId, TargetId, WorkItemId, WorkflowId,
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
