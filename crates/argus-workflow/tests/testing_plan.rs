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
    ApplicabilityState, ByteSpan, Confidence, ConfigurationId, EvidenceId, EvidenceKind,
    EvidenceOrigin, EvidenceProvenance, EvidenceRecord, FindingId, InventoryState, PolicyId,
    PortableTargetKind, Relation, RelationId, RelationProvenance, ResolutionQuality, RunId,
    Severity, SnapshotId, SourceLocation, SourcePath, Target, TargetId, TargetKind,
    TargetVisibility,
};
use argus_evidence::{DataClassification, EvidenceBudget, EvidenceStore};
use argus_policies::{TestingApplicabilityPolicy, TestingLevel, TestingTargetClass};
use argus_storage::{DurableQueue, RunRecord, RunState};
use argus_workflow::{
    TestingEvidenceCatalog, TestingReviewAdmission, TestingReviewMaterialization,
    TestingReviewPlan, TestingReviewPlanner, TestingReviewUnit, UpstreamSignal,
};

fn id(name: &str) -> TargetId {
    TargetId::derive([name.as_bytes()])
}

fn configuration() -> ConfigurationId {
    ConfigurationId::derive([b"cfg".as_slice()])
}

fn snapshot() -> SnapshotId {
    SnapshotId::derive([b"snap".as_slice()])
}

struct Fixture {
    targets: Vec<Target>,
    evidence: Vec<EvidenceRecord>,
    relations: Vec<Relation>,
    signals: Vec<UpstreamSignal>,
}

impl Fixture {
    fn target(
        &mut self,
        kind: PortableTargetKind,
        name: &str,
        path: &str,
        parent: Option<&str>,
        visibility: TargetVisibility,
    ) {
        self.targets.push(Target {
            id: id(name),
            kind: TargetKind::Portable { kind },
            visibility,
            name: name.to_owned(),
            parent: parent.map(id),
            location: Some(SourceLocation {
                path: SourcePath::new(path).unwrap(),
                bytes: ByteSpan::new(0, 0).unwrap(),
                start: None,
                end: None,
            }),
            inventory: InventoryState::Represented,
            capabilities: Vec::new(),
            diagnostic: None,
        });
    }

    fn source(&mut self, name: &str, text: &str) {
        self.evidence.push(EvidenceRecord {
            id: EvidenceId::derive([b"source".as_slice(), name.as_bytes()]),
            kind: EvidenceKind::Source,
            origin: EvidenceOrigin::Direct,
            target: Some(id(name)),
            location: None,
            summary: format!("source of {name}"),
            detail: Some(text.to_owned()),
            provenance: EvidenceProvenance {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                configuration: configuration(),
                ingest_only: true,
                resolution: ResolutionQuality::Exact,
            },
        });
    }

    fn calls(&mut self, source: &str, target: &str) {
        self.relations.push(Relation {
            id: RelationId::derive([source.as_bytes(), target.as_bytes()]),
            source: id(source),
            target: id(target),
            kind: "rust:calls".to_owned(),
            provenance: RelationProvenance {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                configuration: Some(configuration()),
                ingest_only: true,
                resolution: ResolutionQuality::Inferred,
                detail: None,
            },
        });
    }

    fn signal(&mut self, name: &str, target: &str, policy: &str, accepted: bool) {
        self.signals.push(UpstreamSignal {
            finding: FindingId::derive([name.as_bytes()]),
            policy_version: policy.to_owned(),
            target: id(target),
            category: "error_handling".to_owned(),
            severity: Severity::Medium,
            confidence: Confidence::from_basis_points(8000).unwrap(),
            title: format!("{name} title"),
            summary: format!("{name} summary"),
            accepted,
        });
    }
}

fn fixture(body_padding: usize) -> Fixture {
    use PortableTargetKind::{Callable, File, Module, Package, Test};
    use TargetVisibility::{Private, Public};
    let mut fx = Fixture {
        targets: Vec::new(),
        evidence: Vec::new(),
        relations: Vec::new(),
        signals: Vec::new(),
    };
    let pad = "x".repeat(body_padding);
    fx.target(Package, "calc", "Cargo.toml", None, Public);
    fx.target(File, "src/lib.rs", "src/lib.rs", Some("calc"), Public);
    fx.target(Callable, "add", "src/lib.rs", Some("src/lib.rs"), Public);
    fx.target(Callable, "clamp", "src/lib.rs", Some("src/lib.rs"), Private);
    fx.target(Callable, "noop", "src/lib.rs", Some("src/lib.rs"), Private);
    fx.target(Module, "tests", "src/lib.rs", Some("src/lib.rs"), Private);
    fx.target(Test, "add_works", "src/lib.rs", Some("tests"), Private);
    fx.targets.push(Target {
        id: id("api"),
        kind: TargetKind::LanguageSpecific {
            language: "rust".to_owned(),
            kind: "cargo_target:test".to_owned(),
        },
        visibility: Public,
        name: "api".to_owned(),
        parent: Some(id("calc")),
        location: Some(SourceLocation {
            path: SourcePath::new("tests/api.rs").unwrap(),
            bytes: ByteSpan::new(0, 0).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    });
    fx.target(File, "tests/api.rs", "tests/api.rs", Some("api"), Public);
    fx.target(File, "src/parser.rs", "src/parser.rs", Some("calc"), Public);
    fx.target(
        Callable,
        "parse",
        "src/parser.rs",
        Some("src/parser.rs"),
        Public,
    );
    fx.source(
        "add",
        &format!("pub fn add(a: i32, b: i32) -> i32 {{ a + b }} // {pad}"),
    );
    fx.source(
        "clamp",
        &format!("fn clamp(v: i32) -> i32 {{ v.max(0) }} // {pad}"),
    );
    fx.source("noop", &format!("fn noop() {{}} // {pad}"));
    fx.source(
        "add_works",
        "#[test]\nfn add_works() { assert_eq!(add(1, 2), 3); }",
    );
    fx.source(
        "parse",
        "pub fn parse(s: &str) -> Option<i32> { s.parse().ok() }",
    );
    fx.calls("add_works", "add");
    fx.signal("clamp-finding", "clamp", "correctness-conservative@1", true);
    fx.signal(
        "parse-finding",
        "parse",
        "correctness-conservative@1",
        false,
    );
    fx
}

fn budget(max_bytes: usize) -> EvidenceBudget {
    EvidenceBudget {
        max_bytes,
        max_tokens: max_bytes / 4,
        max_items: 32,
        max_relation_depth: 0,
    }
}

fn plan(fx: &Fixture, budget: &EvidenceBudget) -> TestingReviewPlan {
    let policy = TestingApplicabilityPolicy::conservative();
    TestingReviewPlanner::new(
        &policy,
        PolicyId::derive([b"testing".as_slice()]),
        "testing-conservative@1",
    )
    .unwrap()
    .plan(
        &snapshot(),
        &configuration(),
        &fx.targets,
        &fx.evidence,
        &fx.relations,
        &fx.signals,
        budget,
    )
    .unwrap()
}

fn unit<'a>(plan: &'a TestingReviewPlan, target: &TargetId) -> &'a TestingReviewUnit {
    plan.units
        .iter()
        .find(|unit| &unit.target.target == target)
        .unwrap()
}

fn section(plan: &TestingReviewPlan, unit: &TestingReviewUnit, kind: EvidenceKind) -> String {
    plan.evidence
        .iter()
        .filter(|record| unit.evidence.contains(&record.id) && record.kind == kind)
        .map(|record| record.detail.clone().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn plans_module_package_and_project_units() {
    let fx = fixture(0);
    let plan = plan(&fx, &budget(400_000));
    let applicable = plan
        .units
        .iter()
        .filter(|unit| unit.applicability.state == ApplicabilityState::Applicable)
        .map(|unit| (unit.target.name.clone(), unit.level))
        .collect::<Vec<_>>();
    assert_eq!(applicable.len(), 4, "{applicable:?}");
    for expected in [
        ("src/lib.rs".to_owned(), TestingLevel::Unit),
        ("src/parser.rs".to_owned(), TestingLevel::Unit),
        ("calc".to_owned(), TestingLevel::Library),
        ("project".to_owned(), TestingLevel::Project),
    ] {
        assert!(applicable.contains(&expected), "missing {expected:?}");
    }
    for name in ["add", "tests", "add_works"] {
        assert_eq!(
            unit(&plan, &id(name)).applicability.state,
            ApplicabilityState::NotApplicable,
            "{name}"
        );
    }
    assert_eq!(
        unit(&plan, &id("add_works")).target.class,
        TestingTargetClass::TestCode
    );
    assert_eq!(
        unit(&plan, &id("tests")).target.class,
        TestingTargetClass::TestCode
    );
}

#[test]
fn module_unit_carries_members_linked_tests_and_signals() {
    let fx = fixture(0);
    let plan = plan(&fx, &budget(400_000));
    let lib = unit(&plan, &id("src/lib.rs"));
    for target in ["src/lib.rs", "add", "clamp", "noop", "add_works"] {
        assert!(
            lib.scope_targets.contains(&id(target)),
            "{target} not citable"
        );
    }
    let index = section(&plan, lib, EvidenceKind::StaticAnalysis);
    assert!(index.contains("add_works"), "{index}");
    assert!(index.contains("1 linked test(s)"), "{index}");
    let source = section(&plan, lib, EvidenceKind::Source);
    assert!(
        source.contains("pub fn add") && source.contains("fn clamp"),
        "{source}"
    );
    let tests = section(&plan, lib, EvidenceKind::Test);
    assert!(tests.contains("assert_eq!(add(1, 2), 3)"), "{tests}");
    let signals = section(&plan, lib, EvidenceKind::ReviewFinding);
    assert!(
        signals.contains("clamp-finding title") && signals.contains("accepted"),
        "{signals}"
    );
    assert_eq!(
        lib.upstream_signals,
        vec![FindingId::derive([b"clamp-finding".as_slice()])]
    );
    assert_eq!(lib.accepted_signals, lib.upstream_signals);

    let project = unit(&plan, &argus_workflow::testing_project_target());
    let inventory = section(&plan, project, EvidenceKind::Test);
    assert_eq!(inventory.matches("tests/api.rs").count(), 1, "{inventory}");

    let package = unit(&plan, &id("calc"));
    let summary = section(&plan, package, EvidenceKind::StaticAnalysis);
    assert!(
        summary.contains("add") && summary.contains("parse") && !summary.contains("clamp"),
        "{summary}"
    );
    assert!(
        package.upstream_signals.len() == 2,
        "correctness signals inform the library level"
    );
}

#[test]
fn oversized_module_splits_public_and_signalled_members() {
    let fx = fixture(4_000);
    let plan = plan(&fx, &budget(8_000));
    let lib = unit(&plan, &id("src/lib.rs"));
    assert_eq!(lib.applicability.state, ApplicabilityState::Applicable);
    let source = section(&plan, lib, EvidenceKind::Source);
    assert!(source.contains("members listed by signature"), "{source}");
    assert!(
        source.contains("2 public or signalled member(s)"),
        "{source}"
    );

    for name in ["add", "clamp"] {
        let member = unit(&plan, &id(name));
        assert_eq!(
            member.applicability.state,
            ApplicabilityState::Applicable,
            "{name}"
        );
        assert_eq!(member.target.split_from, Some(id("src/lib.rs")));
    }
    assert_eq!(
        unit(&plan, &id("noop")).applicability.state,
        ApplicabilityState::NotApplicable,
        "private members without signals stay in the module unit"
    );
    for record in &plan.evidence {
        assert!(record.detail.as_ref().unwrap().len() <= 8_000 / 10 * 9);
    }
}

#[test]
fn materialized_units_round_trip_through_the_queue() {
    let temporary = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(temporary.path().join("evidence")).unwrap();
    let queue = DurableQueue::open(&temporary.path().join("queue.redb")).unwrap();
    let run = RunId::derive([b"run".as_slice()]);
    queue
        .create_run(&RunRecord {
            id: run.clone(),
            snapshot: snapshot(),
            configuration: configuration(),
            state: RunState::Active,
            created_at_millis: 1,
            updated_at_millis: 1,
            finalized_at_millis: None,
        })
        .unwrap();

    let fx = fixture(0);
    let budget = budget(400_000);
    let plan = plan(&fx, &budget);
    let catalog = TestingEvidenceCatalog::ingest(
        &store,
        &snapshot(),
        DataClassification::Internal,
        &plan.evidence,
    )
    .unwrap();
    let batch = plan
        .materialize_admissible(
            &store,
            &catalog,
            &snapshot(),
            &configuration(),
            &budget,
            DataClassification::Internal,
        )
        .unwrap();
    assert_eq!(batch.materializations.len(), 4);
    let admitted = batch
        .admit(&queue, &run, &snapshot(), &configuration(), "workspace", 2)
        .unwrap();
    assert_eq!(admitted, 4);

    let records = queue.run_records(&run).unwrap();
    let mut levels = Vec::new();
    for work in &records.work {
        assert_eq!(work.coverage.policy, "testing-conservative@1");
        let admission: TestingReviewAdmission = serde_json::from_slice(&work.payload).unwrap();
        let restored = TestingReviewMaterialization::restore(&queue, &admission).unwrap();
        assert_eq!(restored.unit, admission.unit);
        levels.push(restored.contract.level());
    }
    levels.sort();
    assert_eq!(
        levels,
        vec![
            TestingLevel::Unit,
            TestingLevel::Unit,
            TestingLevel::Library,
            TestingLevel::Project
        ]
    );
}

#[test]
fn contract_binds_module_drafts_and_rejects_mismatched_events() {
    use argus_workflow::PolicyAssessmentContract;
    let temporary = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(temporary.path().join("evidence")).unwrap();
    let fx = fixture(0);
    let budget = budget(400_000);
    let plan = plan(&fx, &budget);
    let catalog = TestingEvidenceCatalog::ingest(
        &store,
        &snapshot(),
        DataClassification::Internal,
        &plan.evidence,
    )
    .unwrap();
    let lib = unit(&plan, &id("src/lib.rs"))
        .materialize(
            &store,
            &catalog,
            &snapshot(),
            &configuration(),
            budget,
            DataClassification::Internal,
        )
        .unwrap();
    let contract = lib.contract.as_ref();
    let schema = contract.schema();
    assert_eq!(schema["properties"]["dimensions"]["minItems"], 5);

    let dimensions = [
        "behavioral_correctness",
        "error_and_failure_paths",
        "boundary_and_edge_inputs",
        "state_and_concurrency",
        "regression_protection",
    ]
    .map(|dimension| {
        serde_json::json!({
            "dimension": dimension,
            "status": if dimension == "regression_protection" { "gap" } else { "adequate" },
            "rationale": "Judged from the linked tests.",
            "citations": []
        })
    });
    let assessment = serde_json::json!({
        "dimensions": dimensions,
        "result": {
            "state": "candidate_findings",
            "findings": [{
                "title": "clamp has no regression test",
                "description": "An accepted correctness finding on clamp has no test.",
                "subject": id("clamp").to_string(),
                "recommended_level": "unit",
                "severity": "low",
                "confidence_basis_points": 8000,
                "dimensions": ["regression_protection"],
                "existing_tests": [id("add_works").to_string()],
                "upstream_signals": [FindingId::derive([b"clamp-finding".as_slice()]).to_string()],
                "outline": ["clamp maps negative input to zero"],
                "citations": []
            }]
        }
    });
    contract
        .validate("review.candidate_found", &assessment)
        .unwrap();
    assert!(contract.validate("review.pass", &assessment).is_err());
    let bound = lib.contract.bind_output(&assessment).unwrap();
    let argus_policies::TestingResult::CandidateFindings { findings } = bound.result else {
        panic!("expected findings");
    };
    assert_eq!(
        findings[0].severity,
        Severity::High,
        "accepted signal raises severity"
    );

    let mut foreign = assessment.clone();
    foreign["result"]["findings"][0]["subject"] = serde_json::json!(id("parse").to_string());
    assert!(
        contract
            .validate("review.candidate_found", &foreign)
            .is_err(),
        "parse is outside this unit"
    );
}
