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

//! Policy-specific applicability, rubric, and assessment contracts.

mod architecture;
mod correctness;
mod documentation;

pub use architecture::{
    ALL_ARCHITECTURE_DIMENSIONS, ARCHITECTURE_ASSESSMENT_SCHEMA_VERSION,
    ArchitectureApplicabilityDecision, ArchitectureApplicabilityPolicy,
    ArchitectureApplicabilityRule, ArchitectureAssessment, ArchitectureAssessmentBinding,
    ArchitectureAssessmentDraft, ArchitectureCandidate, ArchitectureCandidateDraft,
    ArchitectureCandidateVerification, ArchitectureDimension, ArchitectureDimensionDraft,
    ArchitectureDimensionResult, ArchitectureDimensionStatus, ArchitectureEvidenceCitation,
    ArchitectureFindingKind, ArchitectureResult, ArchitectureResultDraft, ArchitectureResultStatus,
    ArchitectureScope, ArchitectureTargetClass, ArchitectureTargetProfile,
    ArchitectureVerificationStatus, ArchitectureVisibility, ConstituentHealthSummary,
};
pub use correctness::{
    ALL_CORRECTNESS_DIMENSIONS, CORRECTNESS_ASSESSMENT_SCHEMA_VERSION,
    CorrectnessApplicabilityDecision, CorrectnessApplicabilityPolicy, CorrectnessApplicabilityRule,
    CorrectnessAssessment, CorrectnessAssessmentBinding, CorrectnessAssessmentDraft,
    CorrectnessCandidate, CorrectnessCandidateDraft, CorrectnessDefectKind, CorrectnessDimension,
    CorrectnessDimensionDraft, CorrectnessDimensionResult, CorrectnessDimensionStatus,
    CorrectnessEvidenceCitation, CorrectnessResult, CorrectnessResultDraft, CorrectnessTargetClass,
    CorrectnessTargetProfile, CorrectnessVisibility,
};
pub use documentation::{
    ALL_DOCUMENTATION_DIMENSIONS, DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION,
    DocumentationApplicabilityDecision, DocumentationApplicabilityPolicy,
    DocumentationApplicabilityRule, DocumentationAssessment, DocumentationAssessmentBinding,
    DocumentationAssessmentDraft, DocumentationCandidate, DocumentationCandidateDraft,
    DocumentationClaim, DocumentationClaimDraft, DocumentationComparison, DocumentationCoverage,
    DocumentationDimension, DocumentationDimensionDraft, DocumentationDimensionResult,
    DocumentationDimensionStatus, DocumentationResult, DocumentationResultDraft,
    DocumentationTargetClass, DocumentationTargetProfile, DocumentationVisibility,
    EvidenceCitation, SourceMateriality,
};
