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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdapterIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRole {
    Project,
    Syntax,
    Semantic,
    Build,
    Tool,
    Relationship,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdapterProvider {
    pub identity: String,
    pub role: ProviderRole,
    pub capabilities: Vec<String>,
}

pub trait SourceAccess: Send + Sync {
    fn snapshot_id(&self) -> &SnapshotId;
    fn contains(&self, path: &SourcePath) -> bool;
    fn read(&self, path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError>;
}

pub trait LanguageAdapter: Send + Sync {
    fn identity(&self) -> AdapterIdentity;
    fn providers(&self) -> Vec<AdapterProvider>;
    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError>;

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

pub trait InventorySink {
    fn begin(
        &mut self,
        adapter: AdapterIdentity,
        snapshot: SnapshotId,
    ) -> Result<(), argus_core::ArgusError>;
    fn partition(&mut self, partition: DiscoveryPartition) -> Result<(), argus_core::ArgusError>;
    fn target(&mut self, target: Target) -> Result<(), argus_core::ArgusError>;
    fn evidence(&mut self, _evidence: EvidenceRecord) -> Result<(), argus_core::ArgusError> {
        Ok(())
    }
    fn relation(&mut self, relation: Relation) -> Result<(), argus_core::ArgusError>;
    fn conflict(&mut self, conflict: ConflictRecord) -> Result<(), argus_core::ArgusError>;
    fn finish(&mut self) -> Result<(), argus_core::ArgusError>;
}

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryPartition {
    pub name: String,
    pub status: CapabilityStatus,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub subject: String,
    pub providers: Vec<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdapterInventory {
    pub adapter: AdapterIdentity,
    pub snapshot: SnapshotId,
    pub partitions: Vec<DiscoveryPartition>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRecord>,
    pub relations: Vec<Relation>,
    pub conflicts: Vec<ConflictRecord>,
}

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
