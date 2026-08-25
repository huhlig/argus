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

use argus_core::{ConfigurationId, ContentHash, SnapshotId, SourcePath, SourceTreeId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentInput {
    pub name: String,
    /// Hash of the exact value. Raw values are excluded to avoid persisting secrets.
    pub value_hash: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompilerInput {
    pub implementation: String,
    pub version: String,
    pub commit_hash: Option<String>,
    pub host: String,
}

impl EnvironmentInput {
    #[must_use]
    pub fn new(name: impl Into<String>, value: &[u8]) -> Self {
        Self {
            name: name.into(),
            value_hash: ContentHash::digest(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnalysisConfiguration {
    pub id: ConfigurationId,
    pub target_triple: Option<String>,
    pub profile: String,
    pub features: BTreeSet<String>,
    pub cfg: BTreeSet<String>,
    pub environment: BTreeMap<String, EnvironmentInput>,
    #[serde(default)]
    pub compiler: Option<CompilerInput>,
}

impl AnalysisConfiguration {
    #[must_use]
    pub fn new(
        target_triple: Option<String>,
        profile: impl Into<String>,
        features: BTreeSet<String>,
        cfg: BTreeSet<String>,
        environment: BTreeMap<String, EnvironmentInput>,
    ) -> Self {
        let profile = profile.into();
        let compiler = None;
        let id = configuration_id(
            target_triple.as_deref(),
            &profile,
            &features,
            &cfg,
            &environment,
            compiler.as_ref(),
        );
        Self {
            id,
            target_triple,
            profile,
            features,
            cfg,
            environment,
            compiler,
        }
    }

    #[must_use]
    pub fn with_compiler(mut self, compiler: CompilerInput) -> Self {
        self.compiler = Some(compiler);
        self.id = configuration_id(
            self.target_triple.as_deref(),
            &self.profile,
            &self.features,
            &self.cfg,
            &self.environment,
            self.compiler.as_ref(),
        );
        self
    }

    #[must_use]
    pub fn default_host() -> Self {
        Self::new(
            None,
            "dev",
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeMap::new(),
        )
    }

    #[must_use]
    pub fn has_valid_identity(&self) -> bool {
        configuration_id(
            self.target_triple.as_deref(),
            &self.profile,
            &self.features,
            &self.cfg,
            &self.environment,
            self.compiler.as_ref(),
        ) == self.id
    }
}

fn configuration_id(
    target_triple: Option<&str>,
    profile: &str,
    features: &BTreeSet<String>,
    cfg: &BTreeSet<String>,
    environment: &BTreeMap<String, EnvironmentInput>,
    compiler: Option<&CompilerInput>,
) -> ConfigurationId {
    let mut identity = Vec::new();
    if let Some(target) = target_triple {
        identity.push(1);
        push_identity(&mut identity, target);
    } else {
        identity.push(0);
    }
    push_identity(&mut identity, profile);
    for feature in features {
        push_identity(&mut identity, feature);
    }
    for item in cfg {
        push_identity(&mut identity, item);
    }
    for (name, input) in environment {
        push_identity(&mut identity, name);
        push_identity(&mut identity, input.value_hash.as_str());
    }
    if let Some(compiler) = compiler {
        identity.push(1);
        push_identity(&mut identity, &compiler.implementation);
        push_identity(&mut identity, &compiler.version);
        match &compiler.commit_hash {
            Some(commit_hash) => {
                identity.push(1);
                push_identity(&mut identity, commit_hash);
            }
            None => identity.push(0),
        }
        push_identity(&mut identity, &compiler.host);
    } else {
        identity.push(0);
    }
    ConfigurationId::derive([identity.as_slice()])
}

fn push_identity(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileClass {
    Source,
    Configuration,
    Lockfile,
    GeneratedInput,
    DesignDocument,
    Vendor,
    Binary,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: SourcePath,
    pub content: Option<ContentHash>,
    pub size: u64,
    pub class: FileClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureIssueKind {
    Symlink,
    Submodule,
    Unreadable,
    Oversized,
    UnsupportedEncoding,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaptureIssue {
    pub path: SourcePath,
    pub kind: CaptureIssueKind,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    Added,
    Modified,
    Missing,
    Unreadable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriftRecord {
    pub path: SourcePath,
    pub kind: DriftKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    pub records: Vec<DriftRecord>,
}

impl DriftReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.records.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VcsState {
    pub revision: Option<String>,
    pub dirty: bool,
}

/// Portable capture manifest with separate source-content and occurrence identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub schema_version: u32,
    pub id: SnapshotId,
    pub source_tree: SourceTreeId,
    pub configuration: AnalysisConfiguration,
    pub vcs: VcsState,
    pub files: BTreeMap<SourcePath, FileRecord>,
    pub issues: BTreeMap<SourcePath, CaptureIssue>,
}

impl SnapshotManifest {
    pub fn validate_identity(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != crate::SNAPSHOT_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(
                "unsupported snapshot schema version",
            ));
        }
        if !self.configuration.has_valid_identity()
            || self.derive_source_tree_id()? != self.source_tree
            || self.derive_id()? != self.id
        {
            return Err(argus_core::ArgusError::invariant(
                "snapshot or configuration identity mismatch",
            ));
        }
        Ok(())
    }

    pub(crate) fn derive_id(&self) -> Result<SnapshotId, argus_core::ArgusError> {
        let identity = serde_json::to_vec(&(
            self.schema_version,
            &self.source_tree,
            &self.configuration,
            &self.vcs,
        ))
        .map_err(|error| {
            argus_core::ArgusError::invariant("snapshot identity serialization failed")
                .with_source(error)
        })?;
        Ok(SnapshotId::derive([identity.as_slice()]))
    }

    pub(crate) fn derive_source_tree_id(&self) -> Result<SourceTreeId, argus_core::ArgusError> {
        let identity = serde_json::to_vec(&(self.schema_version, &self.files, &self.issues))
            .map_err(|error| {
                argus_core::ArgusError::invariant("source tree identity serialization failed")
                    .with_source(error)
            })?;
        Ok(SourceTreeId::derive([identity.as_slice()]))
    }
}
