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

//! Testing review planning: one unit per module, package, and the project.
//!
//! Each unit's evidence is synthesized into a few bounded records (member index, source,
//! linked tests, upstream signals) so that the evidence package's item limit never drops
//! members silently. Records are trimmed to a share of the byte budget and every omission
//! is written into the record text.

use crate::testing_scope::{TestLink, TestingScope};
use argus_core::{
    ApplicabilityState, Confidence, ConfigurationId, ContentHash, EvidenceId, EvidenceKind,
    EvidenceOrigin, EvidenceProvenance, EvidenceRecord, FindingId, PolicyId, Relation,
    ResolutionQuality, Severity, SnapshotId, Target, TargetId, WorkItemId,
};
use argus_evidence::{
    CandidateAvailability, ContextArtifact, DataClassification, EvidenceBudget, EvidenceCandidate,
    EvidenceEnvelope, EvidencePackageBuilder, EvidenceStore, PackageArtifact,
    PolicyEvidenceRequirements, ReviewContextBuilder, ReviewContextFrame,
};
use argus_policies::{
    TestingApplicability, TestingApplicabilityPolicy, TestingLevel, TestingTargetClass,
    TestingTargetProfile,
};
use argus_storage::{CoverageKey, DurableQueue, QueueWork};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::Arc,
};

pub const TESTING_REVIEW_PLAN_SCHEMA_VERSION: u32 = 1;
pub const TESTING_EVIDENCE_PACKAGE_ARTIFACT_KIND: &str = "evidence-package";
pub const TESTING_REVIEW_CONTEXT_ARTIFACT_KIND: &str = "review-context";

const PROVIDER: &str = "argus-testing-planner";
const PROVIDER_VERSION: &str = "1";
/// Maximum upstream signals attached to one module or member unit.
const UNIT_SIGNAL_CAP: usize = 20;
/// Maximum upstream signals attached to a package or the project unit.
const SCOPE_SIGNAL_CAP: usize = 10;
/// Upstream pipelines whose findings inform library-level test needs.
const LIBRARY_SIGNAL_POLICIES: [&str; 3] = ["architecture", "optimization", "correctness"];
/// Upstream pipelines whose findings inform project-level test needs.
const PROJECT_SIGNAL_POLICIES: [&str; 2] = ["architecture", "conformance"];

/// A finding from another pipeline in the same run, normalized for use as evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpstreamSignal {
    pub finding: FindingId,
    pub policy_version: String,
    pub target: TargetId,
    /// Pipeline dimension or category, such as `error_handling`.
    pub category: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub title: String,
    pub summary: String,
    /// A human accepted this finding.
    pub accepted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingReviewUnit {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: TestingTargetProfile,
    pub level: TestingLevel,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: TestingApplicability,
    pub evidence: Vec<EvidenceId>,
    /// Targets a review may cite: the unit, its members, and linked tests.
    pub scope_targets: Vec<TargetId>,
    pub upstream_signals: Vec<FindingId>,
    pub accepted_signals: Vec<FindingId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestingReviewPlan {
    pub units: Vec<TestingReviewUnit>,
    /// Synthesized evidence referenced by the units.
    pub evidence: Vec<EvidenceRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingReviewAdmission {
    pub schema_version: u32,
    pub unit: TestingReviewUnit,
    pub evidence_package_ref: String,
    pub review_context_ref: String,
}

#[derive(Clone, Debug)]
pub struct TestingReviewBatch {
    pub materializations: Vec<TestingReviewMaterialization>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestingEvidenceCatalog {
    hashes: BTreeMap<EvidenceId, ContentHash>,
}

impl TestingEvidenceCatalog {
    pub fn ingest(
        store: &EvidenceStore,
        snapshot: &SnapshotId,
        classification: DataClassification,
        records: &[EvidenceRecord],
    ) -> Result<Self, argus_core::ArgusError> {
        let mut hashes = BTreeMap::new();
        for record in records {
            let envelope =
                EvidenceEnvelope::current(snapshot.clone(), classification, record.clone());
            let hash = store.put(&envelope)?;
            if hashes.insert(record.id.clone(), hash).is_some() {
                return Err(argus_core::ArgusError::invariant(
                    "testing evidence catalog contains duplicate IDs",
                ));
            }
        }
        Ok(Self { hashes })
    }
}

#[derive(Clone, Debug)]
pub struct TestingReviewMaterialization {
    pub unit: TestingReviewUnit,
    pub package: PackageArtifact,
    pub context: ContextArtifact,
    pub contract: Arc<crate::TestingAssessmentContract>,
}

impl TestingReviewMaterialization {
    pub fn restore(
        queue: &DurableQueue,
        admission: &TestingReviewAdmission,
    ) -> Result<Self, argus_core::ArgusError> {
        if admission.schema_version != TESTING_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.schema_version != TESTING_REVIEW_PLAN_SCHEMA_VERSION
            || admission.unit.applicability.state != ApplicabilityState::Applicable
        {
            return Err(argus_core::ArgusError::unsupported(
                "unsupported or inapplicable testing review admission",
            ));
        }
        let stored_package = load_artifact(
            queue,
            &admission.evidence_package_ref,
            TESTING_EVIDENCE_PACKAGE_ARTIFACT_KIND,
        )?;
        let package = PackageArtifact {
            hash: stored_package.content_hash,
            package: serde_json::from_slice(&stored_package.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid stored testing evidence package")
                    .with_source(error)
            })?,
        };
        package.validate_identity()?;
        let stored_context = load_artifact(
            queue,
            &admission.review_context_ref,
            TESTING_REVIEW_CONTEXT_ARTIFACT_KIND,
        )?;
        let frame: ReviewContextFrame =
            serde_json::from_slice(&stored_context.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid stored testing review context")
                    .with_source(error)
            })?;
        let context = ContextArtifact {
            hash: stored_context.content_hash,
            frame,
            canonical_json: stored_context.payload,
        };
        validate_restored_identity(&admission.unit, &package, &context)?;
        let contract = Arc::new(crate::TestingAssessmentContract::from_context(
            &admission.unit,
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

impl TestingReviewUnit {
    pub fn materialize(
        &self,
        store: &EvidenceStore,
        catalog: &TestingEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<TestingReviewMaterialization, argus_core::ArgusError> {
        if self.schema_version != TESTING_REVIEW_PLAN_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported testing review plan schema version {}",
                self.schema_version
            )));
        }
        if self.applicability.state != ApplicabilityState::Applicable {
            return Err(argus_core::ArgusError::invalid_input(
                "only applicable testing review units can be materialized",
            ));
        }
        let mut candidates = Vec::with_capacity(self.evidence.len());
        for id in &self.evidence {
            let hash = catalog.hashes.get(id).ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "testing review unit references uncatalogued evidence",
                )
            })?;
            let stored = store.get(hash)?;
            if stored.envelope.record.id != *id {
                return Err(argus_core::ArgusError::invariant(
                    "testing evidence catalog identity mismatch",
                ));
            }
            let kind = stored.envelope.record.kind;
            candidates.push(EvidenceCandidate {
                hash: Some(hash.clone()),
                kind,
                priority: evidence_priority(kind),
                relation_depth: 0,
                estimated_tokens: stored.canonical_bytes.div_ceil(4),
                availability: CandidateAvailability::Available,
                reason: None,
            });
        }
        let requirements = PolicyEvidenceRequirements {
            allowed_kinds: BTreeSet::from([
                EvidenceKind::Source,
                EvidenceKind::Test,
                EvidenceKind::Benchmark,
                EvidenceKind::StaticAnalysis,
                EvidenceKind::ReviewFinding,
            ]),
            required_kinds: BTreeSet::from([EvidenceKind::StaticAnalysis]),
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
        let contract = Arc::new(crate::TestingAssessmentContract::from_context(
            self,
            &context.frame,
        )?);
        Ok(TestingReviewMaterialization {
            unit: self.clone(),
            package,
            context,
            contract,
        })
    }
}

const fn evidence_priority(kind: EvidenceKind) -> u16 {
    match kind {
        EvidenceKind::StaticAnalysis => 40,
        EvidenceKind::Source => 30,
        EvidenceKind::Test | EvidenceKind::Benchmark => 20,
        _ => 10,
    }
}

impl TestingReviewPlan {
    pub fn materialize_admissible(
        &self,
        store: &EvidenceStore,
        catalog: &TestingEvidenceCatalog,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        budget: &EvidenceBudget,
        maximum_classification: DataClassification,
    ) -> Result<TestingReviewBatch, argus_core::ArgusError> {
        let materializations = self
            .units
            .iter()
            .filter(|unit| unit.applicability.state == ApplicabilityState::Applicable)
            .map(|unit| {
                unit.materialize(
                    store,
                    catalog,
                    snapshot,
                    configuration,
                    budget.clone(),
                    maximum_classification,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TestingReviewBatch { materializations })
    }
}

impl TestingReviewBatch {
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
                "testing adapter identity must be normalized",
            ));
        }
        let mut work = Vec::with_capacity(self.materializations.len());
        for materialized in &self.materializations {
            let package_bytes =
                serde_json::to_vec(&materialized.package.package).map_err(|error| {
                    argus_core::ArgusError::invariant("cannot serialize testing evidence package")
                        .with_source(error)
                })?;
            let package =
                queue.store_artifact(TESTING_EVIDENCE_PACKAGE_ARTIFACT_KIND, &package_bytes)?;
            if package.content_hash != materialized.package.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored testing evidence package identity mismatch",
                ));
            }
            let context = queue.store_artifact(
                TESTING_REVIEW_CONTEXT_ARTIFACT_KIND,
                &materialized.context.canonical_json,
            )?;
            if context.content_hash != materialized.context.hash {
                return Err(argus_core::ArgusError::invariant(
                    "stored testing review context identity mismatch",
                ));
            }
            let admission = TestingReviewAdmission {
                schema_version: TESTING_REVIEW_PLAN_SCHEMA_VERSION,
                unit: materialized.unit.clone(),
                evidence_package_ref: package.reference,
                review_context_ref: context.reference,
            };
            let payload = serde_json::to_vec(&admission).map_err(|error| {
                argus_core::ArgusError::invariant(
                    "cannot serialize testing review admission payload",
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
                    target_kind: format!("{:?}", materialized.unit.target.class).to_lowercase(),
                    policy: materialized.unit.policy_version.clone(),
                },
            ));
        }
        queue.admit_batch(&work, at_millis)
    }
}

pub struct TestingReviewPlanner<'a> {
    policy: &'a TestingApplicabilityPolicy,
    policy_id: PolicyId,
    policy_version: String,
}

/// Inputs shared by every unit built for one plan.
struct PlanContext<'a> {
    scope: TestingScope<'a>,
    sources: BTreeMap<TargetId, &'a EvidenceRecord>,
    signals: BTreeMap<TargetId, Vec<&'a UpstreamSignal>>,
    snapshot: &'a SnapshotId,
    configuration: &'a ConfigurationId,
    /// Bytes available to one unit's synthesized evidence.
    unit_bytes: usize,
}

impl<'a> TestingReviewPlanner<'a> {
    pub fn new(
        policy: &'a TestingApplicabilityPolicy,
        policy_id: PolicyId,
        policy_version: impl Into<String>,
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.into();
        if policy_version.trim().is_empty() || policy_version.trim() != policy_version {
            return Err(argus_core::ArgusError::invalid_input(
                "testing policy version must be normalized",
            ));
        }
        Ok(Self {
            policy,
            policy_id,
            policy_version,
        })
    }

    /// Plans module, package, and project units for an inventory.
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        &self,
        snapshot: &SnapshotId,
        configuration: &ConfigurationId,
        targets: &[Target],
        evidence: &[EvidenceRecord],
        relations: &[Relation],
        signals: &[UpstreamSignal],
        budget: &EvidenceBudget,
    ) -> Result<TestingReviewPlan, argus_core::ArgusError> {
        let ids = targets
            .iter()
            .map(|target| &target.id)
            .collect::<BTreeSet<_>>();
        if ids.len() != targets.len() {
            return Err(argus_core::ArgusError::invariant(
                "testing plan contains duplicate targets",
            ));
        }
        let mut sources = BTreeMap::new();
        for record in evidence {
            record.validate()?;
            if record.kind != EvidenceKind::Source {
                continue;
            }
            if let Some(target) = &record.target {
                // Keep the most complete source text when an adapter emits several.
                let replace = sources
                    .get(target)
                    .is_none_or(|existing: &&EvidenceRecord| {
                        detail_len(existing) < detail_len(record)
                    });
                if replace {
                    sources.insert(target.clone(), record);
                }
            }
        }
        let mut signals_by_target: BTreeMap<TargetId, Vec<&UpstreamSignal>> = BTreeMap::new();
        for signal in signals {
            signals_by_target
                .entry(signal.target.clone())
                .or_default()
                .push(signal);
        }
        let context = PlanContext {
            scope: TestingScope::analyze(targets, relations),
            sources,
            signals: signals_by_target,
            snapshot,
            configuration,
            // Tokens are estimated at four bytes each; keep a tenth for envelope overhead.
            unit_bytes: budget.max_bytes.min(budget.max_tokens.saturating_mul(4)) / 10 * 9,
        };

        let mut builder = PlanBuilder {
            planner: self,
            context: &context,
            units: Vec::new(),
            evidence: Vec::new(),
        };
        for target in targets {
            let is_test = context.scope.is_test_code(&target.id);
            let profile = TestingTargetProfile::from_target(target, is_test);
            match profile.class {
                TestingTargetClass::Module => builder.module_units(target, profile),
                TestingTargetClass::Package => builder.package_unit(target, profile),
                _ => builder.inapplicable(profile),
            }
        }
        builder.project_unit(targets);
        builder
            .units
            .sort_by(|left, right| left.work_item.cmp(&right.work_item));
        Ok(TestingReviewPlan {
            units: builder.units,
            evidence: builder.evidence,
        })
    }

    fn work_item(&self, context: &PlanContext<'_>, target: &TargetId) -> WorkItemId {
        WorkItemId::derive([
            b"testing-review".as_slice(),
            context.snapshot.as_str().as_bytes(),
            context.configuration.as_str().as_bytes(),
            target.as_str().as_bytes(),
            self.policy_id.as_str().as_bytes(),
            self.policy_version.as_bytes(),
        ])
    }
}

struct PlanBuilder<'p, 'a> {
    planner: &'p TestingReviewPlanner<'a>,
    context: &'p PlanContext<'a>,
    units: Vec<TestingReviewUnit>,
    evidence: Vec<EvidenceRecord>,
}

/// One synthesized evidence section before it becomes a record.
struct Section {
    kind: EvidenceKind,
    name: &'static str,
    summary: String,
    text: String,
}

impl<'a> PlanBuilder<'_, 'a> {
    fn inapplicable(&mut self, profile: TestingTargetProfile) {
        let applicability = self.planner.policy.evaluate(&profile);
        let level = profile.level().unwrap_or(TestingLevel::Unit);
        let work_item = self.planner.work_item(self.context, &profile.target);
        self.units.push(TestingReviewUnit {
            schema_version: TESTING_REVIEW_PLAN_SCHEMA_VERSION,
            work_item,
            target: profile,
            level,
            policy: self.planner.policy_id.clone(),
            policy_version: self.planner.policy_version.clone(),
            applicability,
            evidence: Vec::new(),
            scope_targets: Vec::new(),
            upstream_signals: Vec::new(),
            accepted_signals: Vec::new(),
        });
    }

    fn push_unit(
        &mut self,
        profile: TestingTargetProfile,
        level: TestingLevel,
        sections: Vec<Section>,
        scope_targets: BTreeSet<TargetId>,
        signals: &[&UpstreamSignal],
    ) {
        let mut applicability = self.planner.policy.evaluate(&profile);
        if applicability.state == ApplicabilityState::Applicable && scope_targets.len() <= 1 {
            applicability = TestingApplicability {
                state: ApplicabilityState::NotApplicable,
                reasons: vec!["unit has no production code to review".to_owned()],
            };
        }
        let mut evidence = Vec::with_capacity(sections.len());
        if applicability.state == ApplicabilityState::Applicable {
            for section in sections {
                let record = self.record(&profile, section);
                evidence.push(record.id.clone());
                self.evidence.push(record);
            }
        }
        let work_item = self.planner.work_item(self.context, &profile.target);
        self.units.push(TestingReviewUnit {
            schema_version: TESTING_REVIEW_PLAN_SCHEMA_VERSION,
            work_item,
            target: profile,
            level,
            policy: self.planner.policy_id.clone(),
            policy_version: self.planner.policy_version.clone(),
            applicability,
            evidence,
            scope_targets: scope_targets.into_iter().collect(),
            upstream_signals: signals
                .iter()
                .map(|signal| signal.finding.clone())
                .collect(),
            accepted_signals: signals
                .iter()
                .filter(|signal| signal.accepted)
                .map(|signal| signal.finding.clone())
                .collect(),
        });
    }

    fn record(&self, profile: &TestingTargetProfile, section: Section) -> EvidenceRecord {
        EvidenceRecord {
            id: EvidenceId::derive([
                b"testing-evidence".as_slice(),
                profile.target.as_str().as_bytes(),
                section.name.as_bytes(),
                ContentHash::digest(section.text.as_bytes())
                    .as_str()
                    .as_bytes(),
            ]),
            kind: section.kind,
            origin: EvidenceOrigin::Inference,
            target: Some(profile.target.clone()),
            location: profile.location.clone(),
            summary: section.summary,
            detail: Some(section.text),
            provenance: EvidenceProvenance {
                provider: PROVIDER.to_owned(),
                provider_version: PROVIDER_VERSION.to_owned(),
                configuration: self.context.configuration.clone(),
                ingest_only: true,
                resolution: ResolutionQuality::ContainingTarget,
            },
        }
    }

    fn module_units(&mut self, module: &Target, profile: TestingTargetProfile) {
        let scope = &self.context.scope;
        let members = scope
            .members(&module.id)
            .iter()
            .filter_map(|id| scope.target(id))
            .collect::<Vec<_>>();
        let share = |percent: usize| self.context.unit_bytes / 100 * percent;
        let source = self.module_source(module, &members);
        let fits = source.len() <= share(45);

        let mut split_members = Vec::new();
        if !fits {
            for member in &members {
                let signalled = self.context.signals.contains_key(&member.id);
                if member.visibility == argus_core::TargetVisibility::Public || signalled {
                    split_members.push(*member);
                }
            }
        }
        let source_section = if fits {
            Section {
                kind: EvidenceKind::Source,
                name: "module-source",
                summary: format!("Source of module {}", module.name),
                text: source,
            }
        } else {
            Section {
                kind: EvidenceKind::Source,
                name: "module-signatures",
                summary: format!("Member signatures of oversized module {}", module.name),
                text: trim(
                    &format!(
                        "Module source exceeds the evidence budget; members listed by signature.{}\n\n{}",
                        if split_members.is_empty() {
                            String::new()
                        } else {
                            format!(
                                " {} public or signalled member(s) are reviewed as separate units.",
                                split_members.len()
                            )
                        },
                        self.signatures(&members)
                    ),
                    share(45),
                ),
            }
        };
        let signals = self.unit_signals(module, &members);
        let mut scope_targets = BTreeSet::from([module.id.clone()]);
        scope_targets.extend(members.iter().map(|member| member.id.clone()));
        let tests = self.linked_tests(&members, Some(module));
        scope_targets.extend(tests.iter().cloned());
        let sections = vec![
            Section {
                kind: EvidenceKind::StaticAnalysis,
                name: "member-index",
                summary: format!("Members and linked tests of module {}", module.name),
                text: trim(&self.member_index(Some(module), &members), share(15)),
            },
            source_section,
            Section {
                kind: EvidenceKind::Test,
                name: "linked-tests",
                summary: format!("Tests linked to module {}", module.name),
                text: trim(&self.test_sources(&tests), share(30)),
            },
            Self::signal_section(&signals, share(10)),
        ];
        self.push_unit(
            profile,
            TestingLevel::Unit,
            sections,
            scope_targets,
            &signals,
        );

        for member in split_members {
            self.member_unit(module, member);
        }
    }

    fn member_unit(&mut self, module: &Target, member: &Target) {
        let share = |percent: usize| self.context.unit_bytes / 100 * percent;
        let profile = TestingTargetProfile::split_member(member, module.id.clone());
        let signals = self.unit_signals(member, &[]);
        let tests = self.linked_tests(&[member], None);
        let mut scope_targets = BTreeSet::from([member.id.clone()]);
        scope_targets.extend(tests.iter().cloned());
        // A split member always has code to review, even with no linked tests.
        scope_targets.insert(module.id.clone());
        let sections = vec![
            Section {
                kind: EvidenceKind::StaticAnalysis,
                name: "member-index",
                summary: format!("Linked tests of {}", member.name),
                text: trim(&self.member_index(None, &[member]), share(15)),
            },
            Section {
                kind: EvidenceKind::Source,
                name: "member-source",
                summary: format!("Source of {}", member.name),
                text: trim(&self.source_text(&member.id).unwrap_or_default(), share(45)),
            },
            Section {
                kind: EvidenceKind::Test,
                name: "linked-tests",
                summary: format!("Tests linked to {}", member.name),
                text: trim(&self.test_sources(&tests), share(30)),
            },
            Self::signal_section(&signals, share(10)),
        ];
        self.push_unit(
            profile,
            TestingLevel::Unit,
            sections,
            scope_targets,
            &signals,
        );
    }

    fn package_unit(&mut self, package: &Target, profile: TestingTargetProfile) {
        let scope = &self.context.scope;
        let share = |percent: usize| self.context.unit_bytes / 100 * percent;
        let in_package = |id: &TargetId| {
            scope
                .package_of(id)
                .is_some_and(|owner| owner.id == package.id)
        };
        let modules = scope
            .modules()
            .filter(|(module, _)| in_package(module))
            .collect::<Vec<_>>();
        let public = modules
            .iter()
            .flat_map(|(_, members)| members.iter())
            .filter_map(|id| scope.target(id))
            .filter(|member| member.visibility == argus_core::TargetVisibility::Public)
            .collect::<Vec<_>>();
        let test_files = scope
            .test_files()
            .filter(|file| in_package(&file.id))
            .collect::<Vec<_>>();

        let mut summary = format!(
            "Package {} (target {})\n\n## Public API\n",
            package.name, package.id
        );
        if public.is_empty() {
            summary.push_str("No public members were discovered.\n");
        }
        summary.push_str(&self.signatures(&public));
        summary.push_str("\n\n## Module test summary\n");
        for (module, members) in &modules {
            let linked = members
                .iter()
                .filter(|member| scope.tests_for(member).next().is_some())
                .count();
            let name = scope
                .target(module)
                .map_or("?", |target| target.name.as_str());
            let _ = writeln!(
                summary,
                "- {name}: {} member(s), {linked} with linked tests, {} importing test file(s)",
                members.len(),
                scope.importing_tests(module).count()
            );
        }
        let signals = self.scope_signals(
            |target| in_package(target) || *target == package.id,
            &LIBRARY_SIGNAL_POLICIES,
        );
        let mut scope_targets = BTreeSet::from([package.id.clone()]);
        scope_targets.extend(public.iter().map(|member| member.id.clone()));
        scope_targets.extend(modules.iter().map(|(module, _)| (*module).clone()));
        scope_targets.extend(test_files.iter().map(|file| file.id.clone()));
        let sections = vec![
            Section {
                kind: EvidenceKind::StaticAnalysis,
                name: "package-summary",
                summary: format!(
                    "Public API and module test summary of package {}",
                    package.name
                ),
                text: trim(&summary, share(50)),
            },
            Section {
                kind: EvidenceKind::Test,
                name: "integration-tests",
                summary: format!("Test inventory of package {}", package.name),
                text: trim(&self.inventory(&test_files, false), share(30)),
            },
            Section {
                kind: EvidenceKind::Benchmark,
                name: "benchmarks",
                summary: format!("Benchmark inventory of package {}", package.name),
                text: trim(&self.inventory(&test_files, true), share(10)),
            },
            Self::signal_section(&signals, share(10)),
        ];
        self.push_unit(
            profile,
            TestingLevel::Library,
            sections,
            scope_targets,
            &signals,
        );
    }

    fn project_unit(&mut self, targets: &[Target]) {
        let scope = &self.context.scope;
        let share = |percent: usize| self.context.unit_bytes / 100 * percent;
        let project = TargetId::derive([b"argus".as_slice(), b"testing-project-v1".as_slice()]);
        let profile = TestingTargetProfile::project(project.clone(), "project".to_owned());
        let packages = targets
            .iter()
            .filter(|target| {
                TestingTargetProfile::from_target(target, false).class
                    == TestingTargetClass::Package
            })
            .collect::<Vec<_>>();
        let test_files = scope.test_files().collect::<Vec<_>>();

        let mut summary = String::from("## Packages\n");
        if packages.is_empty() {
            summary.push_str("No packages were discovered.\n");
        }
        for package in &packages {
            let owned = |id: &TargetId| {
                scope
                    .package_of(id)
                    .is_some_and(|owner| owner.id == package.id)
            };
            let public = scope
                .modules()
                .filter(|(module, _)| owned(module))
                .flat_map(|(_, members)| members.iter())
                .filter_map(|id| scope.target(id))
                .filter(|member| member.visibility == argus_core::TargetVisibility::Public)
                .count();
            let tests = test_files.iter().filter(|file| owned(&file.id)).count();
            let _ = writeln!(
                summary,
                "- {} (target {}) at {}: {public} public member(s), {tests} test file(s)",
                package.name,
                package.id,
                location_text(package)
            );
        }
        summary.push_str("\n## Entry points\n");
        let entry_points = targets
            .iter()
            .filter(|target| matches!(&target.kind, argus_core::TargetKind::LanguageSpecific { kind, .. } if kind.contains("bin")))
            .collect::<Vec<_>>();
        if entry_points.is_empty() {
            summary.push_str("No binary entry points were discovered.\n");
        }
        for entry in &entry_points {
            let _ = writeln!(summary, "- {} at {}", entry.name, location_text(entry));
        }
        let signals = self.scope_signals(
            |target| scope.package_of(target).is_none(),
            &PROJECT_SIGNAL_POLICIES,
        );
        let mut scope_targets = BTreeSet::from([project]);
        scope_targets.extend(packages.iter().map(|package| package.id.clone()));
        scope_targets.extend(test_files.iter().map(|file| file.id.clone()));
        let sections = vec![
            Section {
                kind: EvidenceKind::StaticAnalysis,
                name: "project-summary",
                summary: "Packages and entry points of the project".to_owned(),
                text: trim(&summary, share(50)),
            },
            Section {
                kind: EvidenceKind::Test,
                name: "external-tests",
                summary: "Test inventory of the project".to_owned(),
                text: trim(&self.inventory(&test_files, false), share(30)),
            },
            Section {
                kind: EvidenceKind::Benchmark,
                name: "benchmarks",
                summary: "Benchmark inventory of the project".to_owned(),
                text: trim(&self.inventory(&test_files, true), share(10)),
            },
            Self::signal_section(&signals, share(10)),
        ];
        self.push_unit(
            profile,
            TestingLevel::Project,
            sections,
            scope_targets,
            &signals,
        );
    }

    fn source_text(&self, target: &TargetId) -> Option<String> {
        self.context
            .sources
            .get(target)
            .and_then(|record| record.detail.clone())
            .filter(|detail| !detail.trim().is_empty())
    }

    /// The module's own source when the adapter provides it, else its members' sources.
    fn module_source(&self, module: &Target, members: &[&Target]) -> String {
        if let Some(source) = self.source_text(&module.id) {
            return source;
        }
        let mut text = String::new();
        let mut missing = Vec::new();
        for member in members {
            // A member nested in another member (a method in a type) is already in its source.
            let nested = member.parent.as_ref().is_some_and(|parent| {
                members.iter().any(|other| &other.id == parent)
                    && self.source_text(parent).is_some()
            });
            if nested {
                continue;
            }
            match self.source_text(&member.id) {
                Some(source) => {
                    let _ = writeln!(
                        text,
                        "// {} (target {})\n{source}\n",
                        member.name, member.id
                    );
                }
                None => missing.push(member.name.as_str()),
            }
        }
        if text.is_empty() {
            return "The language adapter provided no source evidence for this module; review from the member index.".to_owned();
        }
        if !missing.is_empty() {
            let _ = writeln!(text, "// No source evidence for: {}", missing.join(", "));
        }
        text
    }

    fn signatures(&self, members: &[&Target]) -> String {
        let mut text = String::new();
        for member in members {
            let signature = self
                .source_text(&member.id)
                .and_then(|source| signature_line(&source))
                .unwrap_or_default();
            let _ = writeln!(
                text,
                "- {} {} (target {}) at {}{}",
                kind_label(member),
                member.name,
                member.id,
                location_text(member),
                if signature.is_empty() {
                    String::new()
                } else {
                    format!(": `{signature}`")
                }
            );
        }
        text
    }

    fn member_index(&self, module: Option<&Target>, members: &[&Target]) -> String {
        let scope = &self.context.scope;
        let mut text = String::new();
        if let Some(module) = module {
            let _ = writeln!(
                text,
                "Module {} (target {}) at {}",
                module.name,
                module.id,
                location_text(module)
            );
            let importers = scope
                .importing_tests(&module.id)
                .filter_map(|id| scope.target(id))
                .collect::<Vec<_>>();
            for importer in importers {
                let _ = writeln!(
                    text,
                    "- imported by test file {} (target {})",
                    importer.name, importer.id
                );
            }
            text.push('\n');
        }
        text.push_str(
            "Test links come from call, reference, and import relations. A member with no linked tests may still be tested through dynamic dispatch, macros, or external inputs.\n\n",
        );
        for member in members {
            let links = scope.tests_for(&member.id).collect::<Vec<_>>();
            let signals = self.context.signals.get(&member.id).map_or(0, Vec::len);
            let _ = writeln!(
                text,
                "- {} {} [{:?}] (target {}) at {}: {} linked test(s), {signals} upstream finding(s)",
                kind_label(member),
                member.name,
                member.visibility,
                member.id,
                location_text(member),
                links.len()
            );
            for link in links {
                let _ = writeln!(text, "  - {}", self.link_text(link));
            }
        }
        text
    }

    fn link_text(&self, link: &TestLink) -> String {
        let scope = &self.context.scope;
        let name = |id: &TargetId| {
            scope
                .target(id)
                .map_or_else(|| id.to_string(), |target| target.name.clone())
        };
        let kind = if scope.is_benchmark(&link.test) {
            "benchmark"
        } else {
            "test"
        };
        match &link.via {
            None => format!("{kind} {} (target {})", name(&link.test), link.test),
            Some(via) => format!(
                "{kind} {} (target {}) indirectly via {}",
                name(&link.test),
                link.test,
                name(via)
            ),
        }
    }

    fn linked_tests(&self, members: &[&Target], module: Option<&Target>) -> BTreeSet<TargetId> {
        let scope = &self.context.scope;
        let mut tests = members
            .iter()
            .flat_map(|member| scope.tests_for(&member.id))
            .map(|link| link.test.clone())
            .collect::<BTreeSet<_>>();
        if let Some(module) = module {
            tests.extend(scope.importing_tests(&module.id).cloned());
        }
        tests
    }

    fn test_sources(&self, tests: &BTreeSet<TargetId>) -> String {
        if tests.is_empty() {
            return "No tests are linked to this unit.".to_owned();
        }
        let scope = &self.context.scope;
        let mut text = String::new();
        for test in tests {
            let Some(target) = scope.target(test) else {
                continue;
            };
            let kind = if scope.is_benchmark(test) {
                "Benchmark"
            } else {
                "Test"
            };
            let _ = writeln!(
                text,
                "// {kind} {} (target {}) at {}",
                target.name,
                target.id,
                location_text(target)
            );
            match self.source_text(test) {
                Some(source) => {
                    let _ = writeln!(text, "{source}\n");
                }
                None => text.push_str("// (no source evidence)\n\n"),
            }
        }
        text
    }

    fn inventory(&self, files: &[&Target], benchmarks: bool) -> String {
        let scope = &self.context.scope;
        let selected = files
            .iter()
            .filter(|file| scope.is_benchmark(&file.id) == benchmarks)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return if benchmarks {
                "No benchmark files were discovered.".to_owned()
            } else {
                "No test files were discovered.".to_owned()
            };
        }
        let mut text = String::new();
        for file in selected {
            let _ = writeln!(
                text,
                "- {} (target {}) at {}",
                file.name,
                file.id,
                location_text(file)
            );
        }
        text
    }

    /// Signals on a unit and its members, highest severity first.
    fn unit_signals(&self, unit: &Target, members: &[&Target]) -> Vec<&'a UpstreamSignal> {
        let mut signals = std::iter::once(&unit.id)
            .chain(members.iter().map(|member| &member.id))
            .filter_map(|id| self.context.signals.get(id))
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        rank_signals(&mut signals);
        signals.truncate(UNIT_SIGNAL_CAP);
        signals
    }

    fn scope_signals(
        &self,
        in_scope: impl Fn(&TargetId) -> bool,
        policies: &[&str],
    ) -> Vec<&'a UpstreamSignal> {
        let mut signals = self
            .context
            .signals
            .iter()
            .filter(|(target, _)| in_scope(target))
            .flat_map(|(_, signals)| signals.iter().copied())
            .filter(|signal| {
                policies
                    .iter()
                    .any(|policy| signal.policy_version.starts_with(policy))
            })
            .collect::<Vec<_>>();
        rank_signals(&mut signals);
        signals.truncate(SCOPE_SIGNAL_CAP);
        signals
    }

    fn signal_section(signals: &[&UpstreamSignal], bytes: usize) -> Section {
        let mut text = String::new();
        if signals.is_empty() {
            text.push_str("No upstream findings are attached to this unit.");
        }
        for signal in signals {
            let _ = writeln!(
                text,
                "- finding {} [{}{}] {:?} severity, {} bp confidence, on target {}: {} — {}",
                signal.finding,
                signal.policy_version,
                if signal.category.is_empty() {
                    String::new()
                } else {
                    format!(" / {}", signal.category)
                },
                signal.severity,
                signal.confidence.basis_points(),
                signal.target,
                signal.title,
                signal.summary,
            );
            if signal.accepted {
                text.push_str("  (accepted by a human reviewer)\n");
            }
        }
        Section {
            kind: EvidenceKind::ReviewFinding,
            name: "upstream-signals",
            summary: format!("{} upstream finding(s) from other pipelines", signals.len()),
            text: trim(&text, bytes),
        }
    }
}

fn rank_signals(signals: &mut [&UpstreamSignal]) {
    signals.sort_by(|left, right| {
        severity_rank(right.severity)
            .cmp(&severity_rank(left.severity))
            .then_with(|| {
                right
                    .confidence
                    .basis_points()
                    .cmp(&left.confidence.basis_points())
            })
            .then_with(|| left.finding.cmp(&right.finding))
    });
}

const fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Note => 0,
        Severity::Low => 1,
        Severity::Medium => 2,
        Severity::High => 3,
        Severity::Critical => 4,
    }
}

fn detail_len(record: &EvidenceRecord) -> usize {
    record.detail.as_ref().map_or(0, String::len)
}

fn kind_label(target: &Target) -> String {
    match &target.kind {
        argus_core::TargetKind::Portable { kind } => format!("{kind:?}").to_lowercase(),
        argus_core::TargetKind::LanguageSpecific { kind, .. } => kind.clone(),
    }
}

fn location_text(target: &Target) -> String {
    target.location.as_ref().map_or_else(
        || "unknown location".to_owned(),
        |location| match location.start {
            Some(start) => format!("{}:{}", location.path.as_str(), start.line),
            None => location.path.as_str().to_owned(),
        },
    )
}

/// First declaration line of a source snippet, skipping comments and attributes.
fn signature_line(source: &str) -> Option<String> {
    source
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !["//", "#", "@", "/*", "*", "\"\"\""]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
        })
        .map(|line| {
            let line = line.trim_end_matches('{').trim_end();
            line.chars().take(160).collect()
        })
}

/// Truncates text to a byte budget on a character boundary, noting the omission.
fn trim(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let note = format!(
        "\n… {} more bytes omitted to fit the evidence budget.",
        text.len()
    );
    let mut end = bytes.saturating_sub(note.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{note}", &text[..end])
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
    unit: &TestingReviewUnit,
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
            "restored testing review identity mismatch",
        ));
    }
    Ok(())
}
