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

//! Short-circuit cache worker for producing review outcomes directly from cached assessments.
//!
//! Evaluates queued review work items against the durable review assessment cache.
//! If an unexpired assessment with an identical review fingerprint exists, the worker
//! creates the effective outcome artifact and receipt directly, marking the work item
//! as succeeded without invoking model providers or launching Langchart actors.

use crate::{
    EffectiveOutcome, LogicalOutcomeKey, OutcomeKind, OutcomeProvenance, OutcomeReceipt,
    OutcomeRecorder, TARGET_REVIEW_WORKFLOW_ID, TARGET_REVIEW_WORKFLOW_VERSION, target_review_hash,
};
use argus_core::{
    ArgusError, PolicyId, ReviewFingerprint, RunId, SnapshotId, TargetId, WorkItemId,
};
use argus_provider::ProviderIdentity;
use argus_storage::{CachedAssessmentRecord, DurableQueue, LeasedWork, ReviewAssessmentCache};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Trait for resolving the active review fingerprint of a target and policy.
pub trait FingerprintResolver: Send + Sync {
    /// Resolves the current review fingerprint for the specified target and policy.
    fn resolve(&self, target: &TargetId, policy: &PolicyId) -> Option<ReviewFingerprint>;
}

impl<F> FingerprintResolver for F
where
    F: Fn(&TargetId, &PolicyId) -> Option<ReviewFingerprint> + Send + Sync,
{
    fn resolve(&self, target: &TargetId, policy: &PolicyId) -> Option<ReviewFingerprint> {
        self(target, policy)
    }
}

/// In-memory map-backed fingerprint resolver.
#[derive(Clone, Debug, Default)]
pub struct MapFingerprintResolver {
    fingerprints: BTreeMap<(TargetId, PolicyId), ReviewFingerprint>,
    target_fingerprints: BTreeMap<TargetId, ReviewFingerprint>,
}

impl MapFingerprintResolver {
    /// Creates an empty fingerprint resolver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a fingerprint keyed by both target and policy.
    pub fn insert(&mut self, fingerprint: ReviewFingerprint) {
        let key = (fingerprint.target.clone(), fingerprint.policy.clone());
        self.target_fingerprints
            .insert(fingerprint.target.clone(), fingerprint.clone());
        self.fingerprints.insert(key, fingerprint);
    }

    /// Extends the resolver with a collection of fingerprints.
    pub fn extend(&mut self, iter: impl IntoIterator<Item = ReviewFingerprint>) {
        for fp in iter {
            self.insert(fp);
        }
    }
}

impl FingerprintResolver for MapFingerprintResolver {
    fn resolve(&self, target: &TargetId, policy: &PolicyId) -> Option<ReviewFingerprint> {
        if let Some(fp) = self.fingerprints.get(&(target.clone(), policy.clone())) {
            Some(fp.clone())
        } else {
            self.target_fingerprints
                .get(target)
                .filter(|fp| fp.policy == *policy)
                .cloned()
        }
    }
}

/// Configuration parameters for the short-circuit cache worker.
#[derive(Clone, Debug)]
pub struct ShortCircuitCacheWorkerConfig {
    /// Audit run identifier owning the work.
    pub audit_run: RunId,
    /// Audit snapshot identifier being reviewed.
    pub audit_snapshot: SnapshotId,
    /// Optional partition adapter name to lease from.
    pub adapter: Option<String>,
    /// Optional partition policy name to lease from.
    pub policy: Option<String>,
    /// Work item lease duration in milliseconds.
    pub lease_duration_millis: u64,
}

/// Result of evaluating a work item with the short-circuit cache worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShortCircuitCacheWorkerResult {
    /// No available work items matching the partition or criteria.
    Idle,
    /// Cache hit: review outcome was generated directly from cached assessment.
    Hit {
        /// Work item resolved.
        work_id: WorkItemId,
        /// Target evaluated.
        target: TargetId,
        /// Policy evaluated.
        policy: PolicyId,
        /// Receipt of the stored outcome.
        outcome_receipt: OutcomeReceipt,
    },
    /// Cache miss: assessment was missing, expired, or fingerprint differed. Lease was released.
    Miss {
        /// Work item evaluated.
        work_id: WorkItemId,
        /// Target evaluated.
        target: TargetId,
        /// Policy evaluated.
        policy: PolicyId,
        /// Reason for miss.
        reason: String,
    },
}

/// Summary report of a multi-item cache short-circuit sweep.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShortCircuitSummary {
    /// Number of cache hits executed.
    pub hits: usize,
    /// Number of cache misses encountered.
    pub misses: usize,
    /// Work item IDs successfully completed via cache hit.
    pub hit_work_ids: Vec<WorkItemId>,
    /// Work item IDs that were misses and released back to queue.
    pub miss_work_ids: Vec<WorkItemId>,
}

/// Worker that scans the work queue and short-circuits evaluation on cache hits.
pub struct ShortCircuitCacheWorker {
    queue: Arc<DurableQueue>,
    cache: Option<Arc<ReviewAssessmentCache>>,
    resolver: Arc<dyn FingerprintResolver>,
    config: ShortCircuitCacheWorkerConfig,
}

impl ShortCircuitCacheWorker {
    /// Creates a new short-circuit cache worker.
    ///
    /// If `cache` is `None`, the worker queries the cache table embedded in `queue`.
    #[must_use]
    pub fn new(
        queue: Arc<DurableQueue>,
        cache: Option<Arc<ReviewAssessmentCache>>,
        resolver: Arc<dyn FingerprintResolver>,
        config: ShortCircuitCacheWorkerConfig,
    ) -> Self {
        Self {
            queue,
            cache,
            resolver,
            config,
        }
    }

    /// Leases the next pending work item and attempts to resolve it via cache hit.
    ///
    /// On hit, the work item transitions to `Succeeded` in `queue` with its effective outcome.
    /// On miss, the lease is immediately released back to `Pending` state for model review.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if database operations fail.
    pub async fn run_next(&self, now_millis: u64) -> Result<ShortCircuitCacheWorkerResult, ArgusError> {
        let queue = self.queue.clone();
        let audit_run = self.config.audit_run.clone();
        let adapter = self.config.adapter.clone();
        let policy = self.config.policy.clone();
        let lease_duration = self.config.lease_duration_millis;

        let leased = tokio::task::spawn_blocking(move || {
            match (&adapter, &policy) {
                (Some(a), Some(p)) => {
                    queue.lease_next_for_partition(now_millis, lease_duration, &audit_run, a, p)
                }
                _ => queue.lease_next(now_millis, lease_duration),
            }
        })
        .await
        .map_err(|err| ArgusError::invariant("cache worker lease task failed").with_source(err))??;

        let Some(leased) = leased else {
            return Ok(ShortCircuitCacheWorkerResult::Idle);
        };

        self.process_leased(&leased, now_millis).await
    }

    /// Performs a non-destructive batch sweep over available work items.
    ///
    /// Automatically tracks misses during the sweep to avoid looping over the same
    /// released work items.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if database operations fail.
    pub async fn run_sweep(
        &self,
        now_millis: u64,
        max_items: usize,
    ) -> Result<ShortCircuitSummary, ArgusError> {
        let mut summary = ShortCircuitSummary::default();
        let mut seen_misses: BTreeSet<WorkItemId> = BTreeSet::new();

        while summary.hits + summary.misses < max_items {
            let queue = self.queue.clone();
            let audit_run = self.config.audit_run.clone();
            let adapter = self.config.adapter.clone();
            let policy = self.config.policy.clone();
            let lease_duration = self.config.lease_duration_millis;
            let excluded = seen_misses.clone();

            let leased = tokio::task::spawn_blocking(move || {
                match (&adapter, &policy) {
                    (Some(a), Some(p)) => queue.lease_next_for_partition_matching(
                        now_millis,
                        lease_duration,
                        &audit_run,
                        a,
                        p,
                        |work| !excluded.contains(&work.id),
                    ),
                    _ => queue.lease_next_for_partition_matching(
                        now_millis,
                        lease_duration,
                        &audit_run,
                        adapter.as_deref().unwrap_or(""),
                        policy.as_deref().unwrap_or(""),
                        |work| !excluded.contains(&work.id),
                    ),
                }
            })
            .await
            .map_err(|err| ArgusError::invariant("cache sweep lease task failed").with_source(err))??;

            let Some(leased) = leased else {
                break;
            };

            match self.process_leased(&leased, now_millis).await? {
                ShortCircuitCacheWorkerResult::Idle => break,
                ShortCircuitCacheWorkerResult::Hit {
                    work_id,
                    outcome_receipt: _,
                    ..
                } => {
                    summary.hits += 1;
                    summary.hit_work_ids.push(work_id);
                }
                ShortCircuitCacheWorkerResult::Miss { work_id, .. } => {
                    seen_misses.insert(work_id.clone());
                    summary.misses += 1;
                    summary.miss_work_ids.push(work_id);
                }
            }
        }

        Ok(summary)
    }

    /// Evaluates an already leased work item for cache hit execution.
    pub async fn process_leased(
        &self,
        leased: &LeasedWork,
        now_millis: u64,
    ) -> Result<ShortCircuitCacheWorkerResult, ArgusError> {
        let Some((target, policy, policy_version)) = parse_work_metadata(&leased.payload) else {
            let queue = self.queue.clone();
            let work_id = leased.id.clone();
            tokio::task::spawn_blocking(move || queue.release_lease(&work_id, now_millis))
                .await
                .map_err(|err| {
                    ArgusError::invariant("release lease task failed").with_source(err)
                })??;
            return Ok(ShortCircuitCacheWorkerResult::Miss {
                work_id: leased.id.clone(),
                target: TargetId::derive([b"unknown".as_slice()]),
                policy: PolicyId::derive([b"unknown".as_slice()]),
                reason: "cannot parse target or policy from work payload".to_owned(),
            });
        };

        let Some(fingerprint) = self.resolver.resolve(&target, &policy) else {
            let queue = self.queue.clone();
            let work_id = leased.id.clone();
            tokio::task::spawn_blocking(move || queue.release_lease(&work_id, now_millis))
                .await
                .map_err(|err| {
                    ArgusError::invariant("release lease task failed").with_source(err)
                })??;
            return Ok(ShortCircuitCacheWorkerResult::Miss {
                work_id: leased.id.clone(),
                target,
                policy,
                reason: "no active fingerprint resolved for target and policy".to_owned(),
            });
        };

        // Query the cache
        let cached_opt = if let Some(cache) = &self.cache {
            cache.get(&target, &policy, &fingerprint, now_millis)?
        } else {
            self.queue
                .get_cached_assessment(&target, &policy, &fingerprint, now_millis)?
        };

        let Some(cached) = cached_opt else {
            let queue = self.queue.clone();
            let work_id = leased.id.clone();
            tokio::task::spawn_blocking(move || queue.release_lease(&work_id, now_millis))
                .await
                .map_err(|err| {
                    ArgusError::invariant("release lease task failed").with_source(err)
                })??;
            return Ok(ShortCircuitCacheWorkerResult::Miss {
                work_id: leased.id.clone(),
                target,
                policy,
                reason: "no matching unexpired assessment in cache".to_owned(),
            });
        };

        // Execute cache hit: create outcome directly
        let receipt = self
            .execute_hit(leased, &cached, &policy_version, now_millis)
            .await?;

        Ok(ShortCircuitCacheWorkerResult::Hit {
            work_id: leased.id.clone(),
            target,
            policy,
            outcome_receipt: receipt,
        })
    }

    async fn execute_hit(
        &self,
        leased: &LeasedWork,
        cached: &CachedAssessmentRecord,
        policy_version: &str,
        now_millis: u64,
    ) -> Result<OutcomeReceipt, ArgusError> {
        let queue = self.queue.clone();
        let cache = self.cache.clone();
        let leased_work_id = leased.id.clone();
        let cached_record = cached.clone();
        let policy_version = policy_version.to_owned();
        let snapshot = self.config.audit_snapshot.clone();
        let run = self.config.audit_run.clone();

        tokio::task::spawn_blocking(move || {
            // 1. Resolve artifact reference
            let artifact_ref = if let Some(first_ref) = cached_record.artifact_references.first() {
                if queue.artifact(first_ref)?.is_some() {
                    first_ref.clone()
                } else {
                    let artifact_kind = assessment_artifact_kind(&cached_record.policy);
                    queue
                        .store_artifact(artifact_kind, &cached_record.payload)?
                        .reference
                }
            } else {
                let artifact_kind = assessment_artifact_kind(&cached_record.policy);
                queue
                    .store_artifact(artifact_kind, &cached_record.payload)?
                    .reference
            };

            // 2. Map outcome kind
            let kind = match cached_record.outcome_kind.to_ascii_lowercase().as_str() {
                "passed" => OutcomeKind::Passed,
                "candidate_findings" | "candidate-findings" | "findings" => {
                    OutcomeKind::CandidateFindings
                }
                "unable_to_verify" | "unable-to-verify" => OutcomeKind::UnableToVerify,
                "suggestion" => OutcomeKind::Suggestion,
                _ => OutcomeKind::Failed,
            };

            // 3. Build effective outcome
            let logical_key = LogicalOutcomeKey {
                audit_snapshot: snapshot,
                audit_run: run,
                work_id: leased_work_id,
                policy_version,
                evidence_revision: 1,
                workflow_hash: target_review_hash(),
            };

            let effective_outcome = EffectiveOutcome {
                logical_key,
                result_ref: artifact_ref,
                kind,
                provenance: OutcomeProvenance {
                    prompt_version: cached_record.fingerprint.prompt_version.clone(),
                    actor_id: "argus.cache-short-circuit".to_owned(),
                    actor_version: "1.0.0".to_owned(),
                    workflow_id: TARGET_REVIEW_WORKFLOW_ID.to_owned(),
                    workflow_version: TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
                    provider: ProviderIdentity {
                        provider: "cached".to_owned(),
                        provider_version: "1".to_owned(),
                        model: cached_record
                            .model_reuse_class
                            .clone()
                            .unwrap_or_else(|| "cached".to_owned()),
                        model_version: "cached".to_owned(),
                    },
                },
            };

            // 4. Record outcome in queue (marks leased work as succeeded)
            let recorder = OutcomeRecorder::new(&queue);
            let receipt = recorder
                .record(&effective_outcome)
                .map_err(|err| ArgusError::invariant(format!("cannot record cached outcome: {err}")))?;

            // 5. Touch cache entry hit stats
            if let Some(cache_engine) = &cache {
                cache_engine.touch(
                    &cached_record.target,
                    &cached_record.policy,
                    &cached_record.fingerprint_hash,
                    now_millis,
                )?;
            } else {
                queue.touch_cached_assessment(
                    &cached_record.target,
                    &cached_record.policy,
                    &cached_record.fingerprint_hash,
                    now_millis,
                )?;
            }

            Ok(receipt)
        })
        .await
        .map_err(|err| ArgusError::invariant("execute hit task failed").with_source(err))?
    }
}

/// Helper mapping a policy ID to its standard assessment artifact kind label.
#[must_use]
pub fn assessment_artifact_kind(policy: &PolicyId) -> &'static str {
    let p = policy.as_str().to_ascii_lowercase();
    if p.contains("correctness") {
        crate::CORRECTNESS_ASSESSMENT_ARTIFACT_KIND
    } else if p.contains("documentation") || p.contains("doc") {
        crate::DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND
    } else if p.contains("architecture") || p.contains("arch") {
        crate::ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND
    } else if p.contains("conformance") {
        crate::CONFORMANCE_ASSESSMENT_ARTIFACT_KIND
    } else if p.contains("maintainability") {
        crate::MAINTAINABILITY_ASSESSMENT_ARTIFACT_KIND
    } else if p.contains("optimization") || p.contains("perf") {
        crate::OPTIMIZATION_ASSESSMENT_ARTIFACT_KIND
    } else {
        "review-assessment.v1"
    }
}

#[derive(Debug, Deserialize)]
struct GenericReviewAdmission {
    unit: GenericReviewUnit,
}

#[derive(Debug, Deserialize)]
struct GenericReviewUnit {
    target: GenericTargetProfile,
    policy: PolicyId,
    policy_version: String,
}

#[derive(Debug, Deserialize)]
struct GenericTargetProfile {
    target: TargetId,
}

fn parse_work_metadata(payload: &[u8]) -> Option<(TargetId, PolicyId, String)> {
    if let Ok(admission) = serde_json::from_slice::<GenericReviewAdmission>(payload) {
        return Some((
            admission.unit.target.target,
            admission.unit.policy,
            admission.unit.policy_version,
        ));
    }
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(payload) {
        let target_str = val
            .get("target")
            .and_then(|v| v.as_str().or_else(|| v.get("target").and_then(|t| t.as_str())));
        let policy_str = val.get("policy").and_then(serde_json::Value::as_str);
        let policy_ver = val
            .get("policy_version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("1.0.0");
        if let (Some(t), Some(p)) = (target_str, policy_str) {
            return Some((
                TargetId::derive([t.as_bytes()]),
                PolicyId::derive([p.as_bytes()]),
                policy_ver.to_owned(),
            ));
        }
    }
    None
}
