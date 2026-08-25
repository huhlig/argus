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

use crate::{DataClassification, EVIDENCE_SCHEMA_VERSION, EvidenceStore};
use argus_core::{ConfigurationId, ContentHash, EvidenceKind, PolicyId, SnapshotId, TargetId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateAvailability {
    Available,
    Summarized,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceCandidate {
    pub hash: Option<ContentHash>,
    pub kind: EvidenceKind,
    pub priority: u16,
    pub relation_depth: u32,
    pub estimated_tokens: usize,
    pub availability: CandidateAvailability,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceBudget {
    pub max_bytes: usize,
    pub max_tokens: usize,
    pub max_items: usize,
    pub max_relation_depth: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyEvidenceRequirements {
    pub allowed_kinds: BTreeSet<EvidenceKind>,
    pub required_kinds: BTreeSet<EvidenceKind>,
    pub maximum_classification: DataClassification,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceDisposition {
    Included,
    Summarized,
    Partial,
    OmittedBudget,
    OmittedPolicy,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidencePackageItem {
    pub hash: Option<ContentHash>,
    pub kind: EvidenceKind,
    pub disposition: EvidenceDisposition,
    pub canonical_bytes: usize,
    pub estimated_tokens: usize,
    pub relation_depth: u32,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidencePackage {
    pub schema_version: u32,
    pub revision: u32,
    pub previous_package: Option<ContentHash>,
    pub snapshot: SnapshotId,
    pub configuration: ConfigurationId,
    pub target: TargetId,
    pub policy: PolicyId,
    pub policy_version: String,
    pub budget: EvidenceBudget,
    pub used_bytes: usize,
    pub used_tokens: usize,
    pub items: Vec<EvidencePackageItem>,
    pub unsatisfied_requirements: Vec<EvidenceKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageArtifact {
    pub hash: ContentHash,
    pub package: EvidencePackage,
}

impl PackageArtifact {
    pub fn validate_identity(&self) -> Result<(), argus_core::ArgusError> {
        let bytes = serde_json::to_vec(&self.package).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize evidence package")
                .with_source(error)
        })?;
        if ContentHash::digest(&bytes) != self.hash {
            return Err(argus_core::ArgusError::invariant(
                "evidence package hash mismatch",
            ));
        }
        Ok(())
    }
}

pub struct EvidencePackageBuilder<'a> {
    store: &'a EvidenceStore,
}

impl<'a> EvidencePackageBuilder<'a> {
    #[must_use]
    pub const fn new(store: &'a EvidenceStore) -> Self {
        Self { store }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn build(
        &self,
        revision: u32,
        snapshot: SnapshotId,
        configuration: ConfigurationId,
        target: TargetId,
        policy: PolicyId,
        policy_version: impl Into<String>,
        budget: EvidenceBudget,
        requirements: &PolicyEvidenceRequirements,
        candidates: Vec<EvidenceCandidate>,
    ) -> Result<PackageArtifact, argus_core::ArgusError> {
        self.build_revision(
            revision,
            None,
            snapshot,
            configuration,
            target,
            policy,
            policy_version,
            budget,
            requirements,
            candidates,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn build_revision(
        &self,
        revision: u32,
        previous_package: Option<ContentHash>,
        snapshot: SnapshotId,
        configuration: ConfigurationId,
        target: TargetId,
        policy: PolicyId,
        policy_version: impl Into<String>,
        budget: EvidenceBudget,
        requirements: &PolicyEvidenceRequirements,
        mut candidates: Vec<EvidenceCandidate>,
    ) -> Result<PackageArtifact, argus_core::ArgusError> {
        let policy_version = policy_version.into();
        let invalid_revision = revision == 0
            || (revision == 1 && previous_package.is_some())
            || (revision > 1 && previous_package.is_none());
        if invalid_revision || policy_version.trim().is_empty() {
            return Err(argus_core::ArgusError::invalid_input(
                "evidence package revision chain and policy version are invalid",
            ));
        }
        if !requirements
            .required_kinds
            .is_subset(&requirements.allowed_kinds)
        {
            return Err(argus_core::ArgusError::invalid_input(
                "required evidence kinds must be allowed by policy",
            ));
        }
        candidates.sort_by(|left, right| {
            requirements
                .required_kinds
                .contains(&right.kind)
                .cmp(&requirements.required_kinds.contains(&left.kind))
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| {
                    left.hash
                        .as_ref()
                        .map_or("", ContentHash::as_str)
                        .cmp(right.hash.as_ref().map_or("", ContentHash::as_str))
                })
        });

        let mut items = Vec::with_capacity(candidates.len());
        let mut used_bytes = 0usize;
        let mut used_tokens = 0usize;
        let mut included_kinds = BTreeSet::new();
        let mut included_count = 0usize;
        for candidate in candidates {
            let mut item = EvidencePackageItem {
                hash: candidate.hash.clone(),
                kind: candidate.kind,
                disposition: EvidenceDisposition::Unavailable,
                canonical_bytes: 0,
                estimated_tokens: candidate.estimated_tokens,
                relation_depth: candidate.relation_depth,
                reason: candidate.reason,
            };
            if candidate.availability == CandidateAvailability::Unavailable {
                if candidate.hash.is_some() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "unavailable evidence cannot reference stored content",
                    ));
                }
                item.reason
                    .get_or_insert_with(|| "evidence producer reported unavailable".to_owned());
                items.push(item);
                continue;
            }
            let hash = candidate.hash.as_ref().ok_or_else(|| {
                argus_core::ArgusError::invalid_input("available evidence requires a content hash")
            })?;
            let stored = self.store.get(hash)?;
            item.canonical_bytes = stored.canonical_bytes;
            if stored.envelope.snapshot != snapshot
                || stored.envelope.record.provenance.configuration != configuration
            {
                return Err(argus_core::ArgusError::invariant(
                    "evidence candidate is outside the package snapshot or configuration",
                ));
            }
            if stored.envelope.record.kind != candidate.kind {
                return Err(argus_core::ArgusError::invariant(
                    "evidence candidate kind does not match stored content",
                ));
            }
            if !requirements.allowed_kinds.contains(&candidate.kind)
                || stored.envelope.classification > requirements.maximum_classification
            {
                item.disposition = EvidenceDisposition::OmittedPolicy;
                item.reason
                    .get_or_insert_with(|| "outside policy evidence envelope".to_owned());
            } else if candidate.relation_depth > budget.max_relation_depth
                || included_count >= budget.max_items
                || used_bytes.saturating_add(stored.canonical_bytes) > budget.max_bytes
                || used_tokens.saturating_add(candidate.estimated_tokens) > budget.max_tokens
            {
                item.disposition = EvidenceDisposition::OmittedBudget;
                item.reason
                    .get_or_insert_with(|| "evidence package budget exhausted".to_owned());
            } else {
                item.disposition = match candidate.availability {
                    CandidateAvailability::Available => EvidenceDisposition::Included,
                    CandidateAvailability::Summarized => EvidenceDisposition::Summarized,
                    CandidateAvailability::Partial => EvidenceDisposition::Partial,
                    CandidateAvailability::Unavailable => unreachable!(),
                };
                used_bytes = used_bytes.saturating_add(stored.canonical_bytes);
                used_tokens = used_tokens.saturating_add(candidate.estimated_tokens);
                included_count += 1;
                included_kinds.insert(candidate.kind);
            }
            items.push(item);
        }
        items.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| {
                    left.hash
                        .as_ref()
                        .map_or("", ContentHash::as_str)
                        .cmp(right.hash.as_ref().map_or("", ContentHash::as_str))
                })
                .then_with(|| left.disposition.cmp(&right.disposition))
        });
        let unsatisfied_requirements = requirements
            .required_kinds
            .difference(&included_kinds)
            .copied()
            .collect();
        let package = EvidencePackage {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            revision,
            previous_package,
            snapshot,
            configuration,
            target,
            policy,
            policy_version,
            budget,
            used_bytes,
            used_tokens,
            items,
            unsatisfied_requirements,
        };
        let bytes = serde_json::to_vec(&package).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize evidence package")
                .with_source(error)
        })?;
        Ok(PackageArtifact {
            hash: ContentHash::digest(&bytes),
            package,
        })
    }
}
