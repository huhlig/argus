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

//! Architectural drift detection, revision evolution classification, and accepted drift registry.

use crate::conformance::ConformanceDisposition;
use argus_core::{ArgusError, DesignArtifactId, FindingId, TargetId, TargetVisibility};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Classification of the nature of an architectural divergence.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionKind {
    /// Direct implementation defect or broken invariant that violates design constraints.
    Defect,
    /// Accidental or unreviewed divergence from governing design documents.
    AccidentalDivergence,
    /// Deliberate, systematic architectural maturation that justifies amending or superseding the ADR.
    ValidEvolution,
}

/// Evaluated classification of an architectural discrepancy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvolutionClassification {
    /// Category of evolution.
    pub kind: EvolutionKind,
    /// Recommended disposition for resolution.
    pub disposition: ConformanceDisposition,
    /// High-level rationale explaining the classification.
    pub rationale: String,
    /// Concrete justification explaining why the divergence is a valid evolution or defect.
    pub justification: String,
}

/// Delta representing structural changes to a code target across revisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TargetRevisionDelta {
    /// Target identifier.
    pub target_id: TargetId,
    /// Qualified target name.
    pub target_name: String,
    /// Baseline revision hash or identifier, if known.
    pub baseline_revision: Option<String>,
    /// Current revision hash or identifier.
    pub current_revision: String,
    /// Dependencies added since baseline.
    pub added_dependencies: BTreeSet<String>,
    /// Dependencies removed since baseline.
    pub removed_dependencies: BTreeSet<String>,
    /// Previous target visibility, if changed.
    pub previous_visibility: Option<TargetVisibility>,
    /// Current target visibility.
    pub current_visibility: TargetVisibility,
    /// Signatures or interfaces added or altered.
    pub interface_mutations: Vec<String>,
    /// Whether the changes introduce a breaking API change.
    pub has_breaking_api_change: bool,
    /// Associated test verification coverage present for the modifications.
    pub verified_by_tests: bool,
}

impl TargetRevisionDelta {
    /// Creates a new revision delta with required target metadata.
    #[must_use]
    pub fn new(
        target_id: TargetId,
        target_name: impl Into<String>,
        current_revision: impl Into<String>,
        current_visibility: TargetVisibility,
    ) -> Self {
        Self {
            target_id,
            target_name: target_name.into(),
            baseline_revision: None,
            current_revision: current_revision.into(),
            added_dependencies: BTreeSet::new(),
            removed_dependencies: BTreeSet::new(),
            previous_visibility: None,
            current_visibility,
            interface_mutations: Vec::new(),
            has_breaking_api_change: false,
            verified_by_tests: false,
        }
    }
}

/// Classifier distinguishing code defects from valid architectural evolution.
pub struct EvolutionClassifier;

impl EvolutionClassifier {
    /// Classifies an observed discrepancy between a governing design artifact and target implementation.
    ///
    /// Evaluates whether the change represents an accidental divergence (which must be fixed in code)
    /// or a valid architectural evolution (which justifies superseding or amending the ADR without silent rewrites).
    pub fn classify(
        discrepancy: &str,
        delta: Option<&TargetRevisionDelta>,
        governing_adr_is_historical: bool,
    ) -> Result<EvolutionClassification, ArgusError> {
        if discrepancy.trim().is_empty() {
            return Err(ArgusError::invalid_input("discrepancy description cannot be empty"));
        }

        let disc_lower = discrepancy.to_ascii_lowercase();

        // 1. Direct defect indicators: broken invariants, crash conditions, memory/resource leaks
        if disc_lower.contains("defect")
            || disc_lower.contains("leak")
            || disc_lower.contains("broken invariant")
            || disc_lower.contains("breaks invariant")
            || disc_lower.contains("crash")
            || disc_lower.contains("panic")
            || disc_lower.contains("data corruption")
            || disc_lower.contains("unhandled error")
        {
            return Ok(EvolutionClassification {
                kind: EvolutionKind::Defect,
                disposition: ConformanceDisposition::FixImplementation,
                rationale: "Implementation violates fundamental safety or integrity invariant".to_owned(),
                justification: "The code bypasses established design constraints and represents a defect that must be corrected in implementation.".to_owned(),
            });
        }

        // 2. Valid architectural evolution: well-tested deliberate changes, replacement of obsolete protocols
        let is_deliberate_and_tested = delta.is_some_and(|d| {
            d.verified_by_tests
                && (!d.added_dependencies.is_empty()
                    || !d.interface_mutations.is_empty()
                    || d.has_breaking_api_change)
        });
        let introduces_new_paradigm = disc_lower.contains("replaced")
            || disc_lower.contains("migrated")
            || disc_lower.contains("substituted")
            || disc_lower.contains("new transport")
            || disc_lower.contains("redesign")
            || disc_lower.contains("evolution")
            || disc_lower.contains("throughput")
            || disc_lower.contains("performance optimization");

        if governing_adr_is_historical && (is_deliberate_and_tested || introduces_new_paradigm) {
            let is_fundamental_shift = disc_lower.contains("replaced")
                || disc_lower.contains("migrated")
                || disc_lower.contains("new architecture")
                || delta.is_some_and(|d| d.has_breaking_api_change);
            let disposition = if is_fundamental_shift {
                ConformanceDisposition::CreateSupersedingAdr
            } else {
                ConformanceDisposition::AmendExistingAdr
            };

            let justification = if is_fundamental_shift {
                "The implementation represents a deliberate, verified architectural evolution that outgrew the original decision. A new superseding ADR must be authored; existing historical ADRs must not be silently rewritten."
            } else {
                "The implementation evolution enhances capability while remaining compatible with original design intent. The existing ADR should be amended with an update record."
            };

            return Ok(EvolutionClassification {
                kind: EvolutionKind::ValidEvolution,
                disposition,
                rationale: "Deliberate architectural maturation with test verification".to_owned(),
                justification: justification.to_owned(),
            });
        }

        // 3. Accidental divergence: uncoordinated structural drift without clear rationale
        Ok(EvolutionClassification {
            kind: EvolutionKind::AccidentalDivergence,
            disposition: ConformanceDisposition::FixImplementation,
            rationale: "Accidental divergence from governing architectural specification".to_owned(),
            justification: "Changes diverge from declared design intent without documented justification or comprehensive test-backed architectural intent.".to_owned(),
        })
    }
}

/// An approved, documented record of intentional architectural drift.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AcceptedDriftRecord {
    /// Deterministic identifier derived from target and artifact signatures.
    pub id: String,
    /// Code target exhibiting intentional drift.
    pub target_id: TargetId,
    /// Governing design artifact being diverged from.
    pub artifact_id: DesignArtifactId,
    /// Designated owner/architect who reviewed and approved the drift.
    pub owner: String,
    /// Justification explaining why the drift is accepted.
    pub rationale: String,
    /// ISO 8601 date string when drift was formally accepted.
    pub accepted_at: String,
    /// Optional expiration or required re-evaluation date (YYYY-MM-DD).
    pub review_date: Option<String>,
}

impl AcceptedDriftRecord {
    /// Validates required fields.
    pub fn validate(&self) -> Result<(), ArgusError> {
        if self.owner.trim().is_empty() {
            return Err(ArgusError::invalid_input("accepted drift owner cannot be empty"));
        }
        if self.rationale.trim().is_empty() {
            return Err(ArgusError::invalid_input("accepted drift rationale cannot be empty"));
        }
        if self.accepted_at.trim().is_empty() {
            return Err(ArgusError::invalid_input("accepted drift acceptance date cannot be empty"));
        }
        Ok(())
    }

    /// Evaluates whether the accepted drift has expired relative to a reference date string.
    #[must_use]
    pub fn is_expired(&self, current_date: &str) -> bool {
        if let Some(rev) = &self.review_date {
            current_date > rev.as_str()
        } else {
            false
        }
    }
}

/// Registry managing accepted intentional drift records and suppression evaluation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AcceptedDriftRegistry {
    records: BTreeMap<String, AcceptedDriftRecord>,
}

impl AcceptedDriftRegistry {
    /// Creates an empty accepted drift registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of registered drift records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns `true` if registry contains no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Registers a validated accepted drift record.
    pub fn register(&mut self, record: AcceptedDriftRecord) -> Result<(), ArgusError> {
        record.validate()?;
        self.records.insert(record.id.clone(), record);
        Ok(())
    }

    /// Retrieves an accepted drift record by its ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&AcceptedDriftRecord> {
        self.records.get(id)
    }

    /// Checks if active (unexpired) accepted drift exists for a given target and governing artifact.
    #[must_use]
    pub fn is_accepted(
        &self,
        target_id: &TargetId,
        artifact_id: &DesignArtifactId,
        current_date: &str,
    ) -> Option<&AcceptedDriftRecord> {
        self.records.values().find(|r| {
            &r.target_id == target_id && &r.artifact_id == artifact_id && !r.is_expired(current_date)
        })
    }

    /// Returns all registered drift records.
    pub fn records(&self) -> impl Iterator<Item = &AcceptedDriftRecord> {
        self.records.values()
    }
}

/// Multi-revision analyzer evaluating architectural drift and candidate findings.
#[derive(Clone, Debug, Default)]
pub struct ArchitecturalDriftAnalyzer;

impl ArchitecturalDriftAnalyzer {
    /// Constructs a new drift analyzer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Evaluates structural changes between revisions against governing design requirements.
    pub fn analyze_drift(
        &self,
        delta: &TargetRevisionDelta,
        artifact_id: &DesignArtifactId,
        governing_rules: &[String],
        drift_registry: &AcceptedDriftRegistry,
        current_date: &str,
    ) -> Result<Option<EvolutionClassification>, ArgusError> {
        // If drift is already explicitly accepted and not expired, suppress finding
        if drift_registry
            .is_accepted(&delta.target_id, artifact_id, current_date)
            .is_some()
        {
            return Ok(None);
        }

        // Check if delta adds unauthorized or conflicting dependencies
        let mut violations = Vec::new();
        for dep in &delta.added_dependencies {
            let dep_lower = dep.to_ascii_lowercase();
            for rule in governing_rules {
                let rule_lower = rule.to_ascii_lowercase();
                if rule_lower.contains("prohibit") && rule_lower.contains(&dep_lower) {
                    violations.push(format!("Added prohibited dependency '{dep}'"));
                }
            }
        }

        if !violations.is_empty() {
            let discrepancy = violations.join("; ");
            let classification = EvolutionClassifier::classify(&discrepancy, Some(delta), true)?;
            return Ok(Some(classification));
        }

        // Check if significant interface mutation occurred without test coverage
        if !delta.interface_mutations.is_empty() && !delta.verified_by_tests {
            let discrepancy = format!(
                "Interface mutated ({}) without corresponding verification tests",
                delta.interface_mutations.join(", ")
            );
            let classification = EvolutionClassifier::classify(&discrepancy, Some(delta), false)?;
            return Ok(Some(classification));
        }

        Ok(None)
    }

    /// Derives a stable finding signature that persists across repeated scans.
    #[must_use]
    pub fn derive_stable_finding_id(
        target_id: &TargetId,
        artifact_id: &DesignArtifactId,
        dimension_slug: &str,
    ) -> FindingId {
        FindingId::derive([
            target_id.as_str().as_bytes(),
            artifact_id.as_str().as_bytes(),
            dimension_slug.as_bytes(),
        ])
    }
}
