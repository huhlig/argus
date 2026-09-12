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

//! Integration tests for durable review assessment caching in `argus-storage`.

use argus_core::{ContentHash, PolicyId, ReviewFingerprint, SnapshotId, TargetId};
use argus_storage::{
    CachedAssessmentRecord, DurableQueue, ReviewAssessmentCache,
    parse_review_assessment_cache_key, review_assessment_cache_key,
};

fn fixture_fingerprint(
    target_str: &str,
    policy_str: &str,
    impl_byte: u8,
) -> ReviewFingerprint {
    ReviewFingerprint {
        snapshot: SnapshotId::derive([b"snapshot-1".as_slice()]),
        target: TargetId::derive([target_str.as_bytes()]),
        policy: PolicyId::derive([policy_str.as_bytes()]),
        target_hash: ContentHash::digest(b"fn target() -> bool"),
        implementation_hash: ContentHash::digest(&[impl_byte]),
        documentation_hash: ContentHash::digest(b"/// Documentation"),
        downstream_dependency_hash: ContentHash::digest(b"dependencies"),
        upstream_call_structure_hash: ContentHash::digest(b"callers"),
        call_tree_behavior_hash: ContentHash::digest(b"contracts"),
        test_hash: ContentHash::digest(b"tests"),
        design_hash: ContentHash::digest(b"design"),
        policy_version: "1.0.0".to_owned(),
        prompt_version: "2.1.0".to_owned(),
        workflow_hash: ContentHash::digest(b"workflow"),
        actor_versions: vec!["actor-1@1.0".to_owned()],
        model_reuse_class: "frontier-tier".to_owned(),
        evidence_builder_version: "1.0.0".to_owned(),
        extension_versions: vec!["rust-analyzer@0.3".to_owned()],
        toolchain_hash: ContentHash::digest(b"rustc 1.85"),
    }
}

#[test]
fn record_validation_enforces_target_policy_and_hash_consistency() {
    let fp = fixture_fingerprint("target-alpha", "policy-docs", 1);
    let valid = CachedAssessmentRecord::new(
        fp.target.clone(),
        fp.policy.clone(),
        fp.clone(),
        b"{\"status\":\"passed\"}".to_vec(),
        "passed",
        1_000,
    );
    assert!(valid.validate().is_ok());

    // Mismatched target
    let mut mismatched_target = valid.clone();
    mismatched_target.target = TargetId::derive([b"other-target".as_slice()]);
    assert!(mismatched_target.validate().is_err());

    // Mismatched policy
    let mut mismatched_policy = valid.clone();
    mismatched_policy.policy = PolicyId::derive([b"other-policy".as_slice()]);
    assert!(mismatched_policy.validate().is_err());

    // Mismatched fingerprint hash
    let mut mismatched_hash = valid.clone();
    mismatched_hash.fingerprint_hash = ContentHash::digest(b"tampered-hash");
    assert!(mismatched_hash.validate().is_err());

    // Empty outcome kind
    let mut empty_kind = valid;
    empty_kind.outcome_kind = "  ".to_owned();
    assert!(empty_kind.validate().is_err());
}

#[test]
fn cache_key_generation_and_parsing_round_trips() {
    let target = TargetId::derive([b"rust:crate::my_module::function".as_slice()]);
    let policy = PolicyId::derive([b"documentation:public-api@2".as_slice()]);
    let hash = ContentHash::digest(b"some-fingerprint-digest");

    let key = review_assessment_cache_key(&target, &policy, &hash);
    let parsed = parse_review_assessment_cache_key(&key).expect("key should parse");

    assert_eq!(parsed.0, target.as_str());
    assert_eq!(parsed.1, policy.as_str());
    assert_eq!(parsed.2, hash.as_str());
}

#[test]
fn standalone_cache_insert_lookup_and_persistence() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_path = temp_dir.path().join("assessment_cache.redb");

    let fp = fixture_fingerprint("target-core", "correctness", 42);
    let record = CachedAssessmentRecord::new(
        fp.target.clone(),
        fp.policy.clone(),
        fp.clone(),
        b"{\"findings\":[]}".to_vec(),
        "passed",
        10_000,
    )
    .with_summary("No defects detected")
    .with_model_reuse_class("frontier-tier");

    {
        let cache = ReviewAssessmentCache::open(&cache_path).unwrap();
        assert_eq!(cache.count().unwrap(), 0);
        cache.insert(&record).unwrap();
        assert_eq!(cache.count().unwrap(), 1);

        let hit = cache
            .get(&fp.target, &fp.policy, &fp, 10_100)
            .unwrap()
            .expect("should find cached assessment");
        assert_eq!(hit.summary.as_deref(), Some("No defects detected"));
        assert_eq!(hit.payload, b"{\"findings\":[]}".as_slice());
        assert_eq!(hit.outcome_kind, "passed");
    }

    // Reopen cache and verify persistence
    {
        let reopened = ReviewAssessmentCache::open(&cache_path).unwrap();
        assert_eq!(reopened.count().unwrap(), 1);

        let hit = reopened
            .get(&fp.target, &fp.policy, &fp, 10_200)
            .unwrap()
            .expect("should persist across reopen");
        assert_eq!(hit.target, fp.target);
        assert_eq!(hit.policy, fp.policy);
    }
}

#[test]
fn cache_hit_vs_miss_on_fingerprint_divergence() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_path = temp_dir.path().join("cache.redb");
    let cache = ReviewAssessmentCache::open(&cache_path).unwrap();

    let fp_baseline = fixture_fingerprint("target-math", "optimization", 1);
    let mut fp_modified = fp_baseline.clone();
    fp_modified.implementation_hash = ContentHash::digest(b"changed-implementation");

    let record = CachedAssessmentRecord::new(
        fp_baseline.target.clone(),
        fp_baseline.policy.clone(),
        fp_baseline.clone(),
        b"cached-optimization-result".to_vec(),
        "passed",
        1000,
    );
    cache.insert(&record).unwrap();

    // Matching fingerprint -> Cache HIT
    assert!(
        cache
            .get(&fp_baseline.target, &fp_baseline.policy, &fp_baseline, 1100)
            .unwrap()
            .is_some()
    );

    // Modified fingerprint -> Cache MISS
    assert!(
        cache
            .get(&fp_modified.target, &fp_modified.policy, &fp_modified, 1100)
            .unwrap()
            .is_none()
    );
}

#[test]
fn ttl_and_timestamp_expiration_pruning() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_path = temp_dir.path().join("cache.redb");
    let cache = ReviewAssessmentCache::open(&cache_path).unwrap();

    let fp1 = fixture_fingerprint("target-expiring", "policy-ttl", 1);
    let fp2 = fixture_fingerprint("target-permanent", "policy-ttl", 2);

    let record_expiring = CachedAssessmentRecord::new(
        fp1.target.clone(),
        fp1.policy.clone(),
        fp1.clone(),
        b"short-lived".to_vec(),
        "passed",
        10_000,
    )
    .with_ttl(2_000); // Expires at 12_000

    let record_permanent = CachedAssessmentRecord::new(
        fp2.target.clone(),
        fp2.policy.clone(),
        fp2.clone(),
        b"permanent".to_vec(),
        "passed",
        10_000,
    ); // Never expires by time

    cache.insert(&record_expiring).unwrap();
    cache.insert(&record_permanent).unwrap();
    assert_eq!(cache.count().unwrap(), 2);

    // At timestamp 11_000: expiring entry is still valid
    assert!(
        cache
            .get(&fp1.target, &fp1.policy, &fp1, 11_000)
            .unwrap()
            .is_some()
    );

    // At timestamp 12_000: expiring entry has expired (lazy lookup returns None)
    assert!(
        cache
            .get(&fp1.target, &fp1.policy, &fp1, 12_000)
            .unwrap()
            .is_none()
    );

    // Permanent entry remains valid
    assert!(
        cache
            .get(&fp2.target, &fp2.policy, &fp2, 12_000)
            .unwrap()
            .is_some()
    );

    // Stats report 1 active, 1 expired
    let stats = cache.stats(12_000).unwrap();
    assert_eq!(stats.total_entries, 2);
    assert_eq!(stats.active_entries, 1);
    assert_eq!(stats.expired_entries, 1);

    // Pruning expired entries removes exactly 1
    let pruned = cache.prune_expired(12_000).unwrap();
    assert_eq!(pruned, 1);
    assert_eq!(cache.count().unwrap(), 1);

    // Only permanent entry remains
    let remaining = cache.all().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].target, fp2.target);
}

#[test]
fn target_and_bulk_invalidation_pruning() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_path = temp_dir.path().join("cache.redb");
    let cache = ReviewAssessmentCache::open(&cache_path).unwrap();

    let fp_a = fixture_fingerprint("target-A", "correctness", 1);
    let fp_b = fixture_fingerprint("target-B", "correctness", 2);
    let fp_c = fixture_fingerprint("target-C", "correctness", 3);

    cache
        .insert(&CachedAssessmentRecord::new(
            fp_a.target.clone(),
            fp_a.policy.clone(),
            fp_a.clone(),
            b"res-a".to_vec(),
            "passed",
            100,
        ))
        .unwrap();

    cache
        .insert(&CachedAssessmentRecord::new(
            fp_b.target.clone(),
            fp_b.policy.clone(),
            fp_b.clone(),
            b"res-b".to_vec(),
            "passed",
            100,
        ))
        .unwrap();

    cache
        .insert(&CachedAssessmentRecord::new(
            fp_c.target.clone(),
            fp_c.policy.clone(),
            fp_c.clone(),
            b"res-c".to_vec(),
            "passed",
            100,
        ))
        .unwrap();

    assert_eq!(cache.count().unwrap(), 3);

    // Invalidate target A individually
    let pruned_a = cache.prune_invalidated_target(&fp_a.target).unwrap();
    assert_eq!(pruned_a, 1);
    assert_eq!(cache.count().unwrap(), 2);
    assert!(
        cache
            .get(&fp_a.target, &fp_a.policy, &fp_a, 200)
            .unwrap()
            .is_none()
    );

    // Bulk invalidate targets B and C in one call (simulating ImpactAnalysis output)
    let bulk_targets = vec![fp_b.target.clone(), fp_c.target.clone()];
    let pruned_bulk = cache.prune_invalidated_targets(&bulk_targets).unwrap();
    assert_eq!(pruned_bulk, 2);
    assert_eq!(cache.count().unwrap(), 0);
}

#[test]
fn target_policy_specific_invalidation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_path = temp_dir.path().join("cache.redb");
    let cache = ReviewAssessmentCache::open(&cache_path).unwrap();

    let fp_docs = fixture_fingerprint("target-shared", "policy-docs", 1);
    let fp_perf = fixture_fingerprint("target-shared", "policy-perf", 1);

    cache
        .insert(&CachedAssessmentRecord::new(
            fp_docs.target.clone(),
            fp_docs.policy.clone(),
            fp_docs.clone(),
            b"docs-out".to_vec(),
            "passed",
            100,
        ))
        .unwrap();

    cache
        .insert(&CachedAssessmentRecord::new(
            fp_perf.target.clone(),
            fp_perf.policy.clone(),
            fp_perf.clone(),
            b"perf-out".to_vec(),
            "passed",
            100,
        ))
        .unwrap();

    assert_eq!(cache.count().unwrap(), 2);

    // Invalidate only docs policy for target-shared
    let pruned = cache
        .prune_invalidated_target_policy(&fp_docs.target, &fp_docs.policy)
        .unwrap();
    assert_eq!(pruned, 1);

    // Docs is gone, Perf remains
    assert!(
        cache
            .get(&fp_docs.target, &fp_docs.policy, &fp_docs, 200)
            .unwrap()
            .is_none()
    );
    assert!(
        cache
            .get(&fp_perf.target, &fp_perf.policy, &fp_perf, 200)
            .unwrap()
            .is_some()
    );
}

#[test]
fn predicate_filter_invalidation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_path = temp_dir.path().join("cache.redb");
    let cache = ReviewAssessmentCache::open(&cache_path).unwrap();

    let mut fp_v1 = fixture_fingerprint("target-p1", "policy-test", 1);
    fp_v1.prompt_version = "v1.0".to_owned();

    let mut fp_v2 = fixture_fingerprint("target-p2", "policy-test", 2);
    fp_v2.prompt_version = "v2.0".to_owned();

    cache
        .insert(&CachedAssessmentRecord::new(
            fp_v1.target.clone(),
            fp_v1.policy.clone(),
            fp_v1.clone(),
            b"out-1".to_vec(),
            "passed",
            100,
        ))
        .unwrap();

    cache
        .insert(&CachedAssessmentRecord::new(
            fp_v2.target.clone(),
            fp_v2.policy.clone(),
            fp_v2.clone(),
            b"out-2".to_vec(),
            "passed",
            100,
        ))
        .unwrap();

    // Prune all assessments generated with prompt version v1.0
    let pruned = cache
        .prune_by_filter(|rec| rec.fingerprint.prompt_version == "v1.0")
        .unwrap();
    assert_eq!(pruned, 1);

    let remaining = cache.all().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].fingerprint.prompt_version, "v2.0");
}

#[test]
fn touch_and_telemetry_stats() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache_path = temp_dir.path().join("cache.redb");
    let cache = ReviewAssessmentCache::open(&cache_path).unwrap();

    let fp = fixture_fingerprint("target-stats", "policy-stats", 5);
    let record = CachedAssessmentRecord::new(
        fp.target.clone(),
        fp.policy.clone(),
        fp.clone(),
        vec![0xAA; 128],
        "passed",
        1_000,
    );
    cache.insert(&record).unwrap();

    let hash = fp.composite_hash();
    assert!(cache.touch(&fp.target, &fp.policy, &hash, 1_500).unwrap());
    assert!(cache.touch(&fp.target, &fp.policy, &hash, 2_000).unwrap());

    let stats = cache.stats(2_500).unwrap();
    assert_eq!(stats.total_entries, 1);
    assert_eq!(stats.active_entries, 1);
    assert_eq!(stats.expired_entries, 0);
    assert_eq!(stats.total_hits, 2);
    assert_eq!(stats.total_payload_bytes, 128);
    assert_eq!(stats.oldest_cached_millis, Some(1_000));
    assert_eq!(stats.newest_cached_millis, Some(1_000));
}

#[test]
fn durable_queue_integrated_cache_operations() {
    let temp_dir = tempfile::tempdir().unwrap();
    let queue_path = temp_dir.path().join("working.redb");
    let queue = DurableQueue::open(&queue_path).unwrap();

    let fp = fixture_fingerprint("queue-target", "queue-policy", 10);
    let record = CachedAssessmentRecord::new(
        fp.target.clone(),
        fp.policy.clone(),
        fp.clone(),
        b"queue-payload".to_vec(),
        "candidate_findings",
        500,
    )
    .with_summary("Potential memory leak detected");

    // Insert via DurableQueue
    queue.insert_cached_assessment(&record).unwrap();
    assert_eq!(queue.cached_assessment_count().unwrap(), 1);

    // Exact lookup via DurableQueue
    let hit = queue
        .get_cached_assessment(&fp.target, &fp.policy, &fp, 600)
        .unwrap()
        .expect("should hit cache in DurableQueue");
    assert_eq!(hit.outcome_kind, "candidate_findings");
    assert_eq!(hit.summary.as_deref(), Some("Potential memory leak detected"));

    // Lookup by hash
    let hash = fp.composite_hash();
    let hash_hit = queue
        .get_cached_assessment_by_hash(&fp.target, &fp.policy, &hash, 600)
        .unwrap()
        .expect("should hit by hash");
    assert_eq!(hash_hit.payload, b"queue-payload".as_slice());

    // Clear via DurableQueue
    let cleared = queue.clear_cached_assessments().unwrap();
    assert_eq!(cleared, 1);
    assert_eq!(queue.cached_assessment_count().unwrap(), 0);
}

#[test]
fn cached_assessment_artifacts_are_protected_from_garbage_collection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let queue_path = temp_dir.path().join("working.redb");
    let queue = DurableQueue::open(&queue_path).unwrap();

    // Store artifact in DurableQueue
    let artifact = queue
        .store_artifact("evidence.v1", b"critical evidence payload")
        .unwrap();

    // Create cached assessment referencing this artifact (not referenced by any outcome)
    let fp = fixture_fingerprint("target-with-artifact", "policy-evidence", 99);
    let record = CachedAssessmentRecord::new(
        fp.target.clone(),
        fp.policy.clone(),
        fp.clone(),
        b"assessment-referencing-artifact".to_vec(),
        "passed",
        1_000,
    )
    .with_artifacts(vec![artifact.reference.clone()]);

    queue.insert_cached_assessment(&record).unwrap();

    // Verify unreferenced artifacts report 0 because the cache references it
    let (unref_count, unref_bytes) = queue.unreferenced_artifacts().unwrap();
    assert_eq!(unref_count, 0);
    assert_eq!(unref_bytes, 0);

    // Prune unreferenced artifacts removes nothing
    let (pruned_count, pruned_bytes) = queue.prune_unreferenced_artifacts().unwrap();
    assert_eq!(pruned_count, 0);
    assert_eq!(pruned_bytes, 0);

    // Artifact remains accessible in queue
    let retrieved = queue
        .artifact(&artifact.reference)
        .unwrap()
        .expect("artifact must be retained");
    assert_eq!(retrieved, artifact);
}
