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
    ApplicabilityState, ConfigurationId, ContentHash, EvidenceId, EvidenceKind, EvidenceRecord,
    PolicyId, SnapshotId, Target, TargetId, TargetVisibility, WorkItemId,
};
use argus_evidence::{
    CandidateAvailability, ContextArtifact, DataClassification, EvidenceBudget, EvidenceCandidate,
    EvidenceEnvelope, EvidencePackageBuilder, EvidenceStore, PackageArtifact,
    PolicyEvidenceRequirements, ReviewContextBuilder, ReviewContextFrame,
};
use argus_policies::{
    CorrectnessApplicabilityDecision, CorrectnessApplicabilityPolicy, CorrectnessTargetClass,
    CorrectnessTargetProfile,
};
use argus_storage::{CoverageKey, DurableQueue, QueueWork};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION: u32 = 1;
pub const CORRECTNESS_EVIDENCE_PACKAGE_ARTIFACT_KIND: &str = "evidence-package";
pub const CORRECTNESS_REVIEW_CONTEXT_ARTIFACT_KIND: &str = "review-context";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessReviewUnit {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: CorrectnessTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: CorrectnessApplicabilityDecision,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrectnessReviewPlan {
    pub units: Vec<CorrectnessReviewUnit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessReviewAdmission {
    pub schema_version: u32,
    pub unit: CorrectnessReviewUnit,
    pub evidence_package_ref: String,
    pub review_context_ref: String,
}

#[derive(Clone, Debug)]
pub struct CorrectnessReviewBatch {
    pub materializations: Vec<CorrectnessReviewMaterialization>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CorrectnessEvidenceCatalog {
    hashes: BTreeMap<EvidenceId, ContentHash>,
}

impl CorrectnessEvidenceCatalog {
    pub fn ingest(
        store: &EvidenceStore,
        snapshot: &SnapshotId,
        classification: DataClassification,
        records: &[EvidenceRecord],
    ) -> Result<Self, argus_core::ArgusError> {
        let mut hashes = BTreeMap::new();
        for record in records {
            if hashes.contains_key(&record.id) {
                return Err(argus_core::ArgusError::invariant(
                    "correctness evidence catalog contains duplicate IDs",
                ));
            }
            let envelope =
                EvidenceEnvelope::current(snapshot.clone(), classification, record.clone());
            let hash = store.put(&envelope)?;
            hashes.insert(record.id.clone(), hash);
        }
        Ok(Self { hashes })
    }
}

#[derive(Clone, Debug)]
pub struct CorrectnessReviewMaterialization {
    pub unit: CorrectnessReviewUnit,
    pub package: PackageArtifact,
    pub context: ContextArtifact,
    pub contract: Arc<crate::CorrectnessAssessmentContract>,
}

impl CorrectnessReviewMaterialization {
    pub fn restore(
        queue: &DurableQueue,
        admission: &CorrectnessReviewAdmission,
    ) -> Result<Self, argus_core::ArgusError> {
        if admission.schema_version != CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.schema_version != CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.applicability.state != ApplicabilityState::Applicable
        {
            return Err(argus_core::ArgusError::unsupported(
                "unsupported or inapplicable correctness review admission",
            ));
        }
        let stored_package = load_artifact(
            queue,
            &admission.evidence_package_ref,
            CORRECTNESS_EVIDENCE_PACKAGE_ARTIFACT_KIND,
        )?;
        let package = PackageArtifact {
            hash: stored_package.content_hash,
            package: serde_json::from_slice(&stored_package.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid stored correctness evidence package")
                    .with_source(error)
            })?,
        };
        package.validate_identity()?;
        let stored_context = load_artifact(
            queue,
            &admission.review_context_ref,
            CORRECTNESS_REVIEW_CONTEXT_ARTIFACT_KIND,
        )?;
        let frame: ReviewContextFrame =
            serde_json::from_slice(&stored_context.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid stored correctness review context")
                    .with_source(error)
            })?;
        let context = ContextArtifact {
            hash: stored_context.content_hash,
            frame,
            canonical_json: stored_context.payload,
        };
        validate_restored_identity(&admission.unit, &package, &context)?;
        let contract = Arc::new(crate::CorrectnessAssessmentContract::from_context(
            admission.unit.work_item.clone(),
            admission.unit.target.clone(),
            admission.unit.applicability.state,
            &context.frame,
        )?);
        Ok(Self {
            unit: admission.unit.clone(),
            package,
            context,
            contract,
        })
    }

    pub fn initialize_workflow_data(
        &self,
        store: &crate::WorkflowDataStore,
        langchart_run_id: &str,
    ) -> Result<crate::WorkflowDataWrite, crate::WorkflowDataError> {
        store.create(
            langchart_run_id,
            crate::ReviewWorkflowData {
                work_id: self.unit.work_item.clone(),
                review_unit_id: self.unit.target.target.to_string(),
                policy_id: self.unit.policy.clone(),
                evidence_package_ref: self.package.hash.as_str().to_owned(),
                evidence_revision: self.package.package.revision,
                primary_decisions: Vec::new(),
                candidate_findings: Vec::new(),
                scheduled_verification_work: Vec::new(),
                verification_results: Vec::new(),
                evidence_request_decisions: Vec::new(),
                evidence_expansions: Vec::new(),
                escalation_count: 0,
                evidence_expansion_count: 0,
                adjudication: None,
            },
        )
    }
}

impl CorrectnessReviewUnit {
    pub fn materialize(
        &self,
        store: &EvidenceStore,
        catalog: &CorrectnessEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<CorrectnessReviewMaterialization, argus_core::ArgusError> {
        if self.schema_version != CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported correctness review plan schema version {}",
                self.schema_version
            )));
        }
        if self.applicability.state != ApplicabilityState::Applicable {
            return Err(argus_core::ArgusError::invalid_input(
                "only applicable correctness review units can be materialized",
            ));
        }

        let mut candidates = Vec::with_capacity(self.evidence.len());
        for id in &self.evidence {
            let hash = catalog.hashes.get(id).ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "correctness review unit references uncatalogued evidence",
                )
            })?;
            let stored = store.get(hash)?;
            if stored.envelope.record.id != *id {
                return Err(argus_core::ArgusError::invariant(
                    "correctness evidence catalog identity mismatch",
                ));
            }
            candidates.push(EvidenceCandidate {
                hash: Some(hash.clone()),
                kind: stored.envelope.record.kind,
                priority: 10,
                relation_depth: 0,
                estimated_tokens: stored.canonical_bytes.div_ceil(4),
                availability: CandidateAvailability::Available,
                reason: None,
            });
        }

        let requirements = PolicyEvidenceRequirements {
            allowed_kinds: BTreeSet::from([EvidenceKind::Source]),
            required_kinds: BTreeSet::from([EvidenceKind::Source]),
            maximum_classification,
        };
        let package = EvidencePackageBuilder::new(store).build(
            1,
            snapshot.clone(),
            configuration.clone(),
            self.target.target.clone(),
            self.policy.clone(),
            self.policy_version.clone(),
            budget,
            &requirements,
            candidates,
        )?;
        let context = ReviewContextBuilder::new(store).build(&package)?;
        let contract = Arc::new(crate::CorrectnessAssessmentContract::from_context(
            self.work_item.clone(),
            self.target.clone(),
            self.applicability.state,
            &context.frame,
        )?);
        Ok(CorrectnessReviewMaterialization {
            unit: self.clone(),
            package,
            context,
            contract,
        })
    }
}

impl CorrectnessReviewPlan {
    pub fn materialize_admissible(
        &self,
        store: &EvidenceStore,
        catalog: &CorrectnessEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: &EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<CorrectnessReviewBatch, argus_core::ArgusError> {
        let mut materializations = Vec::new();
        for unit in &self.units {
            if unit.applicability.state != ApplicabilityState::Applicable {
                continue;
            }
            materializations.push(unit.materialize(
                store,
                catalog,
                snapshot,
                configuration,
                budget.clone(),
                maximum_classification,
            )?);
        }
        Ok(CorrectnessReviewBatch { materializations })
    }
}

impl CorrectnessReviewBatch {
    pub fn admit(
        &self,
        queue: &DurableQueue,
        run: &argus_core::RunId,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        adapter: &str,
        at_millis: u64,
    ) -> Result<u64, argus_core::ArgusError> {
        if adapter.trim().is_empty() || adapter.trim() != adapter {
            return Err(argus_core::ArgusError::invalid_input(
                "correctness adapter identity must be normalized",
            ));
        }
        let mut work = Vec::with_capacity(self.materializations.len());
        for materialized in &self.materializations {
            let package_bytes =
                serde_json::to_vec(&materialized.package.package).map_err(|error| {
                    argus_core::ArgusError::invariant(
                        "cannot serialize correctness evidence package",
                    )
                    .with_source(error)
                })?;
            let package =
                queue.store_artifact(CORRECTNESS_EVIDENCE_PACKAGE_ARTIFACT_KIND, &package_bytes)?;
            if package.content_hash != materialized.package.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored correctness evidence package identity mismatch",
                ));
            }
            let context = queue.store_artifact(
                CORRECTNESS_REVIEW_CONTEXT_ARTIFACT_KIND,
                &materialized.context.canonical_json,
            )?;
            if context.content_hash != materialized.context.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored correctness review context identity mismatch",
                ));
            }
            let admission = CorrectnessReviewAdmission {
                schema_version: CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION,
                unit: materialized.unit.clone(),
                evidence_package_ref: package.reference,
                review_context_ref: context.reference,
            };
            let payload = serde_json::to_vec(&admission).map_err(|error| {
                argus_core::ArgusError::invariant(
                    "cannot serialize correctness review admission payload",
                )
                .with_source(error)
            })?;
            work.push(QueueWork::pending_for(
                materialized.unit.work_item.clone(),
                payload,
                run.clone(),
                CoverageKey {
                    snapshot: snapshot.to_string(),
                    configuration: configuration.to_string(),
                    adapter: adapter.to_owned(),
                    target_kind: target_class_label(materialized.unit.target.class),
                    policy: materialized.unit.policy_version.clone(),
                },
            ));
        }
        queue.admit_batch(&work, at_millis)
    }
}

pub struct CorrectnessReviewPlanner<'a> {
    policy: &'a CorrectnessApplicabilityPolicy,
    policy_id: PolicyId,
    policy_version: String,
}

impl<'a> CorrectnessReviewPlanner<'a> {
    pub fn new(
        policy: &'a CorrectnessApplicabilityPolicy,
        policy_id: PolicyId,
        policy_version: impl Into<String>,
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.into();
        if policy_version.trim().is_empty() || policy_version.trim() != policy_version {
            return Err(argus_core::ArgusError::invalid_input(
                "correctness policy version must be normalized",
            ));
        }
        Ok(Self {
            policy,
            policy_id,
            policy_version,
        })
    }

    pub fn plan(
        &self,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        targets: &[Target],
        evidence: &[EvidenceRecord],
    ) -> Result<CorrectnessReviewPlan, argus_core::ArgusError> {
        let targets_by_id = targets
            .iter()
            .map(|target| (target.id.clone(), target))
            .collect::<BTreeMap<_, _>>();
        if targets_by_id.len() != targets.len() {
            return Err(argus_core::ArgusError::invariant(
                "correctness plan contains duplicate targets",
            ));
        }
        let mut evidence_by_target: BTreeMap<TargetId, Vec<EvidenceId>> = BTreeMap::new();
        for record in evidence {
            record.validate()?;
            if let Some(target) = &record.target {
                if !targets_by_id.contains_key(target) {
                    return Err(argus_core::ArgusError::invariant(
                        "correctness evidence references an unknown target",
                    ));
                }
                evidence_by_target
                    .entry(target.clone())
                    .or_default()
                    .push(record.id.clone());
            }
        }
        for records in evidence_by_target.values_mut() {
            records.sort();
            records.dedup();
        }

        let mut units = Vec::with_capacity(targets.len());
        for target in targets_by_id.values() {
            let mut profile = CorrectnessTargetProfile::from_target(target);
            profile.visibility = effective_visibility(target, &targets_by_id, &mut BTreeSet::new());
            let applicability = self.policy.evaluate(&profile);
            let work_item = WorkItemId::derive([
                b"correctness-review".as_slice(),
                snapshot.as_str().as_bytes(),
                configuration.as_str().as_bytes(),
                profile.target.as_str().as_bytes(),
                self.policy_id.as_str().as_bytes(),
                self.policy_version.as_bytes(),
            ]);
            units.push(CorrectnessReviewUnit {
                schema_version: CORRECTNESS_REVIEW_PLAN_SCHEMA_VERSION,
                work_item,
                target: profile,
                policy: self.policy_id.clone(),
                policy_version: self.policy_version.clone(),
                applicability,
                evidence: evidence_by_target.remove(&target.id).unwrap_or_default(),
            });
        }
        Ok(CorrectnessReviewPlan { units })
    }
}

fn effective_visibility(
    target: &Target,
    targets: &BTreeMap<TargetId, &Target>,
    resolving: &mut BTreeSet<TargetId>,
) -> TargetVisibility {
    if target.visibility != TargetVisibility::Inherited {
        return target.visibility;
    }
    if !resolving.insert(target.id.clone()) {
        return TargetVisibility::Unknown;
    }
    let visibility = target
        .parent
        .as_ref()
        .and_then(|parent| targets.get(parent))
        .map_or(TargetVisibility::Unknown, |parent| {
            effective_visibility(parent, targets, resolving)
        });
    resolving.remove(&target.id);
    visibility
}

fn target_class_label(class: CorrectnessTargetClass) -> String {
    format!("{class:?}").to_lowercase()
}

fn load_artifact(
    queue: &DurableQueue,
    reference: &str,
    kind: &str,
) -> Result<argus_storage::StoredArtifact, argus_core::ArgusError> {
    let artifact = queue.artifact(reference)?.ok_or_else(|| {
        argus_core::ArgusError::invalid_input(format!("artifact `{reference}` is missing"))
    })?;
    if artifact.kind != kind {
        return Err(argus_core::ArgusError::invalid_input(format!(
            "artifact `{reference}` is of kind `{}` instead of `{kind}`",
            artifact.kind
        )));
    }
    Ok(artifact)
}

fn validate_restored_identity(
    unit: &CorrectnessReviewUnit,
    package: &PackageArtifact,
    context: &ContextArtifact,
) -> Result<(), argus_core::ArgusError> {
    let control = &context.frame.trusted_control;
    if package.package.revision != 1
        || package.package.target != unit.target.target
        || package.package.policy != unit.policy
        || package.package.policy_version != unit.policy_version
        || control.snapshot != package.package.snapshot
        || control.target != unit.target.target
        || control.policy != unit.policy
        || control.policy_version != unit.policy_version
        || control.package_hash != package.hash
        || control.package_revision != package.package.revision
        || context.hash != ContentHash::digest(&context.canonical_json)
        || context
            .frame
            .untrusted_evidence
            .iter()
            .any(|item| !unit.evidence.contains(&item.id))
    {
        return Err(argus_core::ArgusError::invariant(
            "restored correctness review identity mismatch",
        ));
    }
    Ok(())
}
