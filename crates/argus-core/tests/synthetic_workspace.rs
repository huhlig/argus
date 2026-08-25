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
    AdjudicationState, ApplicabilityState, Assessment, AssessmentId, AssessmentState, Attempt,
    AttemptId, AuditModel, Capability, CapabilityStatus, ExecutionState, InventoryState, PolicyId,
    PortableTargetKind, Target, TargetId, TargetKind, VerificationState, WorkItem, WorkItemId,
};

fn target(name: &str, kind: PortableTargetKind, inventory: InventoryState) -> Target {
    Target {
        id: TargetId::derive([name.as_bytes()]),
        kind: TargetKind::Portable { kind },
        visibility: argus_core::TargetVisibility::Unknown,
        name: name.to_owned(),
        parent: None,
        location: None,
        inventory,
        capabilities: vec![],
        diagnostic: matches!(
            inventory,
            InventoryState::Failed | InventoryState::Unsupported
        )
        .then(|| "synthetic discovery diagnostic".to_owned()),
    }
}

#[test]
fn multi_package_partial_and_failed_inventory_balances() {
    let mut model = AuditModel::default();
    let package_a = target(
        "package-a",
        PortableTargetKind::Package,
        InventoryState::Represented,
    );
    let mut package_b = target(
        "package-b",
        PortableTargetKind::Package,
        InventoryState::Represented,
    );
    package_b.capabilities.push(Capability {
        name: "semantics".to_owned(),
        status: CapabilityStatus::Partial,
        detail: Some("one generated declaration unresolved".to_owned()),
        provider: Some("synthetic@1".to_owned()),
    });
    let excluded = target(
        "vendor",
        PortableTargetKind::Module,
        InventoryState::Excluded,
    );
    let failed = target(
        "malformed",
        PortableTargetKind::File,
        InventoryState::Failed,
    );

    for item in [package_a, package_b, excluded, failed] {
        model.insert_target(item).unwrap();
    }

    let coverage = model.inventory_coverage();
    assert_eq!(coverage.total(), 4);
    assert_eq!(coverage.represented, 2);
    assert_eq!(coverage.excluded, 1);
    assert_eq!(coverage.failed, 1);
    assert!(
        model.targets[&TargetId::derive([b"package-b".as_slice()])].capabilities[0]
            .validate()
            .is_ok()
    );
}

#[test]
fn attempts_append_but_effective_assessment_is_unique() {
    let mut model = AuditModel::default();
    let target = target(
        "callable",
        PortableTargetKind::Callable,
        InventoryState::Represented,
    );
    let target_id = target.id.clone();
    model.insert_target(target).unwrap();

    let policy_id = PolicyId::derive([b"documentation".as_slice()]);
    let work_id = WorkItemId::derive([b"callable-documentation".as_slice()]);
    model
        .insert_work_item(WorkItem {
            id: work_id.clone(),
            target: target_id,
            policy: policy_id,
            applicability: ApplicabilityState::Applicable,
        })
        .unwrap();

    let failed_id = AttemptId::derive([b"attempt-1".as_slice()]);
    model
        .append_attempt(Attempt {
            id: failed_id,
            work_item: work_id.clone(),
            number: 1,
            execution: ExecutionState::Failed,
            diagnostic: Some("provider unavailable".to_owned()),
        })
        .unwrap();
    let successful_id = AttemptId::derive([b"attempt-2".as_slice()]);
    model
        .append_attempt(Attempt {
            id: successful_id.clone(),
            work_item: work_id.clone(),
            number: 2,
            execution: ExecutionState::Succeeded,
            diagnostic: None,
        })
        .unwrap();

    let assessment = Assessment {
        id: AssessmentId::derive([b"assessment".as_slice()]),
        work_item: work_id,
        attempt: successful_id,
        state: AssessmentState::Passed,
        verification: VerificationState::NotRequested,
        adjudication: AdjudicationState::Unreviewed,
    };
    model
        .record_effective_assessment(assessment.clone())
        .unwrap();
    assert!(model.record_effective_assessment(assessment).is_err());
    assert_eq!(model.attempts.len(), 2);
    assert_eq!(model.effective_assessments.len(), 1);

    let json = serde_json::to_string(&argus_core::Versioned::current(model.clone())).unwrap();
    let decoded: argus_core::Versioned<AuditModel> = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.into_current().unwrap(), model);
}
