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

use crate::{DataClassification, EvidenceBudget, PackageArtifact};
use argus_core::{ContentHash, EvidenceKind, TargetId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Request submitted by a review actor to expand available evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRequest {
    /// 1-based sequential request index within the current review session.
    pub sequence: u32,
    /// Content hash of the baseline package being expanded.
    pub base_package: ContentHash,
    /// Specific targets for which additional evidence is requested.
    pub requested_targets: BTreeSet<TargetId>,
    /// Categories of evidence being sought.
    pub requested_kinds: BTreeSet<EvidenceKind>,
    /// Incremental budget requested for the expansion.
    pub additional_budget: EvidenceBudget,
    /// Justification explaining why this evidence is required.
    pub rationale: String,
}

/// Policy constraints governing evidence expansion eligibility and budget limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceExpansionPolicy {
    /// Maximum number of expansion requests allowed per review run.
    pub max_requests: u32,
    /// Cumulative maximum budget across all approved expansion requests.
    pub cumulative_budget: EvidenceBudget,
    /// Targets permitted for expansion requests.
    pub allowed_targets: BTreeSet<TargetId>,
    /// Evidence kinds permitted for expansion requests.
    pub allowed_kinds: BTreeSet<EvidenceKind>,
    /// Maximum data classification allowed in expansions.
    pub maximum_classification: DataClassification,
}

/// Running tally of approved expansion requests and consumed budget.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpansionUsage {
    /// Number of approved expansion requests so far.
    pub approved_requests: u32,
    /// Cumulative bytes allocated across approved expansions.
    pub used_bytes: usize,
    /// Cumulative tokens allocated across approved expansions.
    pub used_tokens: usize,
    /// Cumulative item count allocated across approved expansions.
    pub used_items: usize,
    /// Maximum relation traversal depth reached across approved expansions.
    pub maximum_relation_depth: u32,
}

/// Reason why an evidence expansion request was rejected by policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpansionDenialReason {
    /// Request fields are empty or have invalid zero limits.
    InvalidRequest,
    /// Request references a package hash that does not match current state.
    WrongPackage,
    /// Request sequence number does not match expected counter.
    OutOfSequence,
    /// Maximum number of expansion requests has been exceeded.
    RequestLimitExhausted,
    /// Target is not within the policy-permitted scope.
    TargetOutsideScope,
    /// Evidence kind is not permitted by expansion policy.
    EvidenceKindOutsideScope,
    /// Cumulative byte budget limit exceeded.
    ByteBudgetExhausted,
    /// Cumulative token budget limit exceeded.
    TokenBudgetExhausted,
    /// Cumulative item count budget limit exceeded.
    ItemBudgetExhausted,
    /// Permitted relation traversal depth exceeded.
    RelationDepthExhausted,
}

/// Formal denial record when an evidence expansion request is rejected.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpansionDenial {
    /// Specific policy violation or budget exhaustion reason.
    pub reason: ExpansionDenialReason,
    /// Diagnostic explanation.
    pub detail: String,
}

/// Authorization record granted when an evidence expansion request complies with policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizedEvidenceExpansion {
    /// Content hash digest of the authorization record.
    pub authorization_hash: ContentHash,
    /// The approved evidence request.
    pub request: EvidenceRequest,
    /// Maximum data classification permitted for the expansion.
    pub maximum_classification: DataClassification,
    /// Updated cumulative usage state after incorporating this approval.
    pub next_usage: ExpansionUsage,
}

/// Evaluator that validates evidence requests against policy and enforces budget tracking.
pub struct EvidenceRequestAuthorizer;

impl EvidenceRequestAuthorizer {
    /// Evaluates an expansion request against policy constraints and current usage.
    ///
    /// # Errors
    ///
    /// Returns [`ExpansionDenial`] if the request violates policy or exceeds budget limits.
    pub fn authorize(
        base: &PackageArtifact,
        request: EvidenceRequest,
        policy: &EvidenceExpansionPolicy,
        usage: &ExpansionUsage,
    ) -> Result<AuthorizedEvidenceExpansion, ExpansionDenial> {
        validate_request(base, &request, policy, usage)?;
        let additional = &request.additional_budget;
        let next_usage = ExpansionUsage {
            approved_requests: usage.approved_requests.saturating_add(1),
            used_bytes: usage.used_bytes.saturating_add(additional.max_bytes),
            used_tokens: usage.used_tokens.saturating_add(additional.max_tokens),
            used_items: usage.used_items.saturating_add(additional.max_items),
            maximum_relation_depth: usage
                .maximum_relation_depth
                .max(additional.max_relation_depth),
        };
        let authorization_bytes =
            serde_json::to_vec(&(&request, policy.maximum_classification, &next_usage)).map_err(
                |error| denial(ExpansionDenialReason::InvalidRequest, error.to_string()),
            )?;
        Ok(AuthorizedEvidenceExpansion {
            authorization_hash: ContentHash::digest(&authorization_bytes),
            request,
            maximum_classification: policy.maximum_classification,
            next_usage,
        })
    }
}

fn validate_request(
    base: &PackageArtifact,
    request: &EvidenceRequest,
    policy: &EvidenceExpansionPolicy,
    usage: &ExpansionUsage,
) -> Result<(), ExpansionDenial> {
    if request.rationale.trim().is_empty()
        || request.requested_targets.is_empty()
        || request.requested_kinds.is_empty()
        || request.additional_budget.max_bytes == 0
        || request.additional_budget.max_tokens == 0
        || request.additional_budget.max_items == 0
    {
        return Err(denial(
            ExpansionDenialReason::InvalidRequest,
            "request requires rationale, targets, evidence kinds, and positive size limits",
        ));
    }
    if request.base_package != base.hash {
        return Err(denial(
            ExpansionDenialReason::WrongPackage,
            "request does not reference the active evidence package",
        ));
    }
    if request.sequence != usage.approved_requests.saturating_add(1) {
        return Err(denial(
            ExpansionDenialReason::OutOfSequence,
            "request sequence does not follow approved expansion usage",
        ));
    }
    if usage.approved_requests >= policy.max_requests {
        return Err(denial(
            ExpansionDenialReason::RequestLimitExhausted,
            "evidence expansion request limit exhausted",
        ));
    }
    if !request.requested_targets.is_subset(&policy.allowed_targets) {
        return Err(denial(
            ExpansionDenialReason::TargetOutsideScope,
            "request names a target outside the policy envelope",
        ));
    }
    if !request.requested_kinds.is_subset(&policy.allowed_kinds) {
        return Err(denial(
            ExpansionDenialReason::EvidenceKindOutsideScope,
            "request names an evidence kind outside the policy envelope",
        ));
    }
    check_limit(
        usage.used_bytes,
        request.additional_budget.max_bytes,
        policy.cumulative_budget.max_bytes,
        ExpansionDenialReason::ByteBudgetExhausted,
        "evidence expansion byte budget exhausted",
    )?;
    check_limit(
        usage.used_tokens,
        request.additional_budget.max_tokens,
        policy.cumulative_budget.max_tokens,
        ExpansionDenialReason::TokenBudgetExhausted,
        "evidence expansion token budget exhausted",
    )?;
    check_limit(
        usage.used_items,
        request.additional_budget.max_items,
        policy.cumulative_budget.max_items,
        ExpansionDenialReason::ItemBudgetExhausted,
        "evidence expansion item budget exhausted",
    )?;
    if request.additional_budget.max_relation_depth > policy.cumulative_budget.max_relation_depth {
        return Err(denial(
            ExpansionDenialReason::RelationDepthExhausted,
            "evidence expansion relation-depth limit exhausted",
        ));
    }
    Ok(())
}

fn check_limit(
    used: usize,
    requested: usize,
    maximum: usize,
    reason: ExpansionDenialReason,
    detail: &'static str,
) -> Result<(), ExpansionDenial> {
    if requested > maximum.saturating_sub(used) {
        Err(denial(reason, detail))
    } else {
        Ok(())
    }
}

fn denial(reason: ExpansionDenialReason, detail: impl Into<String>) -> ExpansionDenial {
    ExpansionDenial {
        reason,
        detail: detail.into(),
    }
}
