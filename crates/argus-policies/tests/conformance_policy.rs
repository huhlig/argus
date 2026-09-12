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

use argus_core::{
    ApplicabilityState, ByteSpan, Confidence, DesignArtifactId, EvidenceId, InventoryState,
    PolicyId, PortableTargetKind, Severity, SourceLocation, SourcePath, Target, TargetId,
    TargetKind, TargetVisibility, WorkItemId,
};
use argus_policies::{
    ALL_CONFORMANCE_DIMENSIONS, CONFORMANCE_ASSESSMENT_SCHEMA_VERSION,
    ConformanceApplicabilityPolicy, ConformanceAssessment, ConformanceAssessmentDraft,
    ConformanceCandidateDraft, ConformanceDimension, ConformanceDimensionDraft,
    ConformanceDimensionResult, ConformanceDimensionStatus, ConformanceDisposition,
    ConformanceResult, ConformanceResultDraft, ConformanceTargetProfile,
};
use std::collections::{BTreeMap, BTreeSet};

fn sample_target(kind: PortableTargetKind, visibility: TargetVisibility) -> Target {
    Target {
        id: TargetId::derive([b"test-target".as_slice()]),
        kind: TargetKind::Portable { kind },
        visibility,
        name: "test_pkg::test_module::Item".to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: SourcePath::new("crates/test/src/lib.rs").unwrap(),
            bytes: ByteSpan::new(0, 100).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    }
}

#[test]
fn applicability_evaluation() {
    let policy = ConformanceApplicabilityPolicy::all_targets().unwrap();

    let callable = sample_target(PortableTargetKind::Callable, TargetVisibility::Public);
    let profile = ConformanceTargetProfile::from_target(&callable);
    let decision = policy.evaluate(&profile);
    assert_eq!(decision.state, ApplicabilityState::Applicable);

    let constant = sample_target(PortableTargetKind::Constant, TargetVisibility::Public);
    let profile = ConformanceTargetProfile::from_target(&constant);
    let decision = policy.evaluate(&profile);
    assert_eq!(decision.state, ApplicabilityState::NotApplicable);

    // Unrepresented inventory returns Pending
    let mut unrep = callable;
    unrep.inventory = InventoryState::Pending;
    let profile = ConformanceTargetProfile::from_target(&unrep);
    let decision = policy.evaluate(&profile);
    assert_eq!(decision.state, ApplicabilityState::Pending);

    // Container-only policy
    let container_policy = ConformanceApplicabilityPolicy::workspace_containers().unwrap();
    let pkg = sample_target(PortableTargetKind::Package, TargetVisibility::Public);
    let profile = ConformanceTargetProfile::from_target(&pkg);
    assert_eq!(
        container_policy.evaluate(&profile).state,
        ApplicabilityState::Applicable
    );

    let callable_profile = ConformanceTargetProfile::from_target(&sample_target(
        PortableTargetKind::Callable,
        TargetVisibility::Public,
    ));
    assert_eq!(
        container_policy.evaluate(&callable_profile).state,
        ApplicabilityState::NotApplicable
    );
}

#[test]
fn assessment_draft_binding_and_validation() {
    let target = sample_target(PortableTargetKind::Type, TargetVisibility::Public);
    let profile = ConformanceTargetProfile::from_target(&target);

    let eid_1 = EvidenceId::derive([b"evidence-1".as_slice()]);
    let eid_2 = EvidenceId::derive([b"evidence-2".as_slice()]);

    let mut evidence_locations = BTreeMap::new();
    evidence_locations.insert(
        eid_1.clone(),
        Some(SourceLocation {
            path: SourcePath::new("crates/test/src/item.rs").unwrap(),
            bytes: ByteSpan::new(10, 50).unwrap(),
            start: None,
            end: None,
        }),
    );
    evidence_locations.insert(eid_2.clone(), None);

    let artifact_id = DesignArtifactId::derive([b"adr-0001".as_slice()]);

    let draft = ConformanceAssessmentDraft {
        dimensions: ALL_CONFORMANCE_DIMENSIONS
            .iter()
            .map(|dim| ConformanceDimensionDraft {
                dimension: *dim,
                status: if *dim == ConformanceDimension::ConstraintConformance {
                    ConformanceDimensionStatus::Deficient
                } else {
                    ConformanceDimensionStatus::Satisfied
                },
                rationale: "Evaluated according to design criteria".to_owned(),
                evidence: vec![eid_1.clone()],
            })
            .collect(),
        result: ConformanceResultDraft::CandidateFindings {
            findings: vec![
                ConformanceCandidateDraft {
                    title: "Violation of mandatory storage abstraction".to_owned(),
                    description: "Direct disk I/O bypasses the redb transaction layer".to_owned(),
                    severity: Severity::High,
                    confidence_basis_points: 9_000,
                    dimensions: BTreeSet::from([ConformanceDimension::ConstraintConformance]),
                    governing_artifacts: vec![artifact_id.clone()],
                    declared_intent: "All persistent data must be committed through redb transactions".to_owned(),
                    observed_implementation: "Direct std::fs::write call found in storage path".to_owned(),
                    discrepancy: "Implementation bypasses transactional safety boundary".to_owned(),
                    impact: "Potential uncommitted partial writes on crash".to_owned(),
                    suggested_disposition: ConformanceDisposition::FixImplementation,
                    evidence: vec![eid_1.clone()],
                },
                ConformanceCandidateDraft {
                    title: "Architectural evolution diverging from ADR 0005".to_owned(),
                    description: "New direct wire transport introduced for telemetry".to_owned(),
                    severity: Severity::Medium,
                    confidence_basis_points: 8_500,
                    dimensions: BTreeSet::from([ConformanceDimension::ArchitecturalDrift]),
                    governing_artifacts: vec![artifact_id],
                    declared_intent: "All wire transports use Langchart adapter contract".to_owned(),
                    observed_implementation: "Telemetry uses custom gRPC channel for throughput".to_owned(),
                    discrepancy: "Telemetry channel is not wrapped in LlmAdapter".to_owned(),
                    impact: "Telemetry traffic bypasses Langchart budget and quota tracking".to_owned(),
                    suggested_disposition: ConformanceDisposition::CreateSupersedingAdr,
                    evidence: vec![eid_2.clone()],
                },
            ],
        },
    };

    let assessment = draft
        .bind(
            WorkItemId::derive([b"work-item-1".as_slice()]),
            profile,
            PolicyId::derive([b"conformance-policy".as_slice()]),
            "conformance-v1".to_owned(),
            ApplicabilityState::Applicable,
            1,
            &evidence_locations,
        )
        .unwrap();

    assert_eq!(
        assessment.schema_version,
        CONFORMANCE_ASSESSMENT_SCHEMA_VERSION
    );
    assert_eq!(assessment.dimensions.len(), 5);

    match &assessment.result {
        ConformanceResult::CandidateFindings { findings } => {
            assert_eq!(findings.len(), 2);
            assert_eq!(findings[0].severity, Severity::High);
            assert_eq!(findings[0].confidence, Confidence::from_basis_points(9_000).unwrap());
            assert_eq!(
                findings[0].suggested_disposition,
                ConformanceDisposition::FixImplementation
            );
            assert_eq!(findings[0].citations.len(), 1);
            assert!(findings[0].citations[0].location.is_some());

            assert_eq!(
                findings[1].suggested_disposition,
                ConformanceDisposition::CreateSupersedingAdr
            );
            assert_eq!(findings[1].citations.len(), 1);
            assert!(findings[1].citations[0].location.is_none());
        }
        _ => panic!("expected CandidateFindings"),
    }

    // Validation passes
    assert!(assessment.validate().is_ok());
}

#[test]
fn serialization_roundtrip() {
    let target = sample_target(PortableTargetKind::Module, TargetVisibility::Public);
    let profile = ConformanceTargetProfile::from_target(&target);

    let assessment = ConformanceAssessment {
        schema_version: CONFORMANCE_ASSESSMENT_SCHEMA_VERSION,
        work_item: WorkItemId::derive([b"wi".as_slice()]),
        target: profile,
        policy: PolicyId::derive([b"pol".as_slice()]),
        policy_version: "v1".to_owned(),
        applicability: ApplicabilityState::Applicable,
        evidence_revision: 1,
        dimensions: vec![ConformanceDimensionResult {
            dimension: ConformanceDimension::Coverage,
            status: ConformanceDimensionStatus::Satisfied,
            rationale: "All requirements met".to_owned(),
            citations: Vec::new(),
        }],
        result: ConformanceResult::Passed,
    };

    let serialized = serde_json::to_string(&assessment).unwrap();
    let deserialized: ConformanceAssessment = serde_json::from_str(&serialized).unwrap();
    assert_eq!(assessment, deserialized);
}

#[test]
fn validation_rejects_empty_fields() {
    let target = sample_target(PortableTargetKind::Type, TargetVisibility::Public);
    let profile = ConformanceTargetProfile::from_target(&target);

    let invalid_assessment = ConformanceAssessment {
        schema_version: CONFORMANCE_ASSESSMENT_SCHEMA_VERSION,
        work_item: WorkItemId::derive([b"wi".as_slice()]),
        target: profile,
        policy: PolicyId::derive([b"pol".as_slice()]),
        policy_version: String::new(), // Invalid: empty string
        applicability: ApplicabilityState::Applicable,
        evidence_revision: 1,
        dimensions: Vec::new(), // Invalid: empty dimensions
        result: ConformanceResult::Passed,
    };

    assert!(invalid_assessment.validate().is_err());
}
