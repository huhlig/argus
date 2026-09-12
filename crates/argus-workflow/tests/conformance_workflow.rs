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
    ApplicabilityState, Confidence, ConfigurationId, ContentHash, DesignArtifactId, DesignLinkId,
    EvidenceRecord, InventoryState, PolicyId, PortableTargetKind, RunId, SnapshotId, SourcePath,
    Target, TargetId, TargetKind, TargetVisibility, WorkItemId,
};
use argus_evidence::{
    DesignArtifact, DesignArtifactIndex, DesignArtifactKind, DesignLink, DesignLinkKind,
    DesignLinkOrigin, DesignLinkageIndex, DesignStatus,
};
use argus_policies::{
    ALL_CONFORMANCE_DIMENSIONS, CONFORMANCE_ASSESSMENT_SCHEMA_VERSION,
    ConformanceApplicabilityPolicy, ConformanceAssessment, ConformanceDimensionResult,
    ConformanceDimensionStatus, ConformanceResult, ConformanceTargetProfile,
};
use argus_provider::ProviderIdentity;
use argus_storage::DurableQueue;
use argus_workflow::{
    CONFORMANCE_ASSESSMENT_ARTIFACT_KIND, ConformanceOutcomeActor, ConformanceReviewPlanner,
    LogicalOutcomeKey, OutcomeDisposition, OutcomeProvenance, TARGET_REVIEW_WORKFLOW_ID,
    TARGET_REVIEW_WORKFLOW_VERSION, conformance_assessment_draft_schema, target_review_hash,
};
use std::{collections::BTreeSet, sync::Arc};

fn create_test_target(id_str: &str, name: &str) -> Target {
    Target {
        id: TargetId::derive([b"crate".as_slice(), id_str.as_bytes()]),
        name: name.to_string(),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Module,
        },
        visibility: TargetVisibility::Public,
        parent: None,
        location: None,
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    }
}

#[test]
fn conformance_review_planner_links_governing_artifacts_and_evaluates_applicability() {
    let policy = ConformanceApplicabilityPolicy::all_targets().unwrap();
    let policy_id = PolicyId::derive([b"test-policy".as_slice()]);
    let policy_version = "1.0.0".to_string();

    let artifact_id = DesignArtifactId::derive([b"adr-0001".as_slice()]);
    let artifact = DesignArtifact {
        id: artifact_id.clone(),
        path: SourcePath::new("docs/adr/0001-test.md").unwrap(),
        kind: DesignArtifactKind::Adr,
        title: "Test ADR 1".to_string(),
        identifier: Some("ADR-0001".to_string()),
        status: DesignStatus::Accepted,
        date: Some("2026-01-01".to_string()),
        authors: vec!["architect".to_string()],
        deciders: Vec::new(),
        supersedes: Vec::new(),
        superseded_by: None,
        governs: vec!["engine".to_string()],
        tags: BTreeSet::new(),
        sections: Vec::new(),
        content_hash: ContentHash::digest(b"content"),
    };

    let mut artifact_index = DesignArtifactIndex::default();
    artifact_index.insert(artifact);

    let target = create_test_target("target-1", "engine");
    let link = DesignLink {
        id: DesignLinkId::derive([
            target.id.as_str().as_bytes(),
            artifact_id.as_str().as_bytes(),
        ]),
        artifact_id: artifact_id.clone(),
        artifact_identifier: Some("ADR-0001".to_string()),
        section_id: None,
        requirement_id: None,
        target_id: target.id.clone(),
        target_name: target.name.clone(),
        target_path: None,
        origin: DesignLinkOrigin::Explicit {
            source: "governs".to_string(),
        },
        kind: DesignLinkKind::Governs,
        confidence: Confidence::from_basis_points(10_000).unwrap(),
        rationale: "explicit governs clause".to_string(),
    };

    let mut linkage_index = DesignLinkageIndex::default();
    linkage_index.insert(link);

    let planner = ConformanceReviewPlanner::new(
        &policy,
        policy_id.clone(),
        policy_version.clone(),
        &linkage_index,
        &artifact_index,
    )
    .unwrap();

    let snapshot = SnapshotId::derive([b"snapshot".as_slice()]);
    let configuration = ConfigurationId::derive([b"config".as_slice()]);
    let targets = vec![target.clone()];
    let evidence = Vec::<EvidenceRecord>::new();

    let plan = planner
        .plan(&snapshot, &configuration, &targets, &evidence)
        .unwrap();

    assert_eq!(plan.units.len(), 1);
    let unit = &plan.units[0];
    assert_eq!(unit.governing_artifacts, vec![artifact_id]);
    assert_eq!(unit.applicability.state, ApplicabilityState::Applicable);
    assert_eq!(unit.policy, policy_id);
    assert_eq!(unit.policy_version, policy_version);
}

#[test]
fn conformance_outcome_actor_persists_assessment_and_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let queue_path = temporary.path().join("queue.redb");
    let queue = Arc::new(DurableQueue::open(&queue_path).unwrap());

    let target = create_test_target("target-1", "engine");
    let profile = ConformanceTargetProfile::from_target(&target);
    let work_item = WorkItemId::derive([b"work-item".as_slice()]);
    let policy = PolicyId::derive([b"policy".as_slice()]);
    let policy_version = "1.0.0".to_string();

    queue
        .admit(&argus_storage::QueueWork::pending(work_item.clone(), Vec::new()))
        .unwrap();
    queue.lease_next(0, 100).unwrap().unwrap();

    let assessment = ConformanceAssessment {
        schema_version: CONFORMANCE_ASSESSMENT_SCHEMA_VERSION,
        work_item: work_item.clone(),
        target: profile,
        policy: policy.clone(),
        policy_version: policy_version.clone(),
        applicability: ApplicabilityState::Applicable,
        evidence_revision: 1,
        dimensions: ALL_CONFORMANCE_DIMENSIONS
            .into_iter()
            .map(|dim| ConformanceDimensionResult {
                dimension: dim,
                status: ConformanceDimensionStatus::Satisfied,
                rationale: "Dimension fully satisfied".to_string(),
                citations: Vec::new(),
            })
            .collect(),
        result: ConformanceResult::Passed,
    };

    let logical_key = LogicalOutcomeKey {
        audit_snapshot: SnapshotId::derive([b"snapshot".as_slice()]),
        audit_run: RunId::derive([b"run".as_slice()]),
        work_id: work_item,
        policy_version,
        evidence_revision: 1,
        workflow_hash: target_review_hash(),
    };

    let provenance = OutcomeProvenance {
        prompt_version: "conformance-review@1".to_string(),
        actor_id: "argus.review".to_string(),
        actor_version: "1.0.0".to_string(),
        workflow_id: TARGET_REVIEW_WORKFLOW_ID.to_string(),
        workflow_version: TARGET_REVIEW_WORKFLOW_VERSION.to_string(),
        provider: ProviderIdentity {
            provider: "local".to_string(),
            provider_version: "1".to_string(),
            model: "model".to_string(),
            model_version: "1".to_string(),
        },
    };

    let actor = ConformanceOutcomeActor::new(
        queue.clone(),
        assessment,
        logical_key,
        provenance,
    )
    .unwrap();

    let receipt = actor.record().unwrap();
    assert_eq!(receipt.disposition, OutcomeDisposition::Inserted);
    assert!(receipt.outcome.result_ref.starts_with("artifact:"));

    let stored = queue.artifact(&receipt.outcome.result_ref).unwrap().unwrap();
    assert_eq!(stored.kind, CONFORMANCE_ASSESSMENT_ARTIFACT_KIND);
}

#[test]
fn conformance_assessment_draft_schema_is_valid() {
    let schema = conformance_assessment_draft_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["dimensions"].is_object());
    assert!(schema["properties"]["result"].is_object());
}
