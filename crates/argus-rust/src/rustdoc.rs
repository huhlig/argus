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
    ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin, EvidenceProvenance, EvidenceRecord,
    ResolutionQuality, SourcePath, Target, TargetId,
};
use argus_language::SourceAccess;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RustdocEvidenceInventory {
    pub evidence: Vec<EvidenceRecord>,
    pub rejected_items: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RustdocCrate {
    format_version: u32,
    #[serde(default)]
    index: BTreeMap<String, Value>,
    #[serde(default)]
    paths: BTreeMap<String, RustdocPath>,
}

#[derive(Debug, Deserialize)]
struct RustdocPath {
    path: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RustdocItem {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    docs: Option<String>,
    #[serde(default)]
    span: Option<RustdocSpan>,
}

#[derive(Debug, Deserialize)]
struct RustdocSpan {
    filename: String,
}

/// Bounded ingestion for the explicitly versioned rustdoc JSON format.
pub struct RustdocJsonProvider {
    json: Vec<u8>,
    configuration: ConfigurationId,
    supported_format_version: u32,
    provider_version: String,
    max_input_bytes: usize,
    workspace_root: PathBuf,
}

impl RustdocJsonProvider {
    #[must_use]
    pub fn new(
        json: Vec<u8>,
        configuration: ConfigurationId,
        supported_format_version: u32,
        provider_version: impl Into<String>,
        max_input_bytes: usize,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            json,
            configuration,
            supported_format_version,
            provider_version: provider_version.into(),
            max_input_bytes,
            workspace_root,
        }
    }

    pub fn ingest(
        &self,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<RustdocEvidenceInventory, argus_core::ArgusError> {
        if self.max_input_bytes == 0 || self.json.len() > self.max_input_bytes {
            return Err(argus_core::ArgusError::invalid_input(
                "rustdoc JSON exceeds its configured input bound",
            ));
        }
        let krate = serde_json::from_slice::<RustdocCrate>(&self.json).map_err(|error| {
            argus_core::ArgusError::invalid_input("invalid rustdoc JSON").with_source(error)
        })?;
        if krate.format_version != self.supported_format_version {
            return Err(argus_core::ArgusError::unsupported(format!(
                "rustdoc JSON format {} is unsupported; expected {}",
                krate.format_version, self.supported_format_version
            )));
        }

        let mut inventory = RustdocEvidenceInventory::default();
        for (item_id, value) in krate.index {
            let item = match serde_json::from_value::<RustdocItem>(value) {
                Ok(item) => item,
                Err(error) => {
                    inventory
                        .rejected_items
                        .push(format!("item {item_id}: {error}"));
                    continue;
                }
            };
            let Some(docs) = item.docs.filter(|docs| !docs.trim().is_empty()) else {
                continue;
            };
            let path = krate.paths.get(&item_id).map(|path| path.path.join("::"));
            let (target, resolution) = resolve_target(
                targets,
                item.name.as_deref(),
                path.as_deref(),
                item.span
                    .as_ref()
                    .and_then(|span| self.rustdoc_path(&span.filename)),
            );
            let label = path
                .as_deref()
                .or(item.name.as_deref())
                .unwrap_or(item_id.as_str());
            let target_identity = target.as_ref().map_or("", TargetId::as_str);
            let record = EvidenceRecord {
                id: EvidenceId::derive([
                    b"rustdoc-json".as_slice(),
                    source.snapshot_id().as_str().as_bytes(),
                    self.configuration.as_str().as_bytes(),
                    self.provider_version.as_bytes(),
                    item_id.as_bytes(),
                    target_identity.as_bytes(),
                    docs.as_bytes(),
                ]),
                kind: EvidenceKind::Documentation,
                origin: EvidenceOrigin::Direct,
                target,
                location: None,
                summary: format!("rustdoc documentation for {label}"),
                detail: Some(docs),
                provenance: EvidenceProvenance {
                    provider: "rustdoc-json".to_owned(),
                    provider_version: self.provider_version.clone(),
                    configuration: self.configuration.clone(),
                    ingest_only: true,
                    resolution,
                },
            };
            record.validate()?;
            inventory.evidence.push(record);
        }
        inventory
            .evidence
            .sort_by(|left, right| left.id.cmp(&right.id));
        inventory.rejected_items.sort();
        Ok(inventory)
    }

    fn rustdoc_path(&self, filename: &str) -> Option<SourcePath> {
        let path = PathBuf::from(filename);
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.workspace_root).ok()?
        } else {
            path.as_path()
        };
        SourcePath::new(relative.to_string_lossy().replace('\\', "/")).ok()
    }
}

fn resolve_target(
    targets: &[Target],
    name: Option<&str>,
    qualified_path: Option<&str>,
    source_path: Option<SourcePath>,
) -> (Option<TargetId>, ResolutionQuality) {
    let mut candidates = targets
        .iter()
        .filter(|target| {
            name.is_some_and(|name| target.name == name)
                || qualified_path.is_some_and(|path| target.name == path)
        })
        .collect::<Vec<_>>();
    if let Some(path) = source_path {
        let located = candidates
            .iter()
            .copied()
            .filter(|target| {
                target
                    .location
                    .as_ref()
                    .is_some_and(|location| location.path == path)
            })
            .collect::<Vec<_>>();
        if !located.is_empty() {
            candidates = located;
        }
    }
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    if candidates.len() == 1 {
        (Some(candidates[0].id.clone()), ResolutionQuality::Exact)
    } else {
        (None, ResolutionQuality::Unmapped)
    }
}
