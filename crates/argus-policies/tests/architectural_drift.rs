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

use argus_core::{DesignArtifactId, TargetId, TargetVisibility};
use argus_policies::{
    AcceptedDriftRecord, AcceptedDriftRegistry, ArchitecturalDriftAnalyzer,
    ConformanceDisposition, EvolutionClassifier, EvolutionKind, TargetRevisionDelta,
};

#[test]
fn evolution_classification_distinguishes_defects_from_evolution() {
    // 1. Defect
    let defect = EvolutionClassifier::classify(
        "Memory leak in buffer pool breaks invariant",
        None,
        false,
    )
    .expect("classification succeeded");
    assert_eq!(defect.kind, EvolutionKind::Defect);
    assert_eq!(defect.disposition, ConformanceDisposition::FixImplementation);

    // 2. Accidental Divergence
    let divergence = EvolutionClassifier::classify(
        "Module bypassed cache layer without documentation",
        None,
        false,
    )
    .expect("classification succeeded");
    assert_eq!(divergence.kind, EvolutionKind::AccidentalDivergence);
    assert_eq!(
        divergence.disposition,
        ConformanceDisposition::FixImplementation
    );

    // 3. Valid Evolution with breaking change -> CreateSupersedingAdr
    let mut delta_breaking = TargetRevisionDelta::new(
        TargetId::derive([b"crate".as_slice(), b"engine".as_slice()]),
        "engine",
        "rev-2",
        TargetVisibility::Public,
    );
    delta_breaking.has_breaking_api_change = true;
    delta_breaking.verified_by_tests = true;

    let evolution_breaking = EvolutionClassifier::classify(
        "Systemic architecture change with intentional redesign and verified tests",
        Some(&delta_breaking),
        true,
    )
    .expect("classification succeeded");
    assert_eq!(evolution_breaking.kind, EvolutionKind::ValidEvolution);
    assert_eq!(
        evolution_breaking.disposition,
        ConformanceDisposition::CreateSupersedingAdr
    );

    // 4. Valid Evolution non-breaking -> AmendExistingAdr
    let mut delta_non_breaking = TargetRevisionDelta::new(
        TargetId::derive([b"crate".as_slice(), b"engine".as_slice()]),
        "engine",
        "rev-2",
        TargetVisibility::Public,
    );
    delta_non_breaking.has_breaking_api_change = false;
    delta_non_breaking.verified_by_tests = true;

    let evolution_non_breaking = EvolutionClassifier::classify(
        "Intentional evolution refining implementation details",
        Some(&delta_non_breaking),
        true,
    )
    .expect("classification succeeded");
    assert_eq!(evolution_non_breaking.kind, EvolutionKind::ValidEvolution);
    assert_eq!(
        evolution_non_breaking.disposition,
        ConformanceDisposition::AmendExistingAdr
    );
}

#[test]
fn accepted_drift_registry_lifecycle_and_expiration() {
    let mut registry = AcceptedDriftRegistry::new();
    assert!(registry.is_empty());

    let target_id = TargetId::derive([b"crate".as_slice(), b"legacy_adapter".as_slice()]);
    let artifact_id = DesignArtifactId::derive([b"adr-0001".as_slice()]);

    let record = AcceptedDriftRecord {
        id: "drift-001".to_string(),
        target_id: target_id.clone(),
        artifact_id: artifact_id.clone(),
        owner: "lead-architect".to_string(),
        rationale: "Temporary adapter during Phase 14 migration".to_string(),
        accepted_at: "2026-01-01".to_string(),
        review_date: Some("2026-06-01".to_string()),
    };

    registry.register(record).expect("valid record registered");
    assert_eq!(registry.len(), 1);

    // Before expiration: accepted
    assert!(registry
        .is_accepted(&target_id, &artifact_id, "2026-05-01")
        .is_some());

    // After expiration: not accepted
    assert!(registry
        .is_accepted(&target_id, &artifact_id, "2026-07-01")
        .is_none());

    // Target mismatch
    let other_target = TargetId::derive([b"crate".as_slice(), b"other".as_slice()]);
    assert!(registry
        .is_accepted(&other_target, &artifact_id, "2026-05-01")
        .is_none());
}

#[test]
fn architectural_drift_analyzer_suppresses_accepted_drift_and_detects_prohibitions() {
    let analyzer = ArchitecturalDriftAnalyzer::new();
    let mut registry = AcceptedDriftRegistry::new();

    let target_id = TargetId::derive([b"crate".as_slice(), b"storage".as_slice()]);
    let artifact_id = DesignArtifactId::derive([b"adr-0002".as_slice()]);

    let mut delta = TargetRevisionDelta::new(
        target_id.clone(),
        "storage",
        "rev-2",
        TargetVisibility::Public,
    );
    delta.added_dependencies.insert("tokio".to_string());

    let governing_rules = vec!["Prohibit direct tokio dependency in core storage".to_string()];

    // 1. Without accepted drift, prohibited dependency triggers drift finding
    let result = analyzer
        .analyze_drift(
            &delta,
            &artifact_id,
            &governing_rules,
            &registry,
            "2026-05-01",
        )
        .expect("analysis succeeded");
    assert!(result.is_some());
    let classification = result.unwrap();
    assert_eq!(
        classification.disposition,
        ConformanceDisposition::FixImplementation
    );

    // 2. Register accepted drift
    let record = AcceptedDriftRecord {
        id: "drift-storage-tokio".to_string(),
        target_id: target_id.clone(),
        artifact_id: artifact_id.clone(),
        owner: "storage-team".to_string(),
        rationale: "Approved temporary async benchmark harness".to_string(),
        accepted_at: "2026-01-01".to_string(),
        review_date: Some("2026-12-31".to_string()),
    };
    registry.register(record).expect("registered");

    // 3. With accepted drift, finding is suppressed
    let suppressed = analyzer
        .analyze_drift(
            &delta,
            &artifact_id,
            &governing_rules,
            &registry,
            "2026-05-01",
        )
        .expect("analysis succeeded");
    assert!(suppressed.is_none());
}

#[test]
fn derived_finding_id_is_stable_and_deterministic() {
    let target = TargetId::derive([b"crate".as_slice(), b"target".as_slice()]);
    let artifact = DesignArtifactId::derive([b"adr-0001".as_slice()]);
    let dimension = "architectural-drift";

    let id1 = ArchitecturalDriftAnalyzer::derive_stable_finding_id(&target, &artifact, dimension);
    let id2 = ArchitecturalDriftAnalyzer::derive_stable_finding_id(&target, &artifact, dimension);
    assert_eq!(id1, id2);

    let other_dimension = "constraint-conformance";
    let id3 = ArchitecturalDriftAnalyzer::derive_stable_finding_id(&target, &artifact, other_dimension);
    assert_ne!(id1, id3);
}
