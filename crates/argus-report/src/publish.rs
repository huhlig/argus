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

//! Durable issue publication for GitHub and Beads issue trackers with
//! cryptographic publication receipts and idempotency tracking.

use crate::differential::DifferentialFinding;
use argus_core::{ArgusError, FindingId, RunId, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Schema version for publication receipts.
pub const PUBLICATION_RECEIPT_SCHEMA_VERSION: u32 = 1;

/// External issue tracker destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationTarget {
    /// Beads decentralized repository-native issue tracker (`bd`).
    Beads,
    /// GitHub Issues API / CLI (`gh`).
    GitHub,
}

impl std::fmt::Display for PublicationTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Beads => write!(f, "beads"),
            Self::GitHub => write!(f, "github"),
        }
    }
}

/// Status of an individual finding within a publication receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationStatus {
    /// Successfully published as a new issue.
    Published,
    /// Skipped because the finding content-hash was already published in a prior receipt.
    SkippedDuplicate,
    /// Dry run preview, no mutation performed.
    DryRun,
}

/// Cryptographic publication receipt item for an individual finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicationReceiptItem {
    /// Canonical cluster finding identifier.
    pub finding_id: FindingId,
    /// Blake3 content hash of finding invariants (policy, title, location, description).
    pub content_hash: String,
    /// Originating review policy.
    pub policy: String,
    /// Finding severity.
    pub severity: Severity,
    /// Finding title.
    pub title: String,
    /// Discovered external issue identifier or reference (e.g. bead id or GitHub #num).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_issue_id: Option<String>,
    /// Publication outcome.
    pub status: PublicationStatus,
}

/// Durable, content-addressed cryptographic receipt for publication runs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicationReceipt {
    /// Receipt schema version.
    pub schema_version: u32,
    /// Review run identifier.
    pub run_id: RunId,
    /// Target issue tracker.
    pub target: PublicationTarget,
    /// Publication timestamp in epoch milliseconds.
    pub published_at_millis: u64,
    /// Cryptographic Blake3 digest covering run_id, target, timestamp, and all item content hashes.
    pub receipt_digest: String,
    /// Total findings evaluated.
    pub total_findings: usize,
    /// Count of published items.
    pub published_count: usize,
    /// Count of skipped duplicate items.
    pub skipped_count: usize,
    /// Individual receipt items.
    pub items: Vec<PublicationReceiptItem>,
}

impl PublicationReceipt {
    /// Serialize receipt to pretty-formatted JSON string.
    pub fn to_json_pretty(&self) -> Result<String, ArgusError> {
        serde_json::to_string_pretty(self).map_err(|e| {
            ArgusError::invariant("cannot serialize publication receipt to json").with_source(e)
        })
    }

    /// Verify cryptographic integrity of this publication receipt.
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        let expected = compute_receipt_digest(
            &self.run_id,
            self.target,
            self.published_at_millis,
            &self.items,
        );
        self.receipt_digest == expected
    }
}

/// Calculate a stable content hash for a review finding.
#[must_use]
pub fn compute_finding_content_hash(finding: &DifferentialFinding) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(finding.policy.as_bytes());
    hasher.update(b"\x00");
    hasher.update(finding.title.as_bytes());
    hasher.update(b"\x00");
    if let Some(ref loc) = finding.primary_location {
        hasher.update(loc.as_bytes());
    }
    hasher.update(b"\x00");
    hasher.update(finding.description.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Calculate the cryptographic Blake3 digest for a publication receipt.
#[must_use]
pub fn compute_receipt_digest(
    run_id: &RunId,
    target: PublicationTarget,
    published_at_millis: u64,
    items: &[PublicationReceiptItem],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(run_id.as_str().as_bytes());
    hasher.update(b"\x00");
    hasher.update(target.to_string().as_bytes());
    hasher.update(b"\x00");
    hasher.update(&published_at_millis.to_le_bytes());
    hasher.update(b"\x00");

    for item in items {
        hasher.update(item.finding_id.as_str().as_bytes());
        hasher.update(b":");
        hasher.update(item.content_hash.as_bytes());
        hasher.update(b":");
        hasher.update(format!("{:?}", item.status).as_bytes());
        hasher.update(b"\x00");
    }

    hasher.finalize().to_hex().to_string()
}

/// Map Argus severity to Beads priority (0-4).
#[must_use]
pub fn severity_to_beads_priority(sev: Severity) -> u8 {
    match sev {
        Severity::Critical => 0,
        Severity::High => 1,
        Severity::Medium => 2,
        Severity::Low => 3,
        Severity::Note => 4,
    }
}

/// Format a `bd create` command for a finding.
#[must_use]
pub fn format_beads_create_command(finding: &DifferentialFinding) -> String {
    let escaped_title = finding.title.replace('"', "\\\"");
    let loc_str = finding.primary_location.as_deref().unwrap_or("unspecified");
    let targets_str = if finding.targets.is_empty() {
        "none".to_owned()
    } else {
        finding
            .targets
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let body = format!(
        "Policy: {}\nSeverity: {:?}\nLocation: {}\nTargets: {}\nCluster: {}\nDimensions: {}\n\n{}",
        finding.policy,
        finding.severity,
        loc_str,
        targets_str,
        finding.id,
        finding.dimensions.join(", "),
        finding.description
    );
    let escaped_body = body.replace('"', "\\\"");

    format!(
        "bd create \"{}\" --type task --priority {} --description \"{}\"",
        escaped_title,
        severity_to_beads_priority(finding.severity),
        escaped_body
    )
}

/// Format a `gh issue create` command for a finding.
#[must_use]
pub fn format_github_issue_command(finding: &DifferentialFinding) -> String {
    let escaped_title = format!("[Argus] {}", finding.title).replace('"', "\\\"");
    let loc_str = finding.primary_location.as_deref().unwrap_or("unspecified");
    let targets_str = if finding.targets.is_empty() {
        "none".to_owned()
    } else {
        finding
            .targets
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let body = format!(
        "### Finding Details\n\n- **Policy**: `{}`\n- **Severity**: `{:?}`\n- **Location**: `{}`\n- **Targets**: {}\n- **Cluster ID**: `{}`\n- **Confidence**: {:.1}%\n\n### Description\n\n{}\n",
        finding.policy,
        finding.severity,
        loc_str,
        targets_str,
        finding.id,
        f64::from(finding.confidence.basis_points()) / 100.0,
        finding.description
    );
    let escaped_body = body.replace('"', "\\\"");

    format!(
        "gh issue create --title \"{}\" --label \"argus,{}\" --body \"{}\"",
        escaped_title,
        finding.policy.to_lowercase(),
        escaped_body
    )
}

/// Evaluates findings for publication against prior publication receipts,
/// ensuring strict idempotency and generating the cryptographic receipt.
#[must_use]
pub fn prepare_publication(
    run_id: &RunId,
    target: PublicationTarget,
    findings: &[DifferentialFinding],
    prior_receipts: &[PublicationReceipt],
    dry_run: bool,
    force: bool,
    now_millis: u64,
) -> (PublicationReceipt, String) {
    let mut already_published_hashes = BTreeSet::new();
    if !force {
        for receipt in prior_receipts {
            if receipt.target == target {
                for item in &receipt.items {
                    if item.status == PublicationStatus::Published {
                        already_published_hashes.insert(item.content_hash.clone());
                    }
                }
            }
        }
    }

    let mut receipt_items = Vec::with_capacity(findings.len());
    let mut payload_commands = Vec::new();
    let mut published_count = 0;
    let mut skipped_count = 0;

    match target {
        PublicationTarget::Beads => {
            payload_commands.push(format!(
                "# Beads issue publication commands for Argus run {run_id}"
            ));
            payload_commands
                .push("# Run these commands to import findings into project tracker:".to_owned());
            payload_commands.push(String::new());
        }
        PublicationTarget::GitHub => {
            payload_commands.push(format!(
                "# GitHub issue publication commands for Argus run {run_id}"
            ));
            payload_commands.push("# Run these commands with the GitHub CLI (gh):".to_owned());
            payload_commands.push(String::new());
        }
    }

    for finding in findings {
        let content_hash = compute_finding_content_hash(finding);

        if !force && already_published_hashes.contains(&content_hash) {
            skipped_count += 1;
            receipt_items.push(PublicationReceiptItem {
                finding_id: finding.id.clone(),
                content_hash,
                policy: finding.policy.clone(),
                severity: finding.severity,
                title: finding.title.clone(),
                external_issue_id: None,
                status: PublicationStatus::SkippedDuplicate,
            });
            continue;
        }

        let status = if dry_run {
            PublicationStatus::DryRun
        } else {
            PublicationStatus::Published
        };

        if status == PublicationStatus::Published {
            published_count += 1;
        }

        let cmd = match target {
            PublicationTarget::Beads => format_beads_create_command(finding),
            PublicationTarget::GitHub => format_github_issue_command(finding),
        };
        payload_commands.push(cmd);

        receipt_items.push(PublicationReceiptItem {
            finding_id: finding.id.clone(),
            content_hash,
            policy: finding.policy.clone(),
            severity: finding.severity,
            title: finding.title.clone(),
            external_issue_id: None,
            status,
        });
    }

    let receipt_digest = compute_receipt_digest(run_id, target, now_millis, &receipt_items);

    let receipt = PublicationReceipt {
        schema_version: PUBLICATION_RECEIPT_SCHEMA_VERSION,
        run_id: run_id.clone(),
        target,
        published_at_millis: now_millis,
        receipt_digest,
        total_findings: findings.len(),
        published_count,
        skipped_count,
        items: receipt_items,
    };

    (receipt, payload_commands.join("\n"))
}
