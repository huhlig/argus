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
    ApplicabilityState, Capability, CapabilityStatus, ConfigurationId, ContentHash, EvidenceId,
    EvidenceKind, EvidenceOrigin, EvidenceProvenance, EvidenceRecord, InventoryState, PolicyId,
    PortableTargetKind, Relation, ResolutionQuality, SnapshotId, Target, TargetId, TargetKind,
    TargetVisibility, WorkItemId,
};
use argus_evidence::{
    CandidateAvailability, ContextArtifact, DataClassification, EvidenceBudget, EvidenceCandidate,
    EvidenceEnvelope, EvidencePackageBuilder, EvidenceStore, PackageArtifact,
    PolicyEvidenceRequirements, ReviewContextBuilder, ReviewContextFrame,
};
use argus_policies::{
    ArchitectureApplicabilityDecision, ArchitectureApplicabilityPolicy, ArchitectureScope,
    ArchitectureTargetClass, ArchitectureTargetProfile, ConstituentHealthSummary,
};
use argus_storage::{CoverageKey, DurableQueue, QueueWork};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION: u32 = 1;
pub const ARCHITECTURE_SCOPE_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const MAX_SCOPE_CONSTITUENTS: usize = 2_048;
const MAX_SCOPE_INTERNAL_RELATIONS: usize = 4_096;
const MAX_SCOPE_BOUNDARY_RELATIONS: usize = 2_048;
const MAX_SCOPE_EVIDENCE_BYTES: usize = 512 * 1_024;
pub const ARCHITECTURE_EVIDENCE_PACKAGE_ARTIFACT_KIND: &str = "evidence-package";
pub const ARCHITECTURE_REVIEW_CONTEXT_ARTIFACT_KIND: &str = "review-context";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureReviewUnit {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: ArchitectureTargetProfile,
    pub scope: ArchitectureScope,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ArchitectureApplicabilityDecision,
    pub evidence: Vec<EvidenceId>,
    pub prerequisite_work: Vec<WorkItemId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchitectureReviewPlan {
    pub units: Vec<ArchitectureReviewUnit>,
    pub evidence: Vec<EvidenceRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureScopeEvidence {
    pub schema_version: u32,
    pub fingerprint: ContentHash,
    pub target: TargetId,
    pub scope: ArchitectureScope,
    pub constituents: Vec<ArchitectureTargetFact>,
    pub omitted_constituents: usize,
    pub boundary_targets: Vec<ArchitectureTargetFact>,
    pub omitted_boundary_targets: usize,
    pub internal_relations: Vec<ArchitectureRelationFact>,
    pub omitted_internal_relations: usize,
    pub boundary_relations: Vec<ArchitectureRelationFact>,
    pub omitted_boundary_relations: usize,
    pub dependency_cycles: Vec<Vec<TargetId>>,
    pub omitted_dependency_cycles: usize,
    pub constituent_health: ConstituentHealthSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureConstituentEvidence {
    pub schema_version: u32,
    pub target: TargetId,
    pub scope: ArchitectureScope,
    pub constituents: Vec<ArchitectureConstituentAssessmentFact>,
    pub constituent_health: ConstituentHealthSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureConstituentAssessmentFact {
    pub work_item: WorkItemId,
    pub target: TargetId,
    pub scope: ArchitectureScope,
    pub status: ArchitectureConstituentStatus,
    pub summary: String,
    pub candidate_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureConstituentStatus {
    Passed,
    Deficient,
    UnableToVerify,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureTargetFact {
    pub id: TargetId,
    pub name: String,
    pub class: ArchitectureTargetClass,
    pub visibility: TargetVisibility,
    pub inventory: InventoryState,
    pub parent: Option<TargetId>,
    pub capabilities: BTreeMap<String, CapabilityStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureRelationFact {
    pub source: TargetId,
    pub target: TargetId,
    pub kind: String,
    pub resolution: ResolutionQuality,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureReviewAdmission {
    pub schema_version: u32,
    pub unit: ArchitectureReviewUnit,
    pub evidence_package_ref: String,
    pub review_context_ref: String,
}

#[derive(Clone, Debug)]
pub struct ArchitectureReviewBatch {
    pub materializations: Vec<ArchitectureReviewMaterialization>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArchitectureEvidenceCatalog {
    hashes: BTreeMap<EvidenceId, ContentHash>,
}

impl ArchitectureEvidenceCatalog {
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
                    "architecture evidence catalog contains duplicate IDs",
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
pub struct ArchitectureReviewMaterialization {
    pub unit: ArchitectureReviewUnit,
    pub package: PackageArtifact,
    pub context: ContextArtifact,
    pub contract: Arc<crate::ArchitectureAssessmentContract>,
}

impl ArchitectureReviewMaterialization {
    pub fn restore(
        queue: &DurableQueue,
        admission: &ArchitectureReviewAdmission,
    ) -> Result<Self, argus_core::ArgusError> {
        if admission.schema_version != ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.schema_version != ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.applicability.state != ApplicabilityState::Applicable
        {
            return Err(argus_core::ArgusError::unsupported(
                "unsupported or inapplicable architecture review admission",
            ));
        }
        let stored_package = load_artifact(
            queue,
            &admission.evidence_package_ref,
            ARCHITECTURE_EVIDENCE_PACKAGE_ARTIFACT_KIND,
        )?;
        let package = PackageArtifact {
            hash: stored_package.content_hash,
            package: serde_json::from_slice(&stored_package.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input(
                    "invalid stored architecture evidence package",
                )
                .with_source(error)
            })?,
        };
        package.validate_identity()?;
        let stored_context = load_artifact(
            queue,
            &admission.review_context_ref,
            ARCHITECTURE_REVIEW_CONTEXT_ARTIFACT_KIND,
        )?;
        let frame: ReviewContextFrame =
            serde_json::from_slice(&stored_context.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid stored architecture review context")
                    .with_source(error)
            })?;
        let context = ContextArtifact {
            hash: stored_context.content_hash,
            frame,
            canonical_json: stored_context.payload,
        };
        validate_restored_identity(&admission.unit, &package, &context)?;
        let contract = Arc::new(crate::ArchitectureAssessmentContract::from_context(
            admission.unit.work_item.clone(),
            admission.unit.target.clone(),
            admission.unit.scope,
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

impl ArchitectureReviewUnit {
    pub fn materialize(
        &self,
        store: &EvidenceStore,
        catalog: &ArchitectureEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<ArchitectureReviewMaterialization, argus_core::ArgusError> {
        if self.schema_version != ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported architecture review plan schema version {}",
                self.schema_version
            )));
        }
        if self.applicability.state != ApplicabilityState::Applicable {
            return Err(argus_core::ArgusError::invalid_input(
                "only applicable architecture review units can be materialized",
            ));
        }

        let mut candidates = Vec::with_capacity(self.evidence.len());
        for id in &self.evidence {
            let hash = catalog.hashes.get(id).ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "architecture review unit references uncatalogued evidence",
                )
            })?;
            let stored = store.get(hash)?;
            if stored.envelope.record.id != *id {
                return Err(argus_core::ArgusError::invariant(
                    "architecture evidence catalog identity mismatch",
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
            allowed_kinds: BTreeSet::from([
                EvidenceKind::Source,
                EvidenceKind::Documentation,
                EvidenceKind::ArchitectureGraph,
                EvidenceKind::ArchitectureSummary,
            ]),
            required_kinds: BTreeSet::from([EvidenceKind::ArchitectureGraph]),
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
        let contract = Arc::new(crate::ArchitectureAssessmentContract::from_context(
            self.work_item.clone(),
            self.target.clone(),
            self.scope,
            &context.frame,
        )?);
        Ok(ArchitectureReviewMaterialization {
            unit: self.clone(),
            package,
            context,
            contract,
        })
    }
}

impl ArchitectureReviewPlan {
    pub fn materialize_admissible(
        &self,
        store: &EvidenceStore,
        catalog: &ArchitectureEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: &EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<ArchitectureReviewBatch, argus_core::ArgusError> {
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
        Ok(ArchitectureReviewBatch { materializations })
    }
}

impl ArchitectureReviewBatch {
    pub fn admit(
        &self,
        queue: &DurableQueue,
        run: &argus_core::RunId,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        adapter: &str,
        at_millis: u64,
    ) -> Result<u64, argus_core::ArgusError> {
        let mut work = Vec::with_capacity(self.materializations.len());
        for materialized in &self.materializations {
            let package_bytes =
                serde_json::to_vec(&materialized.package.package).map_err(|error| {
                    argus_core::ArgusError::invariant(
                        "cannot serialize architecture evidence package",
                    )
                    .with_source(error)
                })?;
            let package = queue
                .store_artifact(ARCHITECTURE_EVIDENCE_PACKAGE_ARTIFACT_KIND, &package_bytes)?;
            if package.content_hash != materialized.package.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored architecture evidence package identity mismatch",
                ));
            }
            let context = queue.store_artifact(
                ARCHITECTURE_REVIEW_CONTEXT_ARTIFACT_KIND,
                &materialized.context.canonical_json,
            )?;
            if context.content_hash != materialized.context.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored architecture review context identity mismatch",
                ));
            }
            let admission = ArchitectureReviewAdmission {
                schema_version: ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION,
                unit: materialized.unit.clone(),
                evidence_package_ref: package.reference,
                review_context_ref: context.reference,
            };
            let payload = serde_json::to_vec(&admission).map_err(|error| {
                argus_core::ArgusError::invariant(
                    "cannot serialize architecture review admission payload",
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

pub struct ArchitectureReviewPlanner<'a> {
    policy: &'a ArchitectureApplicabilityPolicy,
    policy_id: PolicyId,
    policy_version: String,
    scope_cache: RefCell<BTreeMap<(String, String, String), EvidenceRecord>>,
    persistent_cache: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ArchitectureScopeCacheEntry {
    schema_version: u32,
    target: String,
    configuration: String,
    fingerprint: String,
    record: EvidenceRecord,
}

impl<'a> ArchitectureReviewPlanner<'a> {
    pub fn new(
        policy: &'a ArchitectureApplicabilityPolicy,
        policy_id: PolicyId,
        policy_version: impl Into<String>,
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.into();
        if policy_version.trim().is_empty() || policy_version.trim() != policy_version {
            return Err(argus_core::ArgusError::invalid_input(
                "architecture policy version must be normalized",
            ));
        }
        Ok(Self {
            policy,
            policy_id,
            policy_version,
            scope_cache: RefCell::new(BTreeMap::new()),
            persistent_cache: None,
        })
    }

    pub fn with_persistent_cache(
        mut self,
        directory: impl Into<PathBuf>,
    ) -> Result<Self, argus_core::ArgusError> {
        let directory = directory.into();
        fs::create_dir_all(&directory).map_err(|error| {
            argus_core::ArgusError::invariant("cannot create architecture scope cache")
                .with_source(error)
        })?;
        self.persistent_cache = Some(directory);
        Ok(self)
    }

    pub fn plan(
        &self,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        targets: &[Target],
        evidence: &[EvidenceRecord],
        relations: &[Relation],
    ) -> Result<ArchitectureReviewPlan, argus_core::ArgusError> {
        let normalized_targets = normalize_architecture_targets(targets)?;
        let targets_by_id = normalized_targets
            .iter()
            .map(|target| (target.id.clone(), target))
            .collect::<BTreeMap<_, _>>();
        if targets_by_id.len() != normalized_targets.len() {
            return Err(argus_core::ArgusError::invariant(
                "architecture plan contains duplicate targets",
            ));
        }
        let mut normalized_relations = relations.to_vec();
        normalized_relations.sort_by(|left, right| left.id.cmp(&right.id));
        if normalized_relations
            .windows(2)
            .any(|pair| pair[0].id == pair[1].id)
        {
            return Err(argus_core::ArgusError::invariant(
                "architecture plan contains duplicate relations",
            ));
        }
        for relation in &normalized_relations {
            if !targets_by_id.contains_key(&relation.source)
                || !targets_by_id.contains_key(&relation.target)
            {
                return Err(argus_core::ArgusError::invariant(
                    "architecture relation references an unknown target",
                ));
            }
        }
        let mut evidence_by_target: BTreeMap<TargetId, Vec<EvidenceId>> = BTreeMap::new();
        let mut plan_evidence = Vec::with_capacity(evidence.len() + normalized_targets.len());
        for record in evidence {
            record.validate()?;
            if let Some(target) = &record.target {
                if !targets_by_id.contains_key(target) {
                    return Err(argus_core::ArgusError::invariant(
                        "architecture evidence references an unknown target",
                    ));
                }
                evidence_by_target
                    .entry(target.clone())
                    .or_default()
                    .push(record.id.clone());
            }
            plan_evidence.push(record.clone());
        }
        for records in evidence_by_target.values_mut() {
            records.sort();
            records.dedup();
        }

        let mut units = Vec::with_capacity(targets.len());
        for target in targets_by_id.values() {
            let mut profile = ArchitectureTargetProfile::from_target(target);
            profile.visibility = effective_visibility(target, &targets_by_id, &mut BTreeSet::new());
            let applicability = self.policy.evaluate(&profile);

            let scope = match &target.kind {
                TargetKind::Portable { kind } => match kind {
                    PortableTargetKind::Workspace => ArchitectureScope::Workspace,
                    PortableTargetKind::Package => ArchitectureScope::Package,
                    _ => ArchitectureScope::Module,
                },
                TargetKind::LanguageSpecific { .. } => ArchitectureScope::Module,
            };

            let work_item = WorkItemId::derive([
                b"architecture-review".as_slice(),
                snapshot.as_str().as_bytes(),
                configuration.as_str().as_bytes(),
                profile.target.as_str().as_bytes(),
                self.policy_id.as_str().as_bytes(),
                self.policy_version.as_bytes(),
            ]);
            let mut unit_evidence = evidence_by_target.remove(&target.id).unwrap_or_default();
            if applicability.state == ApplicabilityState::Applicable {
                let structural = synthesize_scope_evidence(
                    target,
                    scope,
                    &targets_by_id,
                    &normalized_relations,
                    configuration,
                    &self.scope_cache,
                    self.persistent_cache.as_deref(),
                )?;
                unit_evidence.push(structural.id.clone());
                plan_evidence.push(structural);
            }
            unit_evidence.sort();
            unit_evidence.dedup();
            units.push(ArchitectureReviewUnit {
                schema_version: ARCHITECTURE_REVIEW_PLAN_SCHEMA_VERSION,
                work_item,
                target: profile,
                scope,
                policy: self.policy_id.clone(),
                policy_version: self.policy_version.clone(),
                applicability,
                evidence: unit_evidence,
                prerequisite_work: Vec::new(),
            });
        }
        assign_progressive_prerequisites(&mut units, &targets_by_id, &normalized_relations);
        plan_evidence.sort_by(|left, right| left.id.cmp(&right.id));
        if plan_evidence
            .windows(2)
            .any(|pair| pair[0].id == pair[1].id)
        {
            return Err(argus_core::ArgusError::invariant(
                "architecture plan contains duplicate evidence IDs",
            ));
        }
        Ok(ArchitectureReviewPlan {
            units,
            evidence: plan_evidence,
        })
    }
}

fn assign_progressive_prerequisites(
    units: &mut [ArchitectureReviewUnit],
    targets: &BTreeMap<TargetId, &Target>,
    relations: &[Relation],
) {
    let applicable = units
        .iter()
        .filter(|unit| unit.applicability.state == ApplicabilityState::Applicable)
        .map(|unit| {
            (
                unit.target.target.clone(),
                (unit.scope, unit.work_item.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for unit in units {
        let scope_targets = scope_target_ids(&unit.target.target, targets, relations);
        let preferred_scope = match unit.scope {
            ArchitectureScope::Module => continue,
            ArchitectureScope::Package => ArchitectureScope::Module,
            ArchitectureScope::Workspace => ArchitectureScope::Package,
        };
        let mut prerequisites = applicable
            .iter()
            .filter(|(target, (scope, _))| {
                **target != unit.target.target
                    && *scope == preferred_scope
                    && scope_targets.contains(*target)
            })
            .map(|(_, (_, work_item))| work_item.clone())
            .collect::<Vec<_>>();
        if prerequisites.is_empty() && unit.scope == ArchitectureScope::Workspace {
            prerequisites = applicable
                .iter()
                .filter(|(target, (scope, _))| {
                    **target != unit.target.target
                        && *scope == ArchitectureScope::Module
                        && scope_targets.contains(*target)
                })
                .map(|(_, (_, work_item))| work_item.clone())
                .collect();
        }
        prerequisites.sort();
        prerequisites.dedup();
        unit.prerequisite_work = prerequisites;
    }
}

fn normalize_architecture_targets(
    targets: &[Target],
) -> Result<Vec<Target>, argus_core::ArgusError> {
    let workspace_targets = targets
        .iter()
        .filter(|target| {
            matches!(
                target.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Workspace
                }
            )
        })
        .collect::<Vec<_>>();
    if workspace_targets.len() > 1 {
        return Err(argus_core::ArgusError::invariant(
            "architecture inventory contains multiple workspace targets",
        ));
    }

    let workspace_id = workspace_targets.first().map_or_else(
        || TargetId::derive([b"argus".as_slice(), b"normalized-workspace-v1".as_slice()]),
        |target| target.id.clone(),
    );
    let mut normalized = targets.to_vec();
    if workspace_targets.is_empty() {
        normalized.push(Target {
            id: workspace_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Workspace,
            },
            visibility: TargetVisibility::Unknown,
            name: "workspace".to_owned(),
            parent: None,
            location: None,
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "architecture-scope-normalization".to_owned(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some("argus-workflow".to_owned()),
            }],
            diagnostic: None,
        });
    }
    for target in &mut normalized {
        target.validate()?;
        if target.id != workspace_id && target.parent.is_none() {
            target.parent = Some(workspace_id.clone());
        }
    }
    Ok(normalized)
}

fn synthesize_scope_evidence(
    target: &Target,
    scope: ArchitectureScope,
    targets: &BTreeMap<TargetId, &Target>,
    relations: &[Relation],
    configuration: &ConfigurationId,
    cache: &RefCell<BTreeMap<(String, String, String), EvidenceRecord>>,
    persistent_cache: Option<&Path>,
) -> Result<EvidenceRecord, argus_core::ArgusError> {
    let scope_targets = scope_target_ids(&target.id, targets, relations);
    let constituent_targets = scope_targets
        .iter()
        .filter(|id| *id != &target.id)
        .filter_map(|id| targets.get(id).copied())
        .collect::<Vec<_>>();
    let internal_relation_records = relations
        .iter()
        .filter(|relation| {
            scope_targets.contains(&relation.source) && scope_targets.contains(&relation.target)
        })
        .collect::<Vec<_>>();
    let boundary_relation_records = relations
        .iter()
        .filter(|relation| {
            scope_targets.contains(&relation.source) ^ scope_targets.contains(&relation.target)
        })
        .collect::<Vec<_>>();
    let boundary_target_ids = boundary_relation_records
        .iter()
        .flat_map(|relation| [&relation.source, &relation.target])
        .filter(|id| !scope_targets.contains(*id))
        .cloned()
        .collect::<BTreeSet<_>>();
    let boundary_target_records = boundary_target_ids
        .iter()
        .filter_map(|id| targets.get(id).copied())
        .collect::<Vec<_>>();
    let constituent_health = ConstituentHealthSummary {
        total_constituents: constituent_targets.len(),
        succeeded_constituents: constituent_targets
            .iter()
            .filter(|target| target.inventory == InventoryState::Represented)
            .count(),
        failed_constituents: constituent_targets
            .iter()
            .filter(|target| target.inventory == InventoryState::Failed)
            .count(),
        unable_to_verify_constituents: constituent_targets
            .iter()
            .filter(|target| {
                matches!(
                    target.inventory,
                    InventoryState::Pending
                        | InventoryState::Excluded
                        | InventoryState::Unsupported
                )
            })
            .count(),
    };
    let dependency_cycles = dependency_cycles(&scope_targets, relations);
    let fingerprint_bytes = serde_json::to_vec(&(
        &target.id,
        scope,
        constituent_targets
            .iter()
            .copied()
            .map(target_fact)
            .collect::<Vec<_>>(),
        boundary_target_records
            .iter()
            .copied()
            .map(target_fact)
            .collect::<Vec<_>>(),
        internal_relation_records
            .iter()
            .copied()
            .map(relation_fact)
            .collect::<Vec<_>>(),
        boundary_relation_records
            .iter()
            .copied()
            .map(relation_fact)
            .collect::<Vec<_>>(),
        &dependency_cycles,
        &constituent_health,
    ))
    .map_err(|error| {
        argus_core::ArgusError::invariant("cannot fingerprint architecture scope evidence")
            .with_source(error)
    })?;
    let fingerprint = ContentHash::digest(&fingerprint_bytes);
    let cache_key = (
        target.id.to_string(),
        configuration.to_string(),
        fingerprint.as_str().to_owned(),
    );
    if let Some(cached) = cache.borrow().get(&cache_key) {
        return Ok(cached.clone());
    }
    if let Some(directory) = persistent_cache
        && let Some(cached) = load_persistent_scope_cache(directory, &cache_key)?
    {
        cache.borrow_mut().insert(cache_key, cached.clone());
        return Ok(cached);
    }
    let (constituents, omitted_constituents) = bounded(
        constituent_targets.into_iter().map(target_fact).collect(),
        MAX_SCOPE_CONSTITUENTS,
    );
    let (boundary_targets, omitted_boundary_targets) = bounded(
        boundary_target_records
            .into_iter()
            .map(target_fact)
            .collect(),
        MAX_SCOPE_BOUNDARY_RELATIONS,
    );
    let (internal_relations, omitted_internal_relations) = bounded(
        internal_relation_records
            .into_iter()
            .map(relation_fact)
            .collect(),
        MAX_SCOPE_INTERNAL_RELATIONS,
    );
    let (boundary_relations, omitted_boundary_relations) = bounded(
        boundary_relation_records
            .into_iter()
            .map(relation_fact)
            .collect(),
        MAX_SCOPE_BOUNDARY_RELATIONS,
    );
    let mut scope_evidence = ArchitectureScopeEvidence {
        schema_version: ARCHITECTURE_SCOPE_EVIDENCE_SCHEMA_VERSION,
        fingerprint,
        target: target.id.clone(),
        scope,
        constituents,
        omitted_constituents,
        boundary_targets,
        omitted_boundary_targets,
        internal_relations,
        omitted_internal_relations,
        boundary_relations,
        omitted_boundary_relations,
        dependency_cycles,
        omitted_dependency_cycles: 0,
        constituent_health,
    };
    let bytes = enforce_scope_byte_budget(&mut scope_evidence)?;
    let id = EvidenceId::derive([
        b"architecture-scope-evidence-v1".as_slice(),
        target.id.as_str().as_bytes(),
        bytes.as_slice(),
    ]);
    let detail = String::from_utf8(bytes).map_err(|error| {
        argus_core::ArgusError::invariant("architecture scope evidence is not UTF-8")
            .with_source(error)
    })?;
    let record = EvidenceRecord {
        id,
        kind: EvidenceKind::ArchitectureGraph,
        origin: EvidenceOrigin::Inference,
        target: Some(target.id.clone()),
        location: None,
        summary: format!(
            "Deterministic {scope:?} architecture graph for {}",
            target.name
        ),
        detail: Some(detail),
        provenance: EvidenceProvenance {
            provider: "argus-architecture-synthesis".to_owned(),
            provider_version: "1".to_owned(),
            configuration: configuration.clone(),
            ingest_only: true,
            resolution: ResolutionQuality::Exact,
        },
    };
    if let Some(directory) = persistent_cache {
        store_persistent_scope_cache(directory, &cache_key, &record)?;
    }
    cache.borrow_mut().insert(cache_key, record.clone());
    Ok(record)
}

fn persistent_scope_cache_path(
    directory: &Path,
    key: &(String, String, String),
) -> Result<PathBuf, argus_core::ArgusError> {
    let bytes = serde_json::to_vec(key).map_err(|error| {
        argus_core::ArgusError::invariant("cannot serialize architecture cache key")
            .with_source(error)
    })?;
    let hash = ContentHash::digest(&bytes);
    Ok(directory.join(format!("{}.json", hash.as_str())))
}

fn load_persistent_scope_cache(
    directory: &Path,
    key: &(String, String, String),
) -> Result<Option<EvidenceRecord>, argus_core::ArgusError> {
    let path = persistent_scope_cache_path(directory, key)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(
                argus_core::ArgusError::invariant("cannot read architecture scope cache")
                    .with_source(error),
            );
        }
    };
    let entry: ArchitectureScopeCacheEntry = serde_json::from_slice(&bytes).map_err(|error| {
        argus_core::ArgusError::invalid_input("invalid architecture scope cache entry")
            .with_source(error)
    })?;
    if entry.schema_version != 1
        || entry.target != key.0
        || entry.configuration != key.1
        || entry.fingerprint != key.2
    {
        return Err(argus_core::ArgusError::invalid_input(
            "architecture scope cache identity mismatch",
        ));
    }
    entry.record.validate()?;
    let scope: ArchitectureScopeEvidence =
        serde_json::from_str(entry.record.detail.as_deref().ok_or_else(|| {
            argus_core::ArgusError::invalid_input(
                "architecture scope cache record has no structural detail",
            )
        })?)
        .map_err(|error| {
            argus_core::ArgusError::invalid_input("invalid cached architecture scope evidence")
                .with_source(error)
        })?;
    if scope.target.to_string() != key.0 || scope.fingerprint.as_str() != key.2 {
        return Err(argus_core::ArgusError::invalid_input(
            "cached architecture scope evidence fingerprint mismatch",
        ));
    }
    Ok(Some(entry.record))
}

fn store_persistent_scope_cache(
    directory: &Path,
    key: &(String, String, String),
    record: &EvidenceRecord,
) -> Result<(), argus_core::ArgusError> {
    let path = persistent_scope_cache_path(directory, key)?;
    if path.exists() {
        load_persistent_scope_cache(directory, key)?;
        return Ok(());
    }
    let entry = ArchitectureScopeCacheEntry {
        schema_version: 1,
        target: key.0.clone(),
        configuration: key.1.clone(),
        fingerprint: key.2.clone(),
        record: record.clone(),
    };
    let bytes = serde_json::to_vec(&entry).map_err(|error| {
        argus_core::ArgusError::invariant("cannot serialize architecture scope cache entry")
            .with_source(error)
    })?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                argus_core::ArgusError::invariant("cannot create architecture scope cache entry")
                    .with_source(error)
            })?;
        file.write_all(&bytes).map_err(|error| {
            argus_core::ArgusError::invariant("cannot write architecture scope cache entry")
                .with_source(error)
        })?;
        file.sync_all().map_err(|error| {
            argus_core::ArgusError::invariant("cannot sync architecture scope cache entry")
                .with_source(error)
        })?;
        drop(file);
        fs::rename(&temporary, &path).map_err(|error| {
            argus_core::ArgusError::invariant("cannot publish architecture scope cache entry")
                .with_source(error)
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn enforce_scope_byte_budget(
    evidence: &mut ArchitectureScopeEvidence,
) -> Result<Vec<u8>, argus_core::ArgusError> {
    loop {
        let bytes = serde_json::to_vec(evidence).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize architecture scope evidence")
                .with_source(error)
        })?;
        if bytes.len() <= MAX_SCOPE_EVIDENCE_BYTES {
            return Ok(bytes);
        }
        if evidence.boundary_targets.pop().is_some() {
            evidence.omitted_boundary_targets += 1;
        } else if evidence.boundary_relations.pop().is_some() {
            evidence.omitted_boundary_relations += 1;
        } else if evidence.internal_relations.pop().is_some() {
            evidence.omitted_internal_relations += 1;
        } else if evidence.constituents.pop().is_some() {
            evidence.omitted_constituents += 1;
        } else if evidence.dependency_cycles.pop().is_some() {
            evidence.omitted_dependency_cycles += 1;
        } else {
            return Err(argus_core::ArgusError::invariant(
                "architecture scope metadata exceeds its serialized byte budget",
            ));
        }
    }
}

fn target_fact(target: &Target) -> ArchitectureTargetFact {
    ArchitectureTargetFact {
        id: target.id.clone(),
        name: target.name.clone(),
        class: ArchitectureTargetProfile::from_target(target).class,
        visibility: target.visibility,
        inventory: target.inventory,
        parent: target.parent.clone(),
        capabilities: target
            .capabilities
            .iter()
            .map(|capability| (capability.name.clone(), capability.status))
            .collect(),
    }
}

fn relation_fact(relation: &Relation) -> ArchitectureRelationFact {
    ArchitectureRelationFact {
        source: relation.source.clone(),
        target: relation.target.clone(),
        kind: relation.kind.clone(),
        resolution: relation.provenance.resolution,
    }
}

fn bounded<T>(mut values: Vec<T>, maximum: usize) -> (Vec<T>, usize) {
    let omitted = values.len().saturating_sub(maximum);
    values.truncate(maximum);
    (values, omitted)
}

fn scope_target_ids(
    root: &TargetId,
    targets: &BTreeMap<TargetId, &Target>,
    relations: &[Relation],
) -> BTreeSet<TargetId> {
    let mut children: BTreeMap<TargetId, BTreeSet<TargetId>> = BTreeMap::new();
    for target in targets.values() {
        if let Some(parent) = &target.parent {
            children
                .entry(parent.clone())
                .or_default()
                .insert(target.id.clone());
        }
    }
    for relation in relations
        .iter()
        .filter(|relation| relation.kind == "core:contains")
    {
        children
            .entry(relation.source.clone())
            .or_default()
            .insert(relation.target.clone());
    }
    let mut result = BTreeSet::from([root.clone()]);
    let mut pending = vec![root.clone()];
    while let Some(parent) = pending.pop() {
        if let Some(items) = children.get(&parent) {
            for child in items {
                if result.insert(child.clone()) {
                    pending.push(child.clone());
                }
            }
        }
    }
    result
}

fn dependency_cycles(
    scope_targets: &BTreeSet<TargetId>,
    relations: &[Relation],
) -> Vec<Vec<TargetId>> {
    let mut adjacency = scope_targets
        .iter()
        .cloned()
        .map(|target| (target, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for relation in relations.iter().filter(|relation| {
        relation.kind.ends_with("depends_on")
            && scope_targets.contains(&relation.source)
            && scope_targets.contains(&relation.target)
    }) {
        adjacency
            .entry(relation.source.clone())
            .or_default()
            .push(relation.target.clone());
    }
    for targets in adjacency.values_mut() {
        targets.sort();
        targets.dedup();
    }

    struct Tarjan<'a> {
        adjacency: &'a BTreeMap<TargetId, Vec<TargetId>>,
        next_index: usize,
        stack: Vec<TargetId>,
        on_stack: BTreeSet<TargetId>,
        indices: BTreeMap<TargetId, usize>,
        lowlinks: BTreeMap<TargetId, usize>,
        cycles: Vec<Vec<TargetId>>,
    }
    impl Tarjan<'_> {
        fn visit(&mut self, node: TargetId) {
            let index = self.next_index;
            self.next_index += 1;
            self.indices.insert(node.clone(), index);
            self.lowlinks.insert(node.clone(), index);
            self.stack.push(node.clone());
            self.on_stack.insert(node.clone());

            for neighbor in self.adjacency.get(&node).cloned().unwrap_or_default() {
                if !self.indices.contains_key(&neighbor) {
                    self.visit(neighbor.clone());
                    let lowlink = self.lowlinks[&node].min(self.lowlinks[&neighbor]);
                    self.lowlinks.insert(node.clone(), lowlink);
                } else if self.on_stack.contains(&neighbor) {
                    let lowlink = self.lowlinks[&node].min(self.indices[&neighbor]);
                    self.lowlinks.insert(node.clone(), lowlink);
                }
            }

            if self.lowlinks[&node] == self.indices[&node] {
                let mut component = Vec::new();
                loop {
                    let item = self.stack.pop().expect("Tarjan stack contains its root");
                    self.on_stack.remove(&item);
                    component.push(item.clone());
                    if item == node {
                        break;
                    }
                }
                component.sort();
                let self_cycle = component.len() == 1
                    && self
                        .adjacency
                        .get(&component[0])
                        .is_some_and(|neighbors| neighbors.contains(&component[0]));
                if component.len() > 1 || self_cycle {
                    self.cycles.push(component);
                }
            }
        }
    }

    let mut tarjan = Tarjan {
        adjacency: &adjacency,
        next_index: 0,
        stack: Vec::new(),
        on_stack: BTreeSet::new(),
        indices: BTreeMap::new(),
        lowlinks: BTreeMap::new(),
        cycles: Vec::new(),
    };
    for node in adjacency.keys() {
        if !tarjan.indices.contains_key(node) {
            tarjan.visit(node.clone());
        }
    }
    tarjan.cycles.sort();
    tarjan.cycles
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

fn target_class_label(class: ArchitectureTargetClass) -> String {
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
    unit: &ArchitectureReviewUnit,
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
            "restored architecture review identity mismatch",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_creates_units_for_modules_packages_and_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let cache_directory = temporary.path().join("architecture-cache");
        let applicability = ArchitectureApplicabilityPolicy::conservative().unwrap();
        let policy_id = PolicyId::derive([b"architecture-code-derived@1".as_slice()]);
        let planner = ArchitectureReviewPlanner::new(
            &applicability,
            policy_id.clone(),
            "architecture-code-derived@1",
        )
        .unwrap()
        .with_persistent_cache(&cache_directory)
        .unwrap();

        let package_id = TargetId::derive([b"crate".as_slice()]);
        let module_a = TargetId::derive([b"crate::mod_a".as_slice()]);
        let module_b = TargetId::derive([b"crate::mod_b".as_slice()]);
        let targets = vec![
            Target {
                id: package_id.clone(),
                name: "crate".to_owned(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Package,
                },
                visibility: TargetVisibility::NotApplicable,
                location: None,
                inventory: InventoryState::Represented,
                capabilities: Vec::new(),
                diagnostic: None,
                parent: None,
            },
            Target {
                id: module_a.clone(),
                name: "mod_a".to_owned(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Module,
                },
                visibility: TargetVisibility::Public,
                location: None,
                inventory: argus_core::InventoryState::Represented,
                capabilities: Vec::new(),
                diagnostic: None,
                parent: Some(package_id.clone()),
            },
            Target {
                id: module_b.clone(),
                name: "mod_b".to_owned(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Module,
                },
                visibility: TargetVisibility::Private,
                location: None,
                inventory: InventoryState::Represented,
                capabilities: Vec::new(),
                diagnostic: None,
                parent: Some(package_id.clone()),
            },
            Target {
                id: TargetId::derive([b"crate::mod_a::func".as_slice()]),
                name: "func".to_owned(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Callable,
                },
                visibility: TargetVisibility::Public,
                location: None,
                inventory: argus_core::InventoryState::Represented,
                capabilities: Vec::new(),
                diagnostic: None,
                parent: Some(module_a.clone()),
            },
        ];
        let relations = vec![
            Relation {
                id: argus_core::RelationId::derive([b"a-to-b".as_slice()]),
                source: module_a.clone(),
                target: module_b.clone(),
                kind: "rust:depends_on".to_owned(),
                provenance: argus_core::RelationProvenance {
                    provider: "fixture".to_owned(),
                    provider_version: "1".to_owned(),
                    configuration: None,
                    ingest_only: true,
                    resolution: ResolutionQuality::Exact,
                    detail: None,
                },
            },
            Relation {
                id: argus_core::RelationId::derive([b"b-to-a".as_slice()]),
                source: module_b,
                target: module_a,
                kind: "rust:depends_on".to_owned(),
                provenance: argus_core::RelationProvenance {
                    provider: "fixture".to_owned(),
                    provider_version: "1".to_owned(),
                    configuration: None,
                    ingest_only: true,
                    resolution: ResolutionQuality::Exact,
                    detail: None,
                },
            },
        ];

        let snapshot = SnapshotId::derive([b"snap-1".as_slice()]);
        let configuration = ConfigurationId::derive([b"config-1".as_slice()]);

        let plan = planner
            .plan(&snapshot, &configuration, &targets, &[], &relations)
            .unwrap();
        assert_eq!(plan.units.len(), 5);
        assert_eq!(
            plan.units
                .iter()
                .filter(|unit| unit.applicability.state == ApplicabilityState::Applicable)
                .count(),
            4
        );
        assert_eq!(plan.evidence.len(), 4);
        let package_evidence = plan
            .evidence
            .iter()
            .find(|record| record.target.as_ref() == Some(&package_id))
            .unwrap();
        let scope: ArchitectureScopeEvidence =
            serde_json::from_str(package_evidence.detail.as_deref().unwrap()).unwrap();
        assert_eq!(scope.scope, ArchitectureScope::Package);
        assert_eq!(scope.constituents.len(), 3);
        assert_eq!(scope.internal_relations.len(), 2);
        assert_eq!(scope.dependency_cycles.len(), 1);
        assert!(package_evidence.detail.as_ref().unwrap().len() <= MAX_SCOPE_EVIDENCE_BYTES);
        let package_unit = plan
            .units
            .iter()
            .find(|unit| unit.target.target == package_id)
            .unwrap();
        assert_eq!(package_unit.prerequisite_work.len(), 2);
        let workspace_unit = plan
            .units
            .iter()
            .find(|unit| unit.scope == ArchitectureScope::Workspace)
            .unwrap();
        assert_eq!(
            workspace_unit.prerequisite_work,
            vec![package_unit.work_item.clone()]
        );
        assert!(
            plan.units
                .iter()
                .filter(|unit| unit.scope == ArchitectureScope::Module)
                .all(|unit| unit.prerequisite_work.is_empty())
        );
        let cache_entries = fs::read_dir(&cache_directory).unwrap().count();
        assert_eq!(cache_entries, 4);
        let repeated_planner = ArchitectureReviewPlanner::new(
            &applicability,
            policy_id,
            "architecture-code-derived@1",
        )
        .unwrap()
        .with_persistent_cache(&cache_directory)
        .unwrap();
        let repeated = repeated_planner
            .plan(&snapshot, &configuration, &targets, &[], &relations)
            .unwrap();
        let repeated_scope: ArchitectureScopeEvidence = serde_json::from_str(
            repeated
                .evidence
                .iter()
                .find(|record| record.target.as_ref() == Some(&package_id))
                .unwrap()
                .detail
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(scope.fingerprint, repeated_scope.fingerprint);
        assert_eq!(
            fs::read_dir(&cache_directory).unwrap().count(),
            cache_entries
        );

        let mut changed_relations = relations;
        changed_relations.pop();
        repeated_planner
            .plan(&snapshot, &configuration, &targets, &[], &changed_relations)
            .unwrap();
        assert!(fs::read_dir(&cache_directory).unwrap().count() > cache_entries);
    }

    #[test]
    fn serialized_scope_evidence_is_trimmed_to_the_hard_byte_budget() {
        let target = TargetId::derive([b"large-scope".as_slice()]);
        let mut evidence = ArchitectureScopeEvidence {
            schema_version: 1,
            fingerprint: ContentHash::digest(b"large-scope"),
            target: target.clone(),
            scope: ArchitectureScope::Workspace,
            constituents: Vec::new(),
            omitted_constituents: 0,
            boundary_targets: (0..400)
                .map(|index| ArchitectureTargetFact {
                    id: TargetId::derive([format!("target-{index}").as_bytes()]),
                    name: "x".repeat(4_096),
                    class: ArchitectureTargetClass::Module,
                    visibility: TargetVisibility::Private,
                    inventory: InventoryState::Represented,
                    parent: Some(target.clone()),
                    capabilities: BTreeMap::new(),
                })
                .collect(),
            omitted_boundary_targets: 0,
            internal_relations: Vec::new(),
            omitted_internal_relations: 0,
            boundary_relations: Vec::new(),
            omitted_boundary_relations: 0,
            dependency_cycles: Vec::new(),
            omitted_dependency_cycles: 0,
            constituent_health: ConstituentHealthSummary::default(),
        };

        let bytes = enforce_scope_byte_budget(&mut evidence).unwrap();
        assert!(bytes.len() <= MAX_SCOPE_EVIDENCE_BYTES);
        assert!(evidence.omitted_boundary_targets > 0);
    }
}
