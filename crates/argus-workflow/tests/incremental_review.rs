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

//! Integration tests for incremental review planning and short-circuit cache execution.

use argus_core::{
    ConfigurationId, ContentHash, InventoryState, PolicyId, PortableTargetKind,
    ReviewFingerprint, RunId, SnapshotId, Target, TargetId, TargetKind, TargetVisibility,
};
use argus_policies::CorrectnessApplicabilityPolicy;
use argus_storage::{CachedAssessmentRecord, DurableQueue, QueueState, QueueWork, RunRecord, RunState};
use argus_workflow::{
    AdmissionMode, CorrectnessReviewAdmission, CorrectnessReviewPlanner, MapFingerprintResolver,
    ShortCircuitCacheWorker, ShortCircuitCacheWorkerConfig, ShortCircuitCacheWorkerResult,
    TargetAdmissionDecision, WorkAdmissionPlanner, CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION,
};
use std::{collections::BTreeSet, sync::Arc};

fn create_test_target(id_str: &str, name: &str) -> Target {
    Target {
        id: TargetId::derive([b"crate".as_slice(), id_str.as_bytes()]),
        name: name.to_string(),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        },
        visibility: TargetVisibility::Public,
        parent: None,
        location: None,
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    }
}

fn fixture_fingerprint(
    target: &TargetId,
    policy: &PolicyId,
    impl_byte: u8,
) -> ReviewFingerprint {
    ReviewFingerprint {
        snapshot: SnapshotId::derive([b"snap-1".as_slice()]),
        target: target.clone(),
        policy: policy.clone(),
        target_hash: ContentHash::digest(b"fn target() -> bool"),
        implementation_hash: ContentHash::digest(&[impl_byte]),
        documentation_hash: ContentHash::digest(b"/// Doc"),
        downstream_dependency_hash: ContentHash::digest(b"deps"),
        upstream_call_structure_hash: ContentHash::digest(b"callers"),
        call_tree_behavior_hash: ContentHash::digest(b"contracts"),
        test_hash: ContentHash::digest(b"tests"),
        design_hash: ContentHash::digest(b"design"),
        policy_version: "1.0.0".to_owned(),
        prompt_version: "correctness@1".to_owned(),
        workflow_hash: ContentHash::digest(b"target_review_v1"),
        actor_versions: vec!["review_actor@1.0".to_owned()],
        model_reuse_class: "frontier-tier".to_owned(),
        evidence_builder_version: "1.0.0".to_owned(),
        extension_versions: vec!["rust@1.0".to_owned()],
        toolchain_hash: ContentHash::digest(b"rustc 1.85"),
    }
}

#[test]
fn admission_planner_full_mode_admits_all_targets() {
    let t1 = create_test_target("t1", "alpha");
    let t2 = create_test_target("t2", "beta");
    let t3 = create_test_target("t3", "gamma");
    let targets = vec![t1.clone(), t2.clone(), t3.clone()];

    let planner = WorkAdmissionPlanner::full();
    assert_eq!(*planner.mode(), AdmissionMode::Full);
    assert!(!planner.is_ci_lite());

    let (admitted, stats) = planner.filter_targets(&targets);
    assert_eq!(admitted.len(), 3);
    assert_eq!(stats.total_evaluated, 3);
    assert_eq!(stats.admitted, 3);
    assert_eq!(stats.filtered_out, 0);

    assert_eq!(
        planner.evaluate_target(&t1.id),
        TargetAdmissionDecision::AdmittedFull
    );
    assert!(planner.should_admit_target(&t1.id));
}

#[test]
fn admission_planner_ci_lite_filters_to_directly_changed_and_impacted() {
    let t1 = create_test_target("t1", "modified_func");
    let t2 = create_test_target("t2", "caller_impacted");
    let t3 = create_test_target("t3", "unaffected_module");
    let targets = vec![t1.clone(), t2.clone(), t3.clone()];

    let mut directly_changed = BTreeSet::new();
    directly_changed.insert(t1.id.clone());

    let mut transitively_impacted = BTreeSet::new();
    transitively_impacted.insert(t2.id.clone());

    let planner = WorkAdmissionPlanner::from_affected(
        AdmissionMode::CiLite,
        directly_changed,
        transitively_impacted,
    );

    assert!(planner.is_ci_lite());
    assert_eq!(
        planner.evaluate_target(&t1.id),
        TargetAdmissionDecision::AdmittedDirectlyChanged
    );
    assert_eq!(
        planner.evaluate_target(&t2.id),
        TargetAdmissionDecision::AdmittedTransitivelyImpacted
    );
    assert_eq!(
        planner.evaluate_target(&t3.id),
        TargetAdmissionDecision::FilteredUnaffectedCiLite
    );

    assert!(planner.should_admit_target(&t1.id));
    assert!(planner.should_admit_target(&t2.id));
    assert!(!planner.should_admit_target(&t3.id));

    let (admitted, stats) = planner.filter_targets(&targets);
    assert_eq!(admitted.len(), 2);
    assert_eq!(stats.total_evaluated, 3);
    assert_eq!(stats.admitted, 2);
    assert_eq!(stats.filtered_out, 1);
    assert_eq!(stats.directly_changed, 1);
    assert_eq!(stats.transitively_impacted, 1);
    assert_eq!(admitted[0].id, t1.id);
    assert_eq!(admitted[1].id, t2.id);
}

#[test]
fn admission_planner_filters_correctness_plan() {
    let t1 = create_test_target("t1", "changed");
    let t2 = create_test_target("t2", "unaffected");

    let snapshot = SnapshotId::derive([b"snap-1".as_slice()]);
    let configuration = ConfigurationId::derive([b"cfg-1".as_slice()]);
    let policy_id = PolicyId::derive([b"policy-correctness".as_slice()]);

    let policy = CorrectnessApplicabilityPolicy::conservative().unwrap();
    let correctness_planner = CorrectnessReviewPlanner::new(&policy, policy_id, "1.0.0").unwrap();
    let plan = correctness_planner
        .plan(&snapshot, &configuration, &[t1.clone(), t2.clone()], &[])
        .unwrap();

    assert_eq!(plan.units.len(), 2);

    let directly_changed = BTreeSet::from([t1.id.clone()]);
    let admission_planner = WorkAdmissionPlanner::from_affected(
        AdmissionMode::CiLite,
        directly_changed,
        BTreeSet::new(),
    );

    let (filtered_plan, stats) = admission_planner.filter_correctness_plan(plan);
    assert_eq!(filtered_plan.units.len(), 1);
    assert_eq!(filtered_plan.units[0].target.target, t1.id);
    assert_eq!(stats.admitted, 1);
    assert_eq!(stats.filtered_out, 1);
}

#[tokio::test]
async fn cache_worker_executes_cache_hit_without_model_invocation() {
    let temporary = tempfile::tempdir().unwrap();
    let queue_path = temporary.path().join("queue.redb");
    let queue = Arc::new(DurableQueue::open(&queue_path).unwrap());

    let snapshot = SnapshotId::derive([b"snap-1".as_slice()]);
    let configuration = ConfigurationId::derive([b"cfg-1".as_slice()]);
    let run = RunId::derive([b"run-1".as_slice()]);

    queue
        .create_run(&RunRecord {
            id: run.clone(),
            snapshot: snapshot.clone(),
            configuration: configuration.clone(),
            state: RunState::Active,
            created_at_millis: 1,
            updated_at_millis: 1,
            finalized_at_millis: None,
        })
        .unwrap();

    let target_struct = create_test_target("calc-add", "add");
    let target = target_struct.id.clone();
    let policy = PolicyId::derive([b"correctness".as_slice()]);
    let fp = fixture_fingerprint(&target, &policy, 42);

    // 1. Seed assessment into durable cache
    let cached_record = CachedAssessmentRecord::new(
        target.clone(),
        policy.clone(),
        fp.clone(),
        b"{\"status\":\"passed\",\"notes\":\"all assertions hold\"}".to_vec(),
        "passed",
        1_000,
    )
    .with_summary("Cached correctness passed");

    queue.insert_cached_assessment(&cached_record).unwrap();
    assert_eq!(queue.cached_assessment_count().unwrap(), 1);

    // 2. Queue a review work item matching this target and policy
    let correctness_policy = CorrectnessApplicabilityPolicy::conservative().unwrap();
    let plan = CorrectnessReviewPlanner::new(&correctness_policy, policy.clone(), "1.0.0")
        .unwrap()
        .plan(&snapshot, &configuration, &[target_struct], &[])
        .unwrap();

    let unit = plan.units.into_iter().next().unwrap();
    let work_id = unit.work_item.clone();

    let admission = CorrectnessReviewAdmission {
        schema_version: CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION,
        unit,
        evidence_package_ref: "artifact:pkg:1".to_owned(),
        review_context_ref: "artifact:ctx:1".to_owned(),
    };
    let payload = serde_json::to_vec(&admission).unwrap();

    queue
        .admit(&QueueWork::pending_for(
            work_id.clone(),
            payload,
            run.clone(),
            argus_storage::CoverageKey {
                snapshot: snapshot.to_string(),
                configuration: configuration.to_string(),
                adapter: "fast-adapter".to_owned(),
                target_kind: "function".to_owned(),
                policy: "1.0.0".to_owned(),
            },
        ))
        .unwrap();

    // 3. Configure short-circuit cache worker with matching fingerprint resolver
    let mut resolver = MapFingerprintResolver::new();
    resolver.insert(fp);

    let worker = ShortCircuitCacheWorker::new(
        queue.clone(),
        None,
        Arc::new(resolver),
        ShortCircuitCacheWorkerConfig {
            audit_run: run.clone(),
            audit_snapshot: snapshot.clone(),
            adapter: Some("fast-adapter".to_owned()),
            policy: Some("1.0.0".to_owned()),
            lease_duration_millis: 10_000,
        },
    );

    // 4. Run next work item: should execute cache hit
    let result = worker.run_next(2_000).await.unwrap();
    match result {
        ShortCircuitCacheWorkerResult::Hit {
            work_id: hit_id,
            target: hit_target,
            policy: hit_policy,
            outcome_receipt,
        } => {
            assert_eq!(hit_id, work_id);
            assert_eq!(hit_target, target);
            assert_eq!(hit_policy, policy);
            assert!(outcome_receipt.outcome.result_ref.starts_with("artifact:"));
        }
        other => panic!("expected CacheHit, got {other:?}"),
    }

    // 5. Verify work state in queue transitioned to Succeeded
    let stored_work = queue.get(&work_id).unwrap().unwrap();
    assert_eq!(stored_work.state, QueueState::Succeeded);
    assert_eq!(stored_work.attempt_count, 1);

    // 6. Verify cache entry was touched
    let stats = queue.cached_assessment_stats(2_500).unwrap();
    assert_eq!(stats.total_hits, 1);
}

#[tokio::test]
async fn cache_worker_releases_lease_on_cache_miss() {
    let temporary = tempfile::tempdir().unwrap();
    let queue_path = temporary.path().join("queue.redb");
    let queue = Arc::new(DurableQueue::open(&queue_path).unwrap());

    let snapshot = SnapshotId::derive([b"snap-1".as_slice()]);
    let configuration = ConfigurationId::derive([b"cfg-1".as_slice()]);
    let run = RunId::derive([b"run-1".as_slice()]);

    queue
        .create_run(&RunRecord {
            id: run.clone(),
            snapshot: snapshot.clone(),
            configuration: configuration.clone(),
            state: RunState::Active,
            created_at_millis: 1,
            updated_at_millis: 1,
            finalized_at_millis: None,
        })
        .unwrap();

    let target_struct = create_test_target("calc-sub", "sub");
    let target = target_struct.id.clone();
    let policy = PolicyId::derive([b"correctness".as_slice()]);

    // Cached record exists with implementation byte 1
    let fp_old = fixture_fingerprint(&target, &policy, 1);
    let cached_record = CachedAssessmentRecord::new(
        target.clone(),
        policy.clone(),
        fp_old,
        b"{\"status\":\"passed\"}".to_vec(),
        "passed",
        1_000,
    );
    queue.insert_cached_assessment(&cached_record).unwrap();

    // Queue work
    let correctness_policy = CorrectnessApplicabilityPolicy::conservative().unwrap();
    let plan = CorrectnessReviewPlanner::new(&correctness_policy, policy.clone(), "1.0.0")
        .unwrap()
        .plan(&snapshot, &configuration, &[target_struct], &[])
        .unwrap();

    let unit = plan.units.into_iter().next().unwrap();
    let work_id = unit.work_item.clone();

    let admission = CorrectnessReviewAdmission {
        schema_version: CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION,
        unit,
        evidence_package_ref: "artifact:pkg:1".to_owned(),
        review_context_ref: "artifact:ctx:1".to_owned(),
    };
    let payload = serde_json::to_vec(&admission).unwrap();

    queue
        .admit(&QueueWork::pending_for(
            work_id.clone(),
            payload,
            run.clone(),
            argus_storage::CoverageKey {
                snapshot: snapshot.to_string(),
                configuration: configuration.to_string(),
                adapter: "fast-adapter".to_owned(),
                target_kind: "function".to_owned(),
                policy: "1.0.0".to_owned(),
            },
        ))
        .unwrap();

    // Current code has changed implementation byte 2 -> fingerprint mismatch
    let fp_new = fixture_fingerprint(&target, &policy, 2);
    let mut resolver = MapFingerprintResolver::new();
    resolver.insert(fp_new);

    let worker = ShortCircuitCacheWorker::new(
        queue.clone(),
        None,
        Arc::new(resolver),
        ShortCircuitCacheWorkerConfig {
            audit_run: run.clone(),
            audit_snapshot: snapshot.clone(),
            adapter: Some("fast-adapter".to_owned()),
            policy: Some("1.0.0".to_owned()),
            lease_duration_millis: 10_000,
        },
    );

    // Run next work item: should encounter miss
    let result = worker.run_next(2_000).await.unwrap();
    match result {
        ShortCircuitCacheWorkerResult::Miss {
            work_id: miss_id,
            target: miss_target,
            policy: miss_policy,
            reason: _,
        } => {
            assert_eq!(miss_id, work_id);
            assert_eq!(miss_target, target);
            assert_eq!(miss_policy, policy);
        }
        other => panic!("expected CacheMiss, got {other:?}"),
    }

    // Verify lease was released back to Pending state so model worker can lease it
    let stored_work = queue.get(&work_id).unwrap().unwrap();
    assert_eq!(stored_work.state, QueueState::Pending);
    assert_eq!(stored_work.lease_until_millis, None);
}

#[tokio::test]
async fn cache_worker_run_sweep_resolves_all_hits_without_looping() {
    let temporary = tempfile::tempdir().unwrap();
    let queue_path = temporary.path().join("queue.redb");
    let queue = Arc::new(DurableQueue::open(&queue_path).unwrap());

    let snapshot = SnapshotId::derive([b"snap-1".as_slice()]);
    let configuration = ConfigurationId::derive([b"cfg-1".as_slice()]);
    let run = RunId::derive([b"run-1".as_slice()]);

    queue
        .create_run(&RunRecord {
            id: run.clone(),
            snapshot: snapshot.clone(),
            configuration: configuration.clone(),
            state: RunState::Active,
            created_at_millis: 1,
            updated_at_millis: 1,
            finalized_at_millis: None,
        })
        .unwrap();

    let t1_struct = create_test_target("item-1", "fn1");
    let t2_struct = create_test_target("item-2", "fn2");
    let t1 = t1_struct.id.clone();
    let t2 = t2_struct.id.clone();
    let policy = PolicyId::derive([b"correctness".as_slice()]);

    let fp1 = fixture_fingerprint(&t1, &policy, 1);
    let fp2 = fixture_fingerprint(&t2, &policy, 2);

    // Only t1 is cached
    let cached1 = CachedAssessmentRecord::new(
        t1.clone(),
        policy.clone(),
        fp1.clone(),
        b"{\"status\":\"passed\"}".to_vec(),
        "passed",
        1_000,
    );
    queue.insert_cached_assessment(&cached1).unwrap();

    let correctness_policy = CorrectnessApplicabilityPolicy::conservative().unwrap();
    let plan = CorrectnessReviewPlanner::new(&correctness_policy, policy.clone(), "1.0.0")
        .unwrap()
        .plan(&snapshot, &configuration, &[t1_struct, t2_struct], &[])
        .unwrap();

    let mut work_ids = Vec::new();
    for unit in plan.units {
        let work_id = unit.work_item.clone();
        work_ids.push(work_id.clone());
        let admission = CorrectnessReviewAdmission {
            schema_version: CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION,
            unit,
            evidence_package_ref: "artifact:pkg:1".to_owned(),
            review_context_ref: "artifact:ctx:1".to_owned(),
        };
        let payload = serde_json::to_vec(&admission).unwrap();
        queue
            .admit(&QueueWork::pending_for(
                work_id,
                payload,
                run.clone(),
                argus_storage::CoverageKey {
                    snapshot: snapshot.to_string(),
                    configuration: configuration.to_string(),
                    adapter: "fast-adapter".to_owned(),
                    target_kind: "function".to_owned(),
                    policy: "1.0.0".to_owned(),
                },
            ))
            .unwrap();
    }

    let mut resolver = MapFingerprintResolver::new();
    resolver.insert(fp1);
    resolver.insert(fp2);

    let worker = ShortCircuitCacheWorker::new(
        queue.clone(),
        None,
        Arc::new(resolver),
        ShortCircuitCacheWorkerConfig {
            audit_run: run.clone(),
            audit_snapshot: snapshot.clone(),
            adapter: Some("fast-adapter".to_owned()),
            policy: Some("1.0.0".to_owned()),
            lease_duration_millis: 10_000,
        },
    );

    let summary = worker.run_sweep(2_000, 10).await.unwrap();
    assert_eq!(summary.hits, 1);
    assert_eq!(summary.misses, 1);
    assert_eq!(summary.hit_work_ids.len(), 1);
    assert_eq!(summary.miss_work_ids.len(), 1);

    // Verify hit succeeded, miss remained pending
    assert_eq!(
        queue.get(&summary.hit_work_ids[0]).unwrap().unwrap().state,
        QueueState::Succeeded
    );
    assert_eq!(
        queue.get(&summary.miss_work_ids[0]).unwrap().unwrap().state,
        QueueState::Pending
    );
}
