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

//! Canonical review fingerprints for deterministic assessment reuse and invalidation.

use crate::{ArgusError, ContentHash, PolicyId, SnapshotId, TargetId};
use serde::{Deserialize, Serialize};

/// Detailed discrepancy reason when comparing two review fingerprints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FingerprintDifference {
    /// Associated snapshots differ.
    SnapshotMismatch {
        baseline: SnapshotId,
        current: SnapshotId,
    },
    /// Fingerprinted targets differ.
    TargetMismatch {
        baseline: TargetId,
        current: TargetId,
    },
    /// Policies differ.
    PolicyMismatch {
        baseline: PolicyId,
        current: PolicyId,
    },
    /// Target declaration or signature modified.
    TargetDeclarationChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Executable implementation body modified.
    ImplementationChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Associated documentation comments modified.
    DocumentationChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Downstream dependencies (callees or referenced types) modified.
    DownstreamDependencyChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Upstream call structure (callers, call-sites, or invocation patterns) modified.
    UpstreamCallStructureChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Documented behavioral contracts along the call tree (preconditions, safety, errors, panics) modified.
    CallTreeDocumentedBehaviorChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Associated tests or verification suites modified.
    TestsChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Linked design documents, ADRs, or specifications modified.
    DesignChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Semantic policy definition version updated.
    PolicyVersionChanged { baseline: String, current: String },
    /// Review prompt template version updated.
    PromptVersionChanged { baseline: String, current: String },
    /// Workflow state machine definition modified.
    WorkflowHashChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
    /// Review actor versions changed.
    ActorVersionsChanged {
        baseline: Vec<String>,
        current: Vec<String>,
    },
    /// Model reuse class altered.
    ModelReuseClassChanged { baseline: String, current: String },
    /// Evidence builder implementation version changed.
    EvidenceBuilderVersionChanged { baseline: String, current: String },
    /// Evidence extension versions changed.
    ExtensionVersionsChanged {
        baseline: Vec<String>,
        current: Vec<String>,
    },
    /// Compiler or analysis toolchain changed.
    ToolchainHashChanged {
        baseline: ContentHash,
        current: ContentHash,
    },
}

/// Exhaustive content-addressed fingerprint capturing all inputs to a review assessment.
///
/// When all constituent hashes and environment versions match, a prior review assessment
/// may be reused safely without executing model evaluation again.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewFingerprint {
    /// Associated audit snapshot identifier.
    pub snapshot: SnapshotId,
    /// Target identifier being fingerprinted.
    pub target: TargetId,
    /// Policy identifier.
    pub policy: PolicyId,
    /// Digest of the target's declaration and signature.
    pub target_hash: ContentHash,
    /// Digest of the target's executable implementation body.
    pub implementation_hash: ContentHash,
    /// Digest of doc comments and linked documentation.
    pub documentation_hash: ContentHash,
    /// Digest of downstream dependencies (callees and referenced types).
    pub downstream_dependency_hash: ContentHash,
    /// Digest of upstream call structure (calling targets, call-site signatures, invocation patterns).
    pub upstream_call_structure_hash: ContentHash,
    /// Digest of documented behavioral contracts along the call tree (preconditions, safety, errors, panics).
    pub call_tree_behavior_hash: ContentHash,
    /// Digest of associated test functions and fixtures.
    pub test_hash: ContentHash,
    /// Digest of linked design documents (ADRs, specs, requirements).
    pub design_hash: ContentHash,
    /// Semantic policy identity and version.
    pub policy_version: String,
    /// Review prompt template identity and version.
    pub prompt_version: String,
    /// Digest of the workflow state machine definition.
    pub workflow_hash: ContentHash,
    /// Actor implementation versions participating in review.
    pub actor_versions: Vec<String>,
    /// Model reuse class (e.g. deterministic, fast-tier, frontier-tier).
    pub model_reuse_class: String,
    /// Evidence builder implementation version.
    pub evidence_builder_version: String,
    /// Evidence extension versions.
    pub extension_versions: Vec<String>,
    /// Analysis toolchain and compiler hash.
    pub toolchain_hash: ContentHash,
}

impl ReviewFingerprint {
    /// Computes the deterministic composite BLAKE3 digest of this fingerprint.
    #[must_use]
    pub fn composite_hash(&self) -> ContentHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"argus:review_fingerprint:v1\0");
        hasher.update(self.snapshot.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.target.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.policy.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.target_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.implementation_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.documentation_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.downstream_dependency_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.upstream_call_structure_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.call_tree_behavior_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.test_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.design_hash.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.policy_version.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.prompt_version.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.workflow_hash.as_str().as_bytes());
        hasher.update(b"\0");
        for actor in &self.actor_versions {
            hasher.update(actor.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(b"\xFF");
        hasher.update(self.model_reuse_class.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.evidence_builder_version.as_bytes());
        hasher.update(b"\0");
        for ext in &self.extension_versions {
            hasher.update(ext.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(b"\xFF");
        hasher.update(self.toolchain_hash.as_str().as_bytes());
        ContentHash::digest(hasher.finalize().as_bytes())
    }

    /// Validates internal consistency and non-emptiness of required fields.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if required identifiers or versions are empty.
    pub fn validate(&self) -> Result<(), ArgusError> {
        if self.policy_version.trim().is_empty() {
            return Err(ArgusError::invalid_input("policy version is required"));
        }
        if self.prompt_version.trim().is_empty() {
            return Err(ArgusError::invalid_input("prompt version is required"));
        }
        if self.model_reuse_class.trim().is_empty() {
            return Err(ArgusError::invalid_input("model reuse class is required"));
        }
        if self.evidence_builder_version.trim().is_empty() {
            return Err(ArgusError::invalid_input(
                "evidence builder version is required",
            ));
        }
        Ok(())
    }

    /// Compares two fingerprints and returns all detailed differences.
    #[must_use]
    pub fn diff(&self, other: &Self) -> Vec<FingerprintDifference> {
        let mut diffs = Vec::new();
        self.diff_identities(other, &mut diffs);
        self.diff_content_hashes(other, &mut diffs);
        self.diff_environment(other, &mut diffs);
        diffs
    }

    fn diff_identities(&self, other: &Self, diffs: &mut Vec<FingerprintDifference>) {
        if self.snapshot != other.snapshot {
            diffs.push(FingerprintDifference::SnapshotMismatch {
                baseline: self.snapshot.clone(),
                current: other.snapshot.clone(),
            });
        }
        if self.target != other.target {
            diffs.push(FingerprintDifference::TargetMismatch {
                baseline: self.target.clone(),
                current: other.target.clone(),
            });
        }
        if self.policy != other.policy {
            diffs.push(FingerprintDifference::PolicyMismatch {
                baseline: self.policy.clone(),
                current: other.policy.clone(),
            });
        }
    }

    fn diff_content_hashes(&self, other: &Self, diffs: &mut Vec<FingerprintDifference>) {
        if self.target_hash != other.target_hash {
            diffs.push(FingerprintDifference::TargetDeclarationChanged {
                baseline: self.target_hash.clone(),
                current: other.target_hash.clone(),
            });
        }
        if self.implementation_hash != other.implementation_hash {
            diffs.push(FingerprintDifference::ImplementationChanged {
                baseline: self.implementation_hash.clone(),
                current: other.implementation_hash.clone(),
            });
        }
        if self.documentation_hash != other.documentation_hash {
            diffs.push(FingerprintDifference::DocumentationChanged {
                baseline: self.documentation_hash.clone(),
                current: other.documentation_hash.clone(),
            });
        }
        if self.downstream_dependency_hash != other.downstream_dependency_hash {
            diffs.push(FingerprintDifference::DownstreamDependencyChanged {
                baseline: self.downstream_dependency_hash.clone(),
                current: other.downstream_dependency_hash.clone(),
            });
        }
        if self.upstream_call_structure_hash != other.upstream_call_structure_hash {
            diffs.push(FingerprintDifference::UpstreamCallStructureChanged {
                baseline: self.upstream_call_structure_hash.clone(),
                current: other.upstream_call_structure_hash.clone(),
            });
        }
        if self.call_tree_behavior_hash != other.call_tree_behavior_hash {
            diffs.push(FingerprintDifference::CallTreeDocumentedBehaviorChanged {
                baseline: self.call_tree_behavior_hash.clone(),
                current: other.call_tree_behavior_hash.clone(),
            });
        }
        if self.test_hash != other.test_hash {
            diffs.push(FingerprintDifference::TestsChanged {
                baseline: self.test_hash.clone(),
                current: other.test_hash.clone(),
            });
        }
        if self.design_hash != other.design_hash {
            diffs.push(FingerprintDifference::DesignChanged {
                baseline: self.design_hash.clone(),
                current: other.design_hash.clone(),
            });
        }
    }

    fn diff_environment(&self, other: &Self, diffs: &mut Vec<FingerprintDifference>) {
        if self.policy_version != other.policy_version {
            diffs.push(FingerprintDifference::PolicyVersionChanged {
                baseline: self.policy_version.clone(),
                current: other.policy_version.clone(),
            });
        }
        if self.prompt_version != other.prompt_version {
            diffs.push(FingerprintDifference::PromptVersionChanged {
                baseline: self.prompt_version.clone(),
                current: other.prompt_version.clone(),
            });
        }
        if self.workflow_hash != other.workflow_hash {
            diffs.push(FingerprintDifference::WorkflowHashChanged {
                baseline: self.workflow_hash.clone(),
                current: other.workflow_hash.clone(),
            });
        }
        if self.actor_versions != other.actor_versions {
            diffs.push(FingerprintDifference::ActorVersionsChanged {
                baseline: self.actor_versions.clone(),
                current: other.actor_versions.clone(),
            });
        }
        if self.model_reuse_class != other.model_reuse_class {
            diffs.push(FingerprintDifference::ModelReuseClassChanged {
                baseline: self.model_reuse_class.clone(),
                current: other.model_reuse_class.clone(),
            });
        }
        if self.evidence_builder_version != other.evidence_builder_version {
            diffs.push(FingerprintDifference::EvidenceBuilderVersionChanged {
                baseline: self.evidence_builder_version.clone(),
                current: other.evidence_builder_version.clone(),
            });
        }
        if self.extension_versions != other.extension_versions {
            diffs.push(FingerprintDifference::ExtensionVersionsChanged {
                baseline: self.extension_versions.clone(),
                current: other.extension_versions.clone(),
            });
        }
        if self.toolchain_hash != other.toolchain_hash {
            diffs.push(FingerprintDifference::ToolchainHashChanged {
                baseline: self.toolchain_hash.clone(),
                current: other.toolchain_hash.clone(),
            });
        }
    }
}
