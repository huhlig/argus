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

//! Versioned Langchart workflows owned by Argus.

mod actor_registry;
mod candidate_actor;
mod checkpoint;
mod documentation_broker;
mod documentation_outcome_actor;
mod documentation_plan;
mod documentation_review;
mod documentation_runtime;
mod documentation_worker;
mod evidence_actor;
mod outcome;
mod outcome_actor;
mod recovery;
mod review_actor;
mod workflow_data;

use langchart_model::{validation::CompiledWorkflow, workflow::WorkflowDocument};
use std::{error::Error, fmt};

pub use actor_registry::{ActorFactory, ActorRegistry, ActorRegistryError};
pub use candidate_actor::{CandidateRecorderActor, FindingWorkSchedulerActor};
pub use checkpoint::{CHECKPOINT_DATABASE_FILE, CheckpointOpenError, open_checkpoint_store};
pub use documentation_broker::documentation_worker_runtime;
pub use documentation_outcome_actor::{
    DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND, DocumentationOutcomeActor,
    DurableDocumentationOutcomeActor,
};
pub use documentation_plan::{
    DOCUMENTATION_EVIDENCE_PACKAGE_ARTIFACT_KIND, DOCUMENTATION_REVIEW_CONTEXT_ARTIFACT_KIND,
    DOCUMENTATION_REVIEW_PLAN_SCHEMA_VERSION, DocumentationEvidenceCatalog,
    DocumentationReviewAdmission, DocumentationReviewBatch, DocumentationReviewMaterialization,
    DocumentationReviewPlan, DocumentationReviewPlanner, DocumentationReviewUnit,
};
pub use documentation_review::{
    DocumentationAssessmentContract, DocumentationReviewTransportValidator,
    documentation_assessment_draft_schema,
};
pub use documentation_runtime::{DocumentationRuntimeIdentity, documentation_actor_registry};
pub use documentation_worker::{
    DocumentationWorker, DocumentationWorkerConfig, DocumentationWorkerResult,
    DocumentationWorkerRuntime, WorkflowFailureDiagnostics,
};
pub use evidence_actor::{EvidenceExpander, EvidenceExpansionActor, EvidenceRequestEvaluatorActor};
pub use outcome::{
    EffectiveOutcome, LogicalOutcomeKey, OutcomeDisposition, OutcomeError, OutcomeKind,
    OutcomeProvenance, OutcomeReceipt, OutcomeRecorder,
};
pub use outcome_actor::OutcomeRecorderActor;
pub use recovery::{
    ActorIdentity, RECOVERY_MANIFEST_SCHEMA_VERSION, RecoveryError, RecoveryManifest,
    RecoveryStore, WorkflowArtifactIdentity,
};
pub use review_actor::{
    PolicyAssessmentContract, PolicyReviewDecisionValidator, PrimaryReviewActor,
    ReviewDecisionValidator, review_decision_schema, review_decision_schema_for,
};
pub use workflow_data::{
    CandidateFindingRecord, EvidenceExpansionRecord, EvidenceRequestDecision,
    EvidenceRequestDisposition, PrimaryReviewDecision, ReviewWorkflowData,
    WORKFLOW_DATA_DATABASE_FILE, WORKFLOW_DATA_SCHEMA_VERSION, WorkflowDataError,
    WorkflowDataRecord, WorkflowDataStore, WorkflowDataWrite, verification_work_id,
};

pub const TARGET_REVIEW_WORKFLOW_ID: &str = "argus.target-review";
pub const TARGET_REVIEW_WORKFLOW_VERSION: &str = "1.0.0";

const TARGET_REVIEW_SOURCE: &str = include_str!("workflows/target_review_v1.json");

pub(crate) const fn target_review_source() -> &'static str {
    TARGET_REVIEW_SOURCE
}

pub fn target_review_document() -> Result<WorkflowDocument, serde_json::Error> {
    serde_json::from_str(TARGET_REVIEW_SOURCE)
}

pub fn compile_target_review() -> Result<CompiledWorkflow, WorkflowError> {
    let document = target_review_document().map_err(WorkflowError::Parse)?;
    langchart_model::validation::compile(document).map_err(WorkflowError::Compile)
}

#[must_use]
pub fn target_review_hash() -> String {
    blake3::hash(TARGET_REVIEW_SOURCE.as_bytes())
        .to_hex()
        .to_string()
}

#[derive(Debug)]
pub enum WorkflowError {
    Parse(serde_json::Error),
    Compile(langchart_model::error::CompileError),
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(formatter, "built-in workflow JSON is invalid: {error}"),
            Self::Compile(error) => write!(formatter, "built-in workflow is invalid: {error}"),
        }
    }
}

impl Error for WorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Parse(error) => Some(error),
            Self::Compile(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_workflow_has_stable_identity_and_compiles() {
        let document = target_review_document().unwrap();
        assert_eq!(document.id.as_ref(), TARGET_REVIEW_WORKFLOW_ID);
        assert_eq!(document.version.as_ref(), TARGET_REVIEW_WORKFLOW_VERSION);
        assert_eq!(target_review_hash().len(), 64);
        compile_target_review().unwrap();
    }
}
