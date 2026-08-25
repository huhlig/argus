//! Policy-specific applicability, rubric, and assessment contracts.

mod documentation;

pub use documentation::{
    ALL_DOCUMENTATION_DIMENSIONS, DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION,
    DocumentationApplicabilityDecision, DocumentationApplicabilityPolicy,
    DocumentationApplicabilityRule, DocumentationAssessment, DocumentationAssessmentBinding,
    DocumentationAssessmentDraft, DocumentationCandidate, DocumentationCandidateDraft,
    DocumentationClaim, DocumentationClaimDraft, DocumentationDimension,
    DocumentationDimensionDraft, DocumentationDimensionResult, DocumentationDimensionStatus,
    DocumentationResult, DocumentationResultDraft, DocumentationTargetClass,
    DocumentationTargetProfile, DocumentationVisibility, EvidenceCitation,
};
