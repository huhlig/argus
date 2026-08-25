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
    DocumentationApplicabilityDecision, DocumentationApplicabilityPolicy, DocumentationTargetClass,
    DocumentationTargetProfile,
};
use argus_storage::{CoverageKey, DurableQueue, QueueWork};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const DOCUMENTATION_REVIEW_PLAN_SCHEMA_VERSION: u32 = 1;
pub const DOCUMENTATION_EVIDENCE_PACKAGE_ARTIFACT_KIND: &str = "evidence-package";
pub const DOCUMENTATION_REVIEW_CONTEXT_ARTIFACT_KIND: &str = "review-context";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationReviewUnit {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: DocumentationTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: DocumentationApplicabilityDecision,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationReviewPlan {
    pub units: Vec<DocumentationReviewUnit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationReviewAdmission {
    pub schema_version: u32,
    pub unit: DocumentationReviewUnit,
    pub evidence_package_ref: String,
    pub review_context_ref: String,
}

#[derive(Clone, Debug)]
pub struct DocumentationReviewBatch {
    pub materializations: Vec<DocumentationReviewMaterialization>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocumentationEvidenceCatalog {
    hashes: BTreeMap<EvidenceId, ContentHash>,
}

impl DocumentationEvidenceCatalog {
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
                    "documentation evidence catalog contains duplicate IDs",
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
pub struct DocumentationReviewMaterialization {
    pub unit: DocumentationReviewUnit,
    pub package: PackageArtifact,
    pub context: ContextArtifact,
    pub contract: Arc<crate::DocumentationAssessmentContract>,
}

impl DocumentationReviewMaterialization {
    pub fn restore(
        queue: &DurableQueue,
        admission: &DocumentationReviewAdmission,
    ) -> Result<Self, argus_core::ArgusError> {
        if admission.schema_version != DOCUMENTATION_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.schema_version != DOCUMENTATION_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.applicability.state != ApplicabilityState::Applicable
        {
            return Err(argus_core::ArgusError::unsupported(
                "unsupported or inapplicable documentation review admission",
            ));
        }
        let stored_package = load_artifact(
            queue,
            &admission.evidence_package_ref,
            DOCUMENTATION_EVIDENCE_PACKAGE_ARTIFACT_KIND,
        )?;
        let package = PackageArtifact {
            hash: stored_package.content_hash,
            package: serde_json::from_slice(&stored_package.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input(
                    "invalid stored documentation evidence package",
                )
                .with_source(error)
            })?,
        };
        package.validate_identity()?;
        let stored_context = load_artifact(
            queue,
            &admission.review_context_ref,
            DOCUMENTATION_REVIEW_CONTEXT_ARTIFACT_KIND,
        )?;
        let frame: ReviewContextFrame =
            serde_json::from_slice(&stored_context.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid stored documentation review context")
                    .with_source(error)
            })?;
        let context = ContextArtifact {
            hash: stored_context.content_hash,
            frame,
            canonical_json: stored_context.payload,
        };
        validate_restored_identity(&admission.unit, &package, &context)?;
        let contract = Arc::new(crate::DocumentationAssessmentContract::from_context(
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

fn load_artifact(
    queue: &DurableQueue,
    reference: &str,
    expected_kind: &str,
) -> Result<argus_storage::StoredArtifact, argus_core::ArgusError> {
    let artifact = queue
        .artifact(reference)?
        .ok_or_else(|| argus_core::ArgusError::invariant("documentation artifact is missing"))?;
    if artifact.kind != expected_kind {
        return Err(argus_core::ArgusError::invariant(
            "documentation artifact kind mismatch",
        ));
    }
    Ok(artifact)
}

fn validate_restored_identity(
    unit: &DocumentationReviewUnit,
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
            "restored documentation review identity mismatch",
        ));
    }
    Ok(())
}

impl DocumentationReviewUnit {
    pub fn materialize(
        &self,
        store: &EvidenceStore,
        catalog: &DocumentationEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<DocumentationReviewMaterialization, argus_core::ArgusError> {
        if self.schema_version != DOCUMENTATION_REVIEW_PLAN_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported documentation review plan schema version {}",
                self.schema_version
            )));
        }
        if self.applicability.state != ApplicabilityState::Applicable {
            return Err(argus_core::ArgusError::invalid_input(
                "only applicable documentation review units can be materialized",
            ));
        }

        let mut candidates = Vec::with_capacity(self.evidence.len());
        for id in &self.evidence {
            let hash = catalog.hashes.get(id).ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "documentation review unit references uncatalogued evidence",
                )
            })?;
            let stored = store.get(hash)?;
            if stored.envelope.record.id != *id {
                return Err(argus_core::ArgusError::invariant(
                    "documentation evidence catalog identity mismatch",
                ));
            }
            candidates.push(EvidenceCandidate {
                hash: Some(hash.clone()),
                kind: stored.envelope.record.kind,
                priority: if stored.envelope.record.kind == EvidenceKind::Documentation {
                    100
                } else {
                    10
                },
                relation_depth: 0,
                estimated_tokens: stored.canonical_bytes.div_ceil(4),
                availability: CandidateAvailability::Available,
                reason: None,
            });
        }

        let requirements = PolicyEvidenceRequirements {
            allowed_kinds: BTreeSet::from([EvidenceKind::Documentation, EvidenceKind::Source]),
            required_kinds: BTreeSet::from([EvidenceKind::Documentation]),
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
        let contract = Arc::new(crate::DocumentationAssessmentContract::from_context(
            self.work_item.clone(),
            self.target.clone(),
            self.applicability.state,
            &context.frame,
        )?);
        Ok(DocumentationReviewMaterialization {
            unit: self.clone(),
            package,
            context,
            contract,
        })
    }
}

impl DocumentationReviewPlan {
    pub fn materialize_admissible(
        &self,
        store: &EvidenceStore,
        catalog: &DocumentationEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: &EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<DocumentationReviewBatch, argus_core::ArgusError> {
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
        Ok(DocumentationReviewBatch { materializations })
    }
}

impl DocumentationReviewBatch {
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
                "documentation adapter identity must be normalized",
            ));
        }
        let mut work = Vec::with_capacity(self.materializations.len());
        for materialized in &self.materializations {
            let package_bytes =
                serde_json::to_vec(&materialized.package.package).map_err(|error| {
                    argus_core::ArgusError::invariant(
                        "cannot serialize documentation evidence package",
                    )
                    .with_source(error)
                })?;
            let package = queue
                .store_artifact(DOCUMENTATION_EVIDENCE_PACKAGE_ARTIFACT_KIND, &package_bytes)?;
            if package.content_hash != materialized.package.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored documentation evidence package identity mismatch",
                ));
            }
            let context = queue.store_artifact(
                DOCUMENTATION_REVIEW_CONTEXT_ARTIFACT_KIND,
                &materialized.context.canonical_json,
            )?;
            if context.content_hash != materialized.context.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored documentation review context identity mismatch",
                ));
            }
            let admission = DocumentationReviewAdmission {
                schema_version: DOCUMENTATION_REVIEW_PLAN_SCHEMA_VERSION,
                unit: materialized.unit.clone(),
                evidence_package_ref: package.reference,
                review_context_ref: context.reference,
            };
            let payload = serde_json::to_vec(&admission).map_err(|error| {
                argus_core::ArgusError::invariant("cannot serialize documentation review admission")
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

pub struct DocumentationReviewPlanner<'a> {
    policy: &'a DocumentationApplicabilityPolicy,
    policy_id: PolicyId,
    policy_version: String,
}

impl<'a> DocumentationReviewPlanner<'a> {
    pub fn new(
        policy: &'a DocumentationApplicabilityPolicy,
        policy_id: PolicyId,
        policy_version: impl Into<String>,
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.into();
        if policy_version.trim().is_empty() || policy_version.trim() != policy_version {
            return Err(argus_core::ArgusError::invalid_input(
                "documentation policy version must be normalized",
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
    ) -> Result<DocumentationReviewPlan, argus_core::ArgusError> {
        let targets_by_id = targets
            .iter()
            .map(|target| (target.id.clone(), target))
            .collect::<BTreeMap<_, _>>();
        if targets_by_id.len() != targets.len() {
            return Err(argus_core::ArgusError::invariant(
                "documentation plan contains duplicate targets",
            ));
        }
        let mut evidence_by_target: BTreeMap<TargetId, Vec<EvidenceId>> = BTreeMap::new();
        for record in evidence {
            record.validate()?;
            if let Some(target) = &record.target {
                if !targets_by_id.contains_key(target) {
                    return Err(argus_core::ArgusError::invariant(
                        "documentation evidence references an unknown target",
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
            let mut profile = DocumentationTargetProfile::from_target(target);
            profile.visibility = effective_visibility(target, &targets_by_id, &mut BTreeSet::new());
            let applicability = self.policy.evaluate(&profile);
            let work_item = WorkItemId::derive([
                b"documentation-review".as_slice(),
                snapshot.as_str().as_bytes(),
                configuration.as_str().as_bytes(),
                profile.target.as_str().as_bytes(),
                self.policy_id.as_str().as_bytes(),
                self.policy_version.as_bytes(),
            ]);
            units.push(DocumentationReviewUnit {
                schema_version: DOCUMENTATION_REVIEW_PLAN_SCHEMA_VERSION,
                work_item,
                target: profile,
                policy: self.policy_id.clone(),
                policy_version: self.policy_version.clone(),
                applicability,
                evidence: evidence_by_target.remove(&target.id).unwrap_or_default(),
            });
        }
        Ok(DocumentationReviewPlan { units })
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

fn target_class_label(class: DocumentationTargetClass) -> String {
    serde_json::to_value(class)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{
        EvidenceKind, EvidenceOrigin, EvidenceProvenance, InventoryState, PortableTargetKind,
        ResolutionQuality, RunId, TargetKind,
    };
    use argus_policies::{DocumentationApplicabilityRule, DocumentationVisibility};
    use argus_storage::{DurableQueue, RunRecord, RunState};

    fn target(
        name: &str,
        kind: PortableTargetKind,
        visibility: TargetVisibility,
        parent: Option<TargetId>,
        inventory: InventoryState,
    ) -> Target {
        Target {
            id: TargetId::derive([name.as_bytes()]),
            kind: TargetKind::Portable { kind },
            visibility,
            name: name.to_owned(),
            parent,
            location: None,
            inventory,
            capabilities: Vec::new(),
            diagnostic: None,
        }
    }

    fn evidence(target: &Target, configuration: &ConfigurationId) -> EvidenceRecord {
        EvidenceRecord {
            id: EvidenceId::derive([b"documentation".as_slice(), target.id.as_str().as_bytes()]),
            kind: EvidenceKind::Documentation,
            origin: EvidenceOrigin::Direct,
            target: Some(target.id.clone()),
            location: None,
            summary: format!("Documentation observation for {}", target.name),
            detail: None,
            provenance: EvidenceProvenance {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                configuration: configuration.clone(),
                ingest_only: true,
                resolution: ResolutionQuality::Exact,
            },
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn plans_all_targets_and_admits_only_applicable_units() {
        let parent = target(
            "PublicTrait",
            PortableTargetKind::Type,
            TargetVisibility::Public,
            None,
            InventoryState::Represented,
        );
        let child = target(
            "required_method",
            PortableTargetKind::Callable,
            TargetVisibility::Inherited,
            Some(parent.id.clone()),
            InventoryState::Represented,
        );
        let private = target(
            "private_helper",
            PortableTargetKind::Callable,
            TargetVisibility::Private,
            None,
            InventoryState::Represented,
        );
        let failed = target(
            "FailedType",
            PortableTargetKind::Type,
            TargetVisibility::Public,
            None,
            InventoryState::Failed,
        );
        let targets = vec![parent, child, private, failed];
        let configuration = ConfigurationId::derive([b"documentation-config".as_slice()]);
        let evidence = targets
            .iter()
            .map(|target| evidence(target, &configuration))
            .collect::<Vec<_>>();
        let policy = DocumentationApplicabilityPolicy::new(vec![
            DocumentationApplicabilityRule {
                class: DocumentationTargetClass::Type,
                visibility: DocumentationVisibility::Public,
                state: ApplicabilityState::Applicable,
                rationale: "public types require documentation".to_owned(),
            },
            DocumentationApplicabilityRule {
                class: DocumentationTargetClass::Callable,
                visibility: DocumentationVisibility::Public,
                state: ApplicabilityState::Applicable,
                rationale: "public callables require documentation".to_owned(),
            },
            DocumentationApplicabilityRule {
                class: DocumentationTargetClass::Callable,
                visibility: DocumentationVisibility::Private,
                state: ApplicabilityState::NotApplicable,
                rationale: "private helper documentation is optional".to_owned(),
            },
        ])
        .unwrap();
        let snapshot = SnapshotId::derive([b"documentation-snapshot".as_slice()]);
        let planner = DocumentationReviewPlanner::new(
            &policy,
            PolicyId::derive([b"documentation-policy".as_slice()]),
            "documentation@1",
        )
        .unwrap();
        let plan = planner
            .plan(&snapshot, &configuration, &targets, &evidence)
            .unwrap();

        assert_eq!(plan.units.len(), 4);
        let child = plan
            .units
            .iter()
            .find(|unit| unit.target.target == targets[1].id)
            .unwrap();
        assert_eq!(child.target.visibility, TargetVisibility::Public);
        assert_eq!(child.applicability.state, ApplicabilityState::Applicable);
        assert_eq!(child.evidence.len(), 1);
        let failed = plan
            .units
            .iter()
            .find(|unit| unit.target.target == targets[3].id)
            .unwrap();
        assert_eq!(failed.applicability.state, ApplicabilityState::Pending);

        let temporary = tempfile::tempdir().unwrap();
        let store = EvidenceStore::open(temporary.path().join("evidence")).unwrap();
        let catalog = DocumentationEvidenceCatalog::ingest(
            &store,
            &snapshot,
            DataClassification::Internal,
            &evidence,
        )
        .unwrap();
        let batch = plan
            .materialize_admissible(
                &store,
                &catalog,
                &snapshot,
                &configuration,
                &EvidenceBudget {
                    max_bytes: 1_000_000,
                    max_tokens: 250_000,
                    max_items: 10,
                    max_relation_depth: 0,
                },
                DataClassification::Internal,
            )
            .unwrap();
        assert_eq!(batch.materializations.len(), 2);
        let materialized = batch
            .materializations
            .iter()
            .find(|item| item.unit.work_item == child.work_item)
            .unwrap();
        assert_eq!(materialized.package.package.target, targets[1].id);
        assert!(
            materialized
                .package
                .package
                .unsatisfied_requirements
                .is_empty()
        );
        assert_eq!(materialized.context.frame.untrusted_evidence.len(), 1);
        assert_eq!(
            materialized.context.frame.untrusted_evidence[0].id,
            evidence[1].id
        );

        let run = RunId::derive([b"documentation-run".as_slice()]);
        let queue = DurableQueue::open(&temporary.path().join("queue.redb")).unwrap();
        assert!(
            queue
                .create_run(&RunRecord {
                    id: run.clone(),
                    snapshot: snapshot.clone(),
                    configuration: configuration.clone(),
                    state: RunState::Active,
                    created_at_millis: 10,
                    updated_at_millis: 10,
                    finalized_at_millis: None,
                })
                .unwrap()
        );
        assert_eq!(
            batch
                .admit(&queue, &run, &snapshot, &configuration, "rust", 10)
                .unwrap(),
            2
        );
        assert_eq!(queue.status(10).unwrap().pending, 2);
        assert_eq!(queue.coverage(10).unwrap().len(), 2);
        let workflow_data =
            crate::WorkflowDataStore::open(&temporary.path().join("workflow")).unwrap();
        for item in &batch.materializations {
            let work = queue.get(&item.unit.work_item).unwrap().unwrap();
            let admission =
                serde_json::from_slice::<DocumentationReviewAdmission>(&work.payload).unwrap();
            assert_eq!(admission.unit, item.unit);
            assert!(
                queue
                    .artifact(&admission.evidence_package_ref)
                    .unwrap()
                    .is_some()
            );
            assert!(
                queue
                    .artifact(&admission.review_context_ref)
                    .unwrap()
                    .is_some()
            );
            let restored = DocumentationReviewMaterialization::restore(&queue, &admission).unwrap();
            assert_eq!(restored.unit, item.unit);
            assert_eq!(restored.package, item.package);
            assert_eq!(restored.context, item.context);
            let langchart_run_id = format!("documentation-{}", item.unit.work_item);
            assert!(matches!(
                restored
                    .initialize_workflow_data(&workflow_data, &langchart_run_id)
                    .unwrap(),
                crate::WorkflowDataWrite::Inserted(_)
            ));
            assert!(matches!(
                restored
                    .initialize_workflow_data(&workflow_data, &langchart_run_id)
                    .unwrap(),
                crate::WorkflowDataWrite::Existing(_)
            ));
        }
    }
}
