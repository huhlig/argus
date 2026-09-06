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

use crate::{
    AdapterIdentity, AdapterInventory, AdapterProvider, DiscoveryPartition, LanguageAdapter,
    ProviderRole, SourceAccess,
};
use argus_core::{
    ByteSpan, Capability, CapabilityStatus, InventoryState, PortableTargetKind, SourceLocation,
    SourcePath, Target, TargetId, TargetKind,
};

/// Operational mode for synthetic adapters used in testing and simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyntheticMode {
    /// Normal operation: emits complete inventory without errors.
    Complete,
    /// Simulates a conflict between providers.
    Conflict,
    /// Simulates partial capability availability with diagnostic message.
    Partial,
    /// Simulates entirely unavailable language capabilities.
    Unavailable,
    /// Simulates a failed discovery partition.
    Failed,
    /// Emits malformed inventory data pointing outside the snapshot.
    Malformed,
    /// Injects an unhandled crash/invariant error during discovery.
    Crash,
}

/// Test double implementing [`LanguageAdapter`] with configurable synthetic behaviors.
#[derive(Clone, Debug)]
pub struct SyntheticAdapter {
    /// Adapter identifier name.
    pub name: String,
    /// Simulation mode governing discovery outcome.
    pub mode: SyntheticMode,
    /// Synthetic source file path to emit as target location.
    pub path: SourcePath,
}

impl SyntheticAdapter {
    /// Creates a new synthetic adapter with the given name, mode, and target source path.
    pub fn new(name: impl Into<String>, mode: SyntheticMode, path: SourcePath) -> Self {
        Self {
            name: name.into(),
            mode,
            path,
        }
    }
}

impl LanguageAdapter for SyntheticAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: self.name.clone(),
            version: "synthetic-1".to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![AdapterProvider {
            identity: format!("{}:syntax", self.name),
            role: ProviderRole::Syntax,
            capabilities: vec!["declarations".to_owned()],
        }]
    }

    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError> {
        if self.mode == SyntheticMode::Crash {
            return Err(argus_core::ArgusError::invariant(
                "injected synthetic adapter crash",
            ));
        }
        let status = match self.mode {
            SyntheticMode::Complete | SyntheticMode::Conflict | SyntheticMode::Malformed => {
                CapabilityStatus::Complete
            }
            SyntheticMode::Partial => CapabilityStatus::Partial,
            SyntheticMode::Unavailable => CapabilityStatus::Unavailable,
            SyntheticMode::Failed => CapabilityStatus::Failed,
            SyntheticMode::Crash => unreachable!(),
        };
        let detail = matches!(status, CapabilityStatus::Partial | CapabilityStatus::Failed)
            .then_some("injected synthetic capability gap");
        let id = TargetId::derive([
            self.name.as_bytes(),
            self.path.as_str().as_bytes(),
            b"module",
        ]);
        let location_path = if self.mode == SyntheticMode::Malformed {
            SourcePath::new("missing/outside.rs")?
        } else {
            self.path.clone()
        };
        Ok(AdapterInventory {
            adapter: self.identity(),
            snapshot: source.snapshot_id().clone(),
            partitions: vec![DiscoveryPartition {
                name: "syntax".to_owned(),
                status,
                diagnostic: detail.map(str::to_owned),
            }],
            targets: vec![Target {
                id: id.clone(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Module,
                },
                visibility: argus_core::TargetVisibility::Unknown,
                name: format!("{}-module", self.name),
                parent: None,
                location: Some(SourceLocation {
                    path: location_path,
                    bytes: ByteSpan::new(0, 0)?,
                    start: None,
                    end: None,
                }),
                inventory: InventoryState::Represented,
                capabilities: vec![Capability {
                    name: "syntax".to_owned(),
                    status,
                    detail: detail.map(str::to_owned),
                    provider: Some(format!("{}:syntax", self.name)),
                }],
                diagnostic: None,
            }],
            evidence: Vec::new(),
            relations: vec![],
            conflicts: (self.mode == SyntheticMode::Conflict)
                .then(|| crate::ConflictRecord {
                    subject: id.to_string(),
                    providers: vec![
                        format!("{}:syntax-a", self.name),
                        format!("{}:syntax-b", self.name),
                    ],
                    detail: "synthetic providers disagree".to_owned(),
                })
                .into_iter()
                .collect(),
        })
    }
}
