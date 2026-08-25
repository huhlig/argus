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
    ConfigurationId, Relation, RelationId, RelationProvenance, ResolutionQuality, Target, TargetId,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const DEFAULT_PROVIDER: &str = "rust-semantic-export";
const DEFAULT_PROVIDER_VERSION: &str = "1";

/// Relationships accepted from a captured Rust semantic-tool export.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InputKind {
    Reference,
    Call,
    Type,
    Implementation,
}

impl InputKind {
    const fn namespaced(self) -> &'static str {
        match self {
            Self::Reference => "rust:references",
            Self::Call => "rust:calls",
            Self::Type => "rust:has_type",
            Self::Implementation => "rust:implements",
        }
    }
}

#[derive(Debug, Deserialize)]
struct InputRelationship {
    source: TargetId,
    target: TargetId,
    kind: InputKind,
    resolution: ResolutionQuality,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RejectedRelationship {
    pub line: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RustRelationshipInventory {
    pub relations: Vec<Relation>,
    pub rejected: Vec<RejectedRelationship>,
}

/// Ingests newline-delimited, captured semantic relationships without executing tools.
pub struct RustRelationshipProvider {
    configuration: ConfigurationId,
    provider: String,
    provider_version: String,
}

impl RustRelationshipProvider {
    #[must_use]
    pub fn new(configuration: ConfigurationId) -> Self {
        Self {
            configuration,
            provider: DEFAULT_PROVIDER.to_owned(),
            provider_version: DEFAULT_PROVIDER_VERSION.to_owned(),
        }
    }

    #[must_use]
    pub fn ingest(&self, json_lines: &[u8], targets: &[Target]) -> RustRelationshipInventory {
        let known = targets
            .iter()
            .map(|target| target.id.clone())
            .collect::<BTreeSet<_>>();
        let mut relations = BTreeMap::new();
        let mut rejected = Vec::new();

        for (offset, raw) in json_lines.split(|byte| *byte == b'\n').enumerate() {
            let line = offset + 1;
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let input = match serde_json::from_slice::<InputRelationship>(raw) {
                Ok(input) => input,
                Err(error) => {
                    rejected.push(RejectedRelationship {
                        line,
                        reason: format!("invalid relationship JSON: {error}"),
                    });
                    continue;
                }
            };
            if !known.contains(&input.source) || !known.contains(&input.target) {
                rejected.push(RejectedRelationship {
                    line,
                    reason: "relationship references an unknown target".to_owned(),
                });
                continue;
            }
            if matches!(
                input.resolution,
                ResolutionQuality::FileFallback | ResolutionQuality::Unmapped
            ) {
                rejected.push(RejectedRelationship {
                    line,
                    reason: "semantic relationships require target-level resolution".to_owned(),
                });
                continue;
            }

            let kind = input.kind.namespaced();
            let resolution = resolution_name(input.resolution);
            let id = RelationId::derive([
                input.source.as_str().as_bytes(),
                input.target.as_str().as_bytes(),
                kind.as_bytes(),
                self.provider.as_bytes(),
                self.provider_version.as_bytes(),
                self.configuration.as_str().as_bytes(),
                resolution.as_bytes(),
            ]);
            relations.entry(id.clone()).or_insert_with(|| Relation {
                id,
                source: input.source,
                target: input.target,
                kind: kind.to_owned(),
                provenance: RelationProvenance {
                    provider: self.provider.clone(),
                    provider_version: self.provider_version.clone(),
                    configuration: Some(self.configuration.clone()),
                    ingest_only: true,
                    resolution: input.resolution,
                    detail: input.detail,
                },
            });
        }

        RustRelationshipInventory {
            relations: relations.into_values().collect(),
            rejected,
        }
    }
}

const fn resolution_name(resolution: ResolutionQuality) -> &'static str {
    match resolution {
        ResolutionQuality::Exact => "exact",
        ResolutionQuality::Inferred => "inferred",
        ResolutionQuality::ContainingTarget => "containing_target",
        ResolutionQuality::FileFallback => "file_fallback",
        ResolutionQuality::Unmapped => "unmapped",
    }
}
