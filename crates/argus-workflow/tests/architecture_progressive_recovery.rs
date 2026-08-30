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
    ApplicabilityState, ConfigurationId, InventoryState, PolicyId, RunId, SnapshotId, TargetId,
    TargetVisibility, WorkItemId,
};
use argus_policies::{
    ArchitectureApplicabilityDecision, ArchitectureScope, ArchitectureTargetClass,
    ArchitectureTargetProfile,
};
use argus_storage::{CoverageKey, DurableQueue, QueueState, QueueWork, RunRecord, RunState};
use argus_workflow::{
    ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION, ArchitectureReviewAdmission, ArchitectureReviewUnit,
};
use std::collections::BTreeMap;

const POLICY: &str = "architecture-code-derived@1";

fn admission(
    work_item: WorkItemId,
    target: TargetId,
    scope: ArchitectureScope,
    prerequisites: Vec<WorkItemId>,
) -> ArchitectureReviewAdmission {
    ArchitectureReviewAdmission {
        schema_version: ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION,
        unit: ArchitectureReviewUnit {
            schema_version: ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION,
            work_item,
            target: ArchitectureTargetProfile {
                target,
                class: match scope {
                    ArchitectureScope::Module => ArchitectureTargetClass::Module,
                    ArchitectureScope::Package => ArchitectureTargetClass::Package,
                    ArchitectureScope::Workspace => ArchitectureTargetClass::Workspace,
                },
                visibility: TargetVisibility::Private,
                inventory: InventoryState::Represented,
            },
            scope,
            policy: PolicyId::derive([POLICY.as_bytes()]),
            policy_version: POLICY.to_owned(),
            applicability: ArchitectureApplicabilityDecision {
                state: ApplicabilityState::Applicable,
                rationale: "progressive recovery fixture".to_owned(),
            },
            evidence: Vec::new(),
            prerequisite_work: prerequisites,
        },
        evidence_package_ref: "fixture-package".to_owned(),
        review_context_ref: "fixture-context".to_owned(),
    }
}

fn lease_ready_architecture(
    queue: &DurableQueue,
    run: &RunId,
    now: u64,
) -> argus_storage::LeasedWork {
    let states = queue
        .run_records(run)
        .unwrap()
        .work
        .into_iter()
        .map(|work| (work.id, work.state))
        .collect::<BTreeMap<_, _>>();
    queue
        .lease_next_for_partition_matching(now, 100, run, "rust", POLICY, |work| {
            serde_json::from_slice::<ArchitectureReviewAdmission>(&work.payload).is_ok_and(
                |admission| {
                    admission.unit.prerequisite_work.iter().all(|prerequisite| {
                        states.get(prerequisite).is_some_and(|state| {
                            matches!(
                                state,
                                QueueState::Succeeded | QueueState::Failed | QueueState::Cancelled
                            )
                        })
                    })
                },
            )
        })
        .unwrap()
        .unwrap()
}

#[test]
fn failed_and_cancelled_modules_unblock_package_after_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("state.redb");
    let run = RunRecord {
        id: RunId::derive([b"architecture-progressive-recovery".as_slice()]),
        snapshot: SnapshotId::derive([b"snapshot".as_slice()]),
        configuration: ConfigurationId::derive([b"configuration".as_slice()]),
        state: RunState::Active,
        created_at_millis: 0,
        updated_at_millis: 0,
        finalized_at_millis: None,
    };
    let first = WorkItemId::derive([b"module-a".as_slice()]);
    let second = WorkItemId::derive([b"module-b".as_slice()]);
    let package = WorkItemId::derive([b"package".as_slice()]);
    let coverage = CoverageKey {
        snapshot: run.snapshot.to_string(),
        configuration: run.configuration.to_string(),
        adapter: "rust".to_owned(),
        target_kind: "architecture".to_owned(),
        policy: POLICY.to_owned(),
    };

    {
        let queue = DurableQueue::open(&database).unwrap();
        queue.create_run(&run).unwrap();
        let work = [
            admission(
                first.clone(),
                TargetId::derive([b"module-a".as_slice()]),
                ArchitectureScope::Module,
                Vec::new(),
            ),
            admission(
                second.clone(),
                TargetId::derive([b"module-b".as_slice()]),
                ArchitectureScope::Module,
                Vec::new(),
            ),
            admission(
                package.clone(),
                TargetId::derive([b"package".as_slice()]),
                ArchitectureScope::Package,
                vec![first.clone(), second.clone()],
            ),
        ]
        .map(|admission| {
            QueueWork::pending_for(
                admission.unit.work_item.clone(),
                serde_json::to_vec(&admission).unwrap(),
                run.id.clone(),
                coverage.clone(),
            )
        });
        queue.admit_batch(&work, 0).unwrap();
        let leased = lease_ready_architecture(&queue, &run.id, 1);
        queue
            .fail_attempt(&leased.id, 2, "module review failed", 1)
            .unwrap();
    }

    {
        let queue = DurableQueue::open(&database).unwrap();
        let leased = lease_ready_architecture(&queue, &run.id, 3);
        assert_ne!(leased.id, package);
        queue.cancel(&leased.id, 4).unwrap();
    }

    let queue = DurableQueue::open(&database).unwrap();
    let leased = lease_ready_architecture(&queue, &run.id, 5);
    assert_eq!(leased.id, package);
    let records = queue.run_records(&run.id).unwrap();
    assert_eq!(
        records
            .work
            .iter()
            .filter(|work| work.state == QueueState::Failed)
            .count(),
        1
    );
    assert_eq!(
        records
            .work
            .iter()
            .filter(|work| work.state == QueueState::Cancelled)
            .count(),
        1
    );
}
