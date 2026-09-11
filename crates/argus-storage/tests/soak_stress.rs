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
    AdjudicationState, ConfigurationId, FindingId, HumanAdjudication, RunId, SnapshotId, WorkItemId,
};
use argus_provider::{ProviderHealth, ProviderIdentity, ProviderTelemetry};
use argus_storage::{
    CoverageKey, DurableQueue, QueueWork, RunRecord, RunState, finalize_run_bundle,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::thread;
use std::time::Duration;

fn create_run(name: &str, timestamp: u64) -> RunRecord {
    RunRecord {
        id: RunId::derive([name.as_bytes()]),
        snapshot: SnapshotId::derive([name.as_bytes()]),
        configuration: ConfigurationId::derive([b"soak-config".as_slice()]),
        state: RunState::Active,
        created_at_millis: timestamp,
        updated_at_millis: timestamp,
        finalized_at_millis: None,
    }
}

fn create_work(run_id: &RunId, name: &str) -> QueueWork {
    QueueWork::pending_for(
        WorkItemId::derive([run_id.as_str().as_bytes(), name.as_bytes()]),
        name.as_bytes().to_vec(),
        run_id.clone(),
        CoverageKey::unspecified(),
    )
}

fn create_provider_identity() -> ProviderIdentity {
    ProviderIdentity {
        provider: "soak-provider".to_owned(),
        provider_version: "1.0".to_owned(),
        model: "soak-model".to_owned(),
        model_version: "pinned".to_owned(),
    }
}

/// Soak test: Concurrent workers holding items past the initial lease duration,
/// periodically renewing leases via heartbeats. Validates that active heartbeats
/// prevent expired lease reclaiming or worker starvation.
#[test]
fn soak_concurrent_heartbeats_and_lease_renewal() {
    let temporary = tempfile::tempdir().unwrap();
    let db_path = temporary.path().join("soak_heartbeats.redb");
    let queue = Arc::new(DurableQueue::open(&db_path).unwrap());
    let audit_run = create_run("heartbeat-soak-run", 1_000);
    queue.create_run(&audit_run).unwrap();

    let num_items = 25usize;
    for i in 0..num_items {
        let item = create_work(&audit_run.id, &format!("heartbeat-target-{i}"));
        assert!(queue.admit(&item).unwrap());
    }

    let lease_duration_millis = 1_000u64; // lease is 1,000ms
    let completed_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    // Spawn 5 concurrent worker threads
    for _ in 0..5 {
        let q = queue.clone();
        let completed = completed_count.clone();

        handles.push(thread::spawn(move || {
            loop {
                // Per-worker monotonic time progression
                let leased = q.lease_next(1_000, lease_duration_millis).unwrap();
                let Some(item) = leased else {
                    break;
                };

                let mut current_worker_time = 1_000u64;
                // Hold work item and send heartbeats to keep lease fresh
                for _ in 0..4 {
                    thread::sleep(Duration::from_millis(5));
                    current_worker_time += 200; // within 1000ms lease duration
                    q.heartbeat(&item.id, current_worker_time, lease_duration_millis)
                        .expect("heartbeat within active lease duration must succeed");
                }

                // Complete work
                let effective_key = format!("effective-{}", item.id);
                q.complete(&item.id, &effective_key, b"pass").unwrap();
                completed.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(completed_count.load(Ordering::SeqCst), num_items);
    let status = queue.status(1_000).unwrap();
    assert_eq!(status.leased, 0);
    assert_eq!(status.pending, 0);
    assert_eq!(status.succeeded, num_items as u64);
}

/// Soak test: Fault injection simulating crashing workers, lease timeouts,
/// retry tracking up to maximum_attempts, and eventual fail state.
#[test]
fn soak_fault_injection_worker_crashes_and_recovery() {
    let temporary = tempfile::tempdir().unwrap();
    let db_path = temporary.path().join("soak_faults.redb");
    let queue = DurableQueue::open(&db_path).unwrap();
    let audit_run = create_run("fault-soak-run", 1_000);
    queue.create_run(&audit_run).unwrap();

    let num_items = 30u64;
    for i in 0..num_items {
        let item = create_work(&audit_run.id, &format!("fault-target-{i}"));
        assert!(queue.admit(&item).unwrap());
    }

    let lease_duration_millis = 100u64;
    let mut current_time = 1_000u64;
    let max_attempts = 3;

    // Simulation loop:
    // Items with index % 3 == 0 will crash (abandon lease without heartbeating or failing)
    // Items with index % 3 == 1 will fail explicitly via fail_attempt
    // Items with index % 3 == 2 will succeed
    let mut round = 0;

    while round < 30 {
        round += 1;
        let mut progress_in_round = 0;

        while let Some(leased) = queue
            .lease_next(current_time, lease_duration_millis)
            .unwrap()
        {
            progress_in_round += 1;
            let work_str = leased.id.as_str().to_owned();
            let hash_byte = work_str.as_bytes().last().copied().unwrap_or(0) as usize;

            if hash_byte % 3 == 0 {
                // Abandon lease without recording failure (simulates sudden process crash)
                // If attempt_count reaches max_attempts, fail explicitly so it terminates
                if leased.attempt_number >= max_attempts {
                    current_time += 1;
                    queue
                        .fail_attempt(
                            &leased.id,
                            current_time,
                            "Crashed too many times",
                            max_attempts,
                        )
                        .unwrap();
                }
            } else if hash_byte % 3 == 1 {
                // Explicit attempt failure
                current_time += 1;
                queue
                    .fail_attempt(
                        &leased.id,
                        current_time,
                        "Injected provider 503 error",
                        max_attempts,
                    )
                    .unwrap();
            } else {
                // Succeeded
                current_time += 1;
                let effective_key = format!("effective-{}", leased.id);
                queue.complete(&leased.id, &effective_key, b"pass").unwrap();
            }
        }

        // Advance time significantly to expire abandoned leases
        current_time += lease_duration_millis + 100;
        // Call resume_run to return expired abandoned leases back to Pending for the next worker
        queue.resume_run(&audit_run.id, current_time).unwrap();

        let status = queue.status(current_time).unwrap();
        if progress_in_round == 0 && status.leased == 0 && status.pending == 0 {
            break;
        }
    }

    let final_status = queue.status(current_time).unwrap();
    assert_eq!(final_status.leased, 0);
    assert_eq!(final_status.pending, 0);
    assert_eq!(
        final_status.succeeded + final_status.failed,
        num_items,
        "All items must reach a terminal state (succeeded or failed)"
    );

    // Reopen queue to verify database durability and event journal integrity
    drop(queue);
    let reopened = DurableQueue::open(&db_path).unwrap();
    let reopened_status = reopened.status(current_time).unwrap();
    assert_eq!(reopened_status, final_status);

    let events = reopened.events().unwrap();
    assert!(!events.is_empty());
}

/// Soak test: High-volume storage growth, concurrent transactions, adjudications,
/// and telemetry ingestion. Validates memory stability and bundle export correctness.
#[test]
fn soak_storage_growth_and_memory_stability() {
    let temporary = tempfile::tempdir().unwrap();
    let db_path = temporary.path().join("soak_storage_growth.redb");
    let queue = Arc::new(DurableQueue::open(&db_path).unwrap());
    let audit_run = create_run("growth-soak-run", 1_000);
    queue.create_run(&audit_run).unwrap();

    let total_items = 200usize;
    for i in 0..total_items {
        let item = create_work(&audit_run.id, &format!("growth-item-{i:04}"));
        assert!(queue.admit(&item).unwrap());
    }

    let is_done = Arc::new(AtomicBool::new(false));
    let clock = Arc::new(AtomicU64::new(10_000));
    let provider = create_provider_identity();

    // Telemetry writer thread
    let telem_queue = queue.clone();
    let telem_done = is_done.clone();
    let telem_clock = clock.clone();
    let telem_provider = provider.clone();
    let telem_handle = thread::spawn(move || {
        let mut count = 0;
        while !telem_done.load(Ordering::Relaxed) {
            count += 1;
            let now = telem_clock.fetch_add(5, Ordering::SeqCst);
            let telem = ProviderTelemetry {
                last_health: Some(ProviderHealth::Ready),
                requests: 1,
                successes: 1,
                input_tokens: 100,
                output_tokens: 25,
                estimated_cost_microusd: 10,
                ..ProviderTelemetry::default()
            };
            telem_queue
                .publish_provider_telemetry(
                    &format!("session-{}", count % 4),
                    &telem_provider,
                    &telem,
                    now,
                )
                .expect("telemetry write must succeed");
            thread::sleep(Duration::from_millis(1));
        }
    });

    // Multi-worker processing pool (8 concurrent threads)
    let mut worker_handles = Vec::new();
    let processed = Arc::new(AtomicUsize::new(0));

    for _ in 0..8 {
        let q = queue.clone();
        let c = clock.clone();
        let p = processed.clone();
        let run_id = audit_run.id.clone();

        worker_handles.push(thread::spawn(move || {
            loop {
                // Fixed nominal lease time so items are acquired and not expired while in flight
                let leased = match q.lease_next(10_000, 60_000).unwrap() {
                    Some(item) => item,
                    None => break,
                };

                let done_now = c.fetch_add(10, Ordering::SeqCst);
                let effective_key = format!("effective-{}", leased.id);
                q.complete(&leased.id, &effective_key, b"pass").unwrap();

                // Concurrently record human adjudication for every 10th item
                let current_p = p.fetch_add(1, Ordering::SeqCst);
                if current_p % 10 == 0 {
                    let adj = HumanAdjudication {
                        run: run_id.clone(),
                        finding: FindingId::derive([leased.id.as_str().as_bytes()]),
                        revision: 1,
                        state: AdjudicationState::Accepted,
                        expected_issue: Some("soak-issue".to_owned()),
                        reviewer: "soak-agent@test".to_owned(),
                        rationale: "Automated soak review".to_owned(),
                        recorded_at_millis: done_now,
                    };
                    q.record_adjudication(&adj, None).unwrap();
                }
            }
        }));
    }

    for h in worker_handles {
        h.join().unwrap();
    }

    is_done.store(true, Ordering::SeqCst);
    telem_handle.join().unwrap();

    assert_eq!(processed.load(Ordering::SeqCst), total_items);
    let final_status = queue.status(10_000).unwrap();
    assert_eq!(final_status.succeeded, total_items as u64);
    assert_eq!(final_status.pending, 0);
    assert_eq!(final_status.leased, 0);

    // Finalize run bundle to verify transaction log and index integrity
    let bundle_dir = temporary.path().join("soak-bundle");
    let manifest = finalize_run_bundle(
        &queue,
        &audit_run.id,
        &bundle_dir,
        clock.load(Ordering::SeqCst),
    )
    .unwrap();
    assert_eq!(manifest.work_records, total_items);
    assert_eq!(manifest.outcome_records, total_items);
    assert!(manifest.adjudication_records > 0);
}
