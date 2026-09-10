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

use argus_core::{CapabilityStatus, EvidenceRecord, Relation, SnapshotId, SourcePath, Target};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Unique identity and version for a language adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdapterIdentity {
    /// Human-readable adapter name (e.g. "rust", "python").
    pub name: String,
    /// Adapter implementation version.
    pub version: String,
}

/// Functional role played by a provider within an adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRole {
    /// Discovers project configuration, workspace layout, or package manifests.
    Project,
    /// Performs syntactic parsing and AST construction.
    Syntax,
    /// Performs semantic analysis, type checking, and symbol resolution.
    Semantic,
    /// Interfaces with the build system or compiler output.
    Build,
    /// Discovers or wraps external tool integrations (linters, analyzers).
    Tool,
    /// Discovers cross-target relationships and dependency edges.
    Relationship,
}

/// Description of an underlying capability provider bundled within an adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdapterProvider {
    /// Identity string of the provider.
    pub identity: String,
    /// Functional role of this provider.
    pub role: ProviderRole,
    /// Capabilities exposed by this provider (e.g. "declarations", "types").
    pub capabilities: Vec<String>,
}

/// Read-only access abstraction to source files within a snapshot.
pub trait SourceAccess: Send + Sync {
    /// Returns the snapshot identifier backing this source view.
    fn snapshot_id(&self) -> &SnapshotId;
    /// Checks whether the snapshot contains a file at the given relative path.
    fn contains(&self, path: &SourcePath) -> bool;
    /// Reads the raw file bytes for a relative path.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist or reading fails.
    fn read(&self, path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError>;
}

/// Common trait implemented by language-specific code analyzers.
///
/// Adapters inspect source code within a snapshot and produce an [`AdapterInventory`]
/// containing targets, relations, discovery partitions, and evidence.
pub trait LanguageAdapter: Send + Sync {
    /// Returns the identity of this adapter.
    fn identity(&self) -> AdapterIdentity;
    /// Lists the internal providers bundled within this adapter.
    fn providers(&self) -> Vec<AdapterProvider>;
    /// Discovers targets, relations, and capabilities across the snapshot.
    ///
    /// # Errors
    ///
    /// Returns an [`argus_core::ArgusError`] if inventory generation fails.
    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError>;

    /// Streams inventory discovery items into an [`InventorySink`].
    ///
    /// The default implementation calls [`inventory`](Self::inventory) and feeds the sink sequentially.
    ///
    /// # Errors
    ///
    /// Returns an error if inventory discovery or sink operations fail.
    fn inventory_into(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<(), argus_core::ArgusError> {
        let inventory = self.inventory(source)?;
        sink.begin(inventory.adapter, inventory.snapshot)?;
        for partition in inventory.partitions {
            sink.partition(partition)?;
        }
        for target in inventory.targets {
            sink.target(target)?;
        }
        for evidence in inventory.evidence {
            sink.evidence(evidence)?;
        }
        for relation in inventory.relations {
            sink.relation(relation)?;
        }
        for conflict in inventory.conflicts {
            sink.conflict(conflict)?;
        }
        sink.finish()
    }
}

/// Streaming sink interface for receiving inventory discovery events.
pub trait InventorySink {
    /// Initializes the sink with adapter identity and snapshot.
    fn begin(
        &mut self,
        adapter: AdapterIdentity,
        snapshot: SnapshotId,
    ) -> Result<(), argus_core::ArgusError>;
    /// Emits a discovery partition record.
    fn partition(&mut self, partition: DiscoveryPartition) -> Result<(), argus_core::ArgusError>;
    /// Emits a discovered target.
    fn target(&mut self, target: Target) -> Result<(), argus_core::ArgusError>;
    /// Emits an evidence record. Default implementation is a no-op.
    fn evidence(&mut self, _evidence: EvidenceRecord) -> Result<(), argus_core::ArgusError> {
        Ok(())
    }
    /// Emits a relation edge between targets.
    fn relation(&mut self, relation: Relation) -> Result<(), argus_core::ArgusError>;
    /// Emits a conflict detected during discovery.
    fn conflict(&mut self, conflict: ConflictRecord) -> Result<(), argus_core::ArgusError>;
    /// Finalizes the inventory sink.
    fn finish(&mut self) -> Result<(), argus_core::ArgusError>;
}

/// In-memory sink that collects streamed inventory items into an [`AdapterInventory`].
#[derive(Default)]
pub struct CollectingInventorySink {
    adapter: Option<AdapterIdentity>,
    snapshot: Option<SnapshotId>,
    partitions: Vec<DiscoveryPartition>,
    targets: Vec<Target>,
    evidence: Vec<EvidenceRecord>,
    relations: Vec<Relation>,
    conflicts: Vec<ConflictRecord>,
}

impl CollectingInventorySink {
    /// Consumes the sink and constructs the consolidated [`AdapterInventory`].
    ///
    /// # Errors
    ///
    /// Returns an error if [`InventorySink::begin`] was never called.
    pub fn into_inventory(self) -> Result<AdapterInventory, argus_core::ArgusError> {
        Ok(AdapterInventory {
            adapter: self.adapter.ok_or_else(|| {
                argus_core::ArgusError::invariant("inventory sink was not started")
            })?,
            snapshot: self.snapshot.ok_or_else(|| {
                argus_core::ArgusError::invariant("inventory sink has no snapshot")
            })?,
            partitions: self.partitions,
            targets: self.targets,
            evidence: self.evidence,
            relations: self.relations,
            conflicts: self.conflicts,
        })
    }
}

impl InventorySink for CollectingInventorySink {
    fn begin(
        &mut self,
        adapter: AdapterIdentity,
        snapshot: SnapshotId,
    ) -> Result<(), argus_core::ArgusError> {
        if self.adapter.is_some() || self.snapshot.is_some() {
            return Err(argus_core::ArgusError::invariant(
                "inventory sink was started more than once",
            ));
        }
        self.adapter = Some(adapter);
        self.snapshot = Some(snapshot);
        Ok(())
    }

    fn partition(&mut self, partition: DiscoveryPartition) -> Result<(), argus_core::ArgusError> {
        self.partitions.push(partition);
        Ok(())
    }

    fn target(&mut self, target: Target) -> Result<(), argus_core::ArgusError> {
        self.targets.push(target);
        Ok(())
    }

    fn relation(&mut self, relation: Relation) -> Result<(), argus_core::ArgusError> {
        self.relations.push(relation);
        Ok(())
    }

    fn evidence(&mut self, evidence: EvidenceRecord) -> Result<(), argus_core::ArgusError> {
        self.evidence.push(evidence);
        Ok(())
    }

    fn conflict(&mut self, conflict: ConflictRecord) -> Result<(), argus_core::ArgusError> {
        self.conflicts.push(conflict);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), argus_core::ArgusError> {
        Ok(())
    }
}

/// Capability status and diagnostics for a partition of discovery work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryPartition {
    /// Human-readable name of the partition (e.g. subsystem or file group).
    pub name: String,
    /// Capability status reported for this partition.
    pub status: CapabilityStatus,
    /// Optional diagnostic message when status is partial or failed.
    pub diagnostic: Option<String>,
}

/// Record of an unresolved conflict or ambiguity between discovery providers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictRecord {
    /// Target or symbol subject to the conflict.
    pub subject: String,
    /// Names of providers involved in the conflict.
    pub providers: Vec<String>,
    /// Detailed description of the disagreement or ambiguity.
    pub detail: String,
}

/// Complete discovery output produced by a language adapter for a snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdapterInventory {
    /// Identity of the adapter that produced this inventory.
    pub adapter: AdapterIdentity,
    /// Snapshot against which this inventory was generated.
    pub snapshot: SnapshotId,
    /// Capability partitions evaluated during discovery.
    pub partitions: Vec<DiscoveryPartition>,
    /// Discovered targets (functions, structs, modules, etc.).
    pub targets: Vec<Target>,
    /// Discovered evidence records attached to targets.
    #[serde(default)]
    pub evidence: Vec<EvidenceRecord>,
    /// Discovered semantic or syntactic relations between targets.
    pub relations: Vec<Relation>,
    /// Ambiguities or conflicting interpretations encountered across providers.
    pub conflicts: Vec<ConflictRecord>,
}

/// Validates, sorts, and canonicalizes an [`AdapterInventory`] against the source snapshot.
///
/// Ensures target IDs are unique and reference valid snapshot files, validates all relations
/// connect existing targets, and orders targets, relations, evidence, and partitions deterministically.
///
/// # Errors
///
/// Returns [`argus_core::ArgusError`] if:
/// - The inventory's snapshot does not match the source view.
/// - Duplicate target IDs or invalid locations are encountered.
/// - Relations refer to unknown target IDs.
/// - Evidence references unknown targets or external paths.
/// - Partition records are empty or lack diagnostics on failure.
pub fn normalize_inventory(
    source: &dyn SourceAccess,
    mut inventory: AdapterInventory,
) -> Result<AdapterInventory, argus_core::ArgusError> {
    if inventory.snapshot != *source.snapshot_id() {
        return Err(argus_core::ArgusError::invariant(
            "adapter inventory references the wrong snapshot",
        ));
    }
    let mut ids = BTreeSet::new();
    for target in &inventory.targets {
        target.validate()?;
        if !ids.insert(target.id.clone()) {
            return Err(argus_core::ArgusError::invariant(
                "duplicate adapter target ID",
            ));
        }
        if target
            .location
            .as_ref()
            .is_some_and(|location| !source.contains(&location.path))
        {
            return Err(argus_core::ArgusError::invariant(
                "adapter target references source outside its snapshot",
            ));
        }
    }
    for relation in &inventory.relations {
        if !ids.contains(&relation.source) || !ids.contains(&relation.target) {
            return Err(argus_core::ArgusError::invariant(
                "adapter relation references an unknown target",
            ));
        }
    }
    let mut evidence_ids = BTreeSet::new();
    for evidence in &inventory.evidence {
        evidence.validate()?;
        if !evidence_ids.insert(evidence.id.clone())
            || evidence
                .target
                .as_ref()
                .is_some_and(|target| !ids.contains(target))
            || evidence
                .location
                .as_ref()
                .is_some_and(|location| !source.contains(&location.path))
        {
            return Err(argus_core::ArgusError::invariant(
                "adapter evidence identity or source mapping is invalid",
            ));
        }
    }
    for partition in &inventory.partitions {
        if partition.name.trim().is_empty()
            || matches!(
                partition.status,
                CapabilityStatus::Partial | CapabilityStatus::Failed
            ) && partition.diagnostic.as_deref().is_none_or(str::is_empty)
        {
            return Err(argus_core::ArgusError::invariant(
                "invalid discovery partition",
            ));
        }
    }
    inventory.targets.sort_by(|a, b| a.id.cmp(&b.id));
    inventory.relations.sort_by(|a, b| a.id.cmp(&b.id));
    inventory.evidence.sort_by(|a, b| a.id.cmp(&b.id));
    inventory.partitions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(inventory)
}
