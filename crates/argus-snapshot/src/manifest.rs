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

/// An environment variable captured as part of an analysis configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentInput {
    /// Name of the environment variable.
    pub name: String,
    /// Hash of the exact value. Raw values are excluded to avoid persisting secrets.
    pub value_hash: ContentHash,
}

/// Description of the compiler or toolchain used during analysis.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompilerInput {
    /// Compiler implementation identifier (e.g. "rustc", "gcc", "clang").
    pub implementation: String,
    /// Semantic version or toolchain release string.
    pub version: String,
    /// Git commit hash of the compiler binary, if known.
    pub commit_hash: Option<String>,
    /// Host architecture target string (e.g. "x86_64-unknown-linux-gnu").
    pub host: String,
}

impl EnvironmentInput {
    /// Creates a new environment variable record, hashing the provided value bytes.
    #[must_use]
    pub fn new(name: impl Into<String>, value: &[u8]) -> Self {
        Self {
            name: name.into(),
            value_hash: ContentHash::digest(value),
        }
    }
}

/// Build, environment, and toolchain parameters under which the snapshot was captured.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnalysisConfiguration {
    /// Deterministic configuration identifier derived from all configuration fields.
    pub id: ConfigurationId,
    /// Compilation target triple, if cross-compiling.
    pub target_triple: Option<String>,
    /// Build profile name (e.g. "dev", "release").
    pub profile: String,
    /// Enabled feature flags.
    pub features: BTreeSet<String>,
    /// Configuration flags (`cfg` options).
    pub cfg: BTreeSet<String>,
    /// Environment variables captured at snapshot creation.
    pub environment: BTreeMap<String, EnvironmentInput>,
    /// Compiler toolchain details.
    #[serde(default)]
    pub compiler: Option<CompilerInput>,
}

impl AnalysisConfiguration {
    /// Constructs an analysis configuration and derives its deterministic identifier.
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

    /// Attaches compiler metadata and re-derives the configuration identity.
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

    /// Constructs a default local development configuration.
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

    /// Verifies that the configuration's stored ID matches its derived ID from current fields.
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

/// Categorization of captured files within a source tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileClass {
    /// Human-written or analyzed program source code.
    Source,
    /// Build or project configuration files (e.g. Cargo.toml).
    Configuration,
    /// Dependency lockfile (e.g. Cargo.lock).
    Lockfile,
    /// Generated source input.
    GeneratedInput,
    /// Design documentation, architectural notes, or markdown.
    DesignDocument,
    /// Third-party vendored code.
    Vendor,
    /// Compiled or arbitrary binary file.
    Binary,
    /// File type not supported for analysis.
    Unsupported,
}

/// Metadata record for a single file tracked in a snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Relative path within the snapshot source root.
    pub path: SourcePath,
    /// BLAKE3 content hash, or `None` if the file could not be hashed.
    pub content: Option<ContentHash>,
    /// File size in bytes.
    pub size: u64,
    /// Classified category of the file.
    pub class: FileClass,
}

/// Category of an issue encountered while capturing a file into a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureIssueKind {
    /// Symlink encountered and skipped or unresolved.
    Symlink,
    /// Git submodule boundary encountered.
    Submodule,
    /// File could not be read due to permissions or I/O error.
    Unreadable,
    /// File exceeded configured maximum capture size.
    Oversized,
    /// File encoding is not valid UTF-8.
    UnsupportedEncoding,
}

/// Non-fatal issue encountered while capturing an individual file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaptureIssue {
    /// Relative path where the issue occurred.
    pub path: SourcePath,
    /// Category of the capture issue.
    pub kind: CaptureIssueKind,
    /// Explanatory diagnostic message.
    pub detail: String,
}

/// Kind of drift detected between a snapshot and the current filesystem state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    /// File was added since snapshot creation.
    Added,
    /// File contents have been modified.
    Modified,
    /// File present in snapshot is missing on disk.
    Missing,
    /// File on disk cannot be read to check hash.
    Unreadable,
}

/// Record of detected content drift for a single path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriftRecord {
    /// Relative path exhibiting drift.
    pub path: SourcePath,
    /// Nature of the detected drift.
    pub kind: DriftKind,
}

/// Summary report of differences between a snapshot manifest and on-disk files.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    /// List of detected drift records.
    pub records: Vec<DriftRecord>,
}

impl DriftReport {
    /// Returns `true` if no files have drifted from the snapshot.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.records.is_empty()
    }
}

/// Version control state captured at snapshot creation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VcsState {
    /// VCS commit revision hash or identifier, if available.
    pub revision: Option<String>,
    /// `true` if uncommitted changes were present in the working copy.
    pub dirty: bool,
}

/// Portable capture manifest with separate source-content and occurrence identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Manifest schema version.
    pub schema_version: u32,
    /// Unique snapshot identifier derived from source tree, configuration, and VCS state.
    pub id: SnapshotId,
    /// Content-derived identifier of the complete source tree.
    pub source_tree: SourceTreeId,
    /// Active build and toolchain configuration.
    pub configuration: AnalysisConfiguration,
    /// VCS status at capture time.
    pub vcs: VcsState,
    /// Map of all captured files keyed by relative path.
    pub files: BTreeMap<SourcePath, FileRecord>,
    /// Non-fatal issues recorded during capture.
    pub issues: BTreeMap<SourcePath, CaptureIssue>,
}

impl SnapshotManifest {
    /// Validates that the stored snapshot and configuration IDs match values derived from their fields.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if schema version is unsupported or identities mismatch.
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
