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

use crate::{WORKFLOW_DATA_SCHEMA_VERSION, target_review_source};
use argus_core::{RunId as AuditRunId, SnapshotId, WorkItemId};
use argus_provider::{ProviderIdentity, ProviderPolicy};
use langchart_model::{
    state::{StateDefinition, StateType},
    validation::{CompiledWorkflow, compile},
    workflow::WorkflowDocument,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const RECOVERY_MANIFEST_SCHEMA_VERSION: u32 = 2;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowArtifactIdentity {
    pub workflow_id: String,
    pub workflow_version: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActorIdentity {
    pub state_id: String,
    pub actor_id: String,
    pub actor_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecoveryManifest {
    pub schema_version: u32,
    pub workflow_data_schema_version: u32,
    pub langchart_run_id: String,
    pub audit_snapshot: SnapshotId,
    pub audit_run: AuditRunId,
    pub work_id: WorkItemId,
    pub workflow: WorkflowArtifactIdentity,
    pub actors: Vec<ActorIdentity>,
    pub provider: ProviderIdentity,
    pub provider_policy: ProviderPolicy,
    pub policy_version: String,
    pub prompt_version: String,
    pub evidence_revision: u32,
    pub langchart_runtime_version: String,
}

#[derive(Debug)]
pub struct RecoveryStore {
    state_directory: PathBuf,
}

impl RecoveryStore {
    pub fn open(state_directory: &Path) -> Result<Self, RecoveryError> {
        fs::create_dir_all(state_directory.join("workflows")).map_err(RecoveryError::Io)?;
        fs::create_dir_all(state_directory.join("recovery")).map_err(RecoveryError::Io)?;
        Ok(Self {
            state_directory: state_directory.to_owned(),
        })
    }

    pub fn store_target_review(&self) -> Result<WorkflowArtifactIdentity, RecoveryError> {
        self.store_workflow(target_review_source().as_bytes())
    }

    pub fn store_workflow(&self, source: &[u8]) -> Result<WorkflowArtifactIdentity, RecoveryError> {
        let document: WorkflowDocument =
            serde_json::from_slice(source).map_err(RecoveryError::Json)?;
        compile(document.clone()).map_err(RecoveryError::Compile)?;
        let content_hash = blake3::hash(source).to_hex().to_string();
        let identity = WorkflowArtifactIdentity {
            workflow_id: document.id.as_ref().to_owned(),
            workflow_version: document.version.as_ref().to_owned(),
            content_hash,
        };
        write_immutable(&self.workflow_path(&identity.content_hash), source)?;
        Ok(identity)
    }

    pub fn actor_identities(
        &self,
        workflow: &WorkflowArtifactIdentity,
    ) -> Result<Vec<ActorIdentity>, RecoveryError> {
        let document = self.load_document(workflow)?;
        let mut actors = Vec::new();
        collect_actor_identities(&document.states, &mut actors)?;
        actors.sort_by(|left, right| left.state_id.cmp(&right.state_id));
        if actors
            .windows(2)
            .any(|pair| pair[0].state_id == pair[1].state_id)
        {
            return Err(RecoveryError::Invalid(
                "workflow contains duplicate agentic state identities".to_owned(),
            ));
        }
        Ok(actors)
    }

    pub fn write_manifest(&self, manifest: &RecoveryManifest) -> Result<(), RecoveryError> {
        self.validate_manifest(manifest)?;
        let bytes = serde_json::to_vec_pretty(manifest).map_err(RecoveryError::Json)?;
        write_immutable(&self.manifest_path(&manifest.langchart_run_id), &bytes)
    }

    pub fn load_manifest(&self, langchart_run_id: &str) -> Result<RecoveryManifest, RecoveryError> {
        let bytes = fs::read(self.manifest_path(langchart_run_id)).map_err(RecoveryError::Io)?;
        let manifest: RecoveryManifest =
            serde_json::from_slice(&bytes).map_err(RecoveryError::Json)?;
        if manifest.langchart_run_id != langchart_run_id {
            return Err(RecoveryError::Invalid(
                "recovery manifest run identity does not match its lookup key".to_owned(),
            ));
        }
        self.validate_manifest(&manifest)?;
        Ok(manifest)
    }

    pub fn load_compiled(
        &self,
        workflow: &WorkflowArtifactIdentity,
    ) -> Result<CompiledWorkflow, RecoveryError> {
        compile(self.load_document(workflow)?).map_err(RecoveryError::Compile)
    }

    fn load_document(
        &self,
        workflow: &WorkflowArtifactIdentity,
    ) -> Result<WorkflowDocument, RecoveryError> {
        validate_digest(&workflow.content_hash)?;
        let bytes =
            fs::read(self.workflow_path(&workflow.content_hash)).map_err(RecoveryError::Io)?;
        let actual_hash = blake3::hash(&bytes).to_hex().to_string();
        if actual_hash != workflow.content_hash.to_ascii_lowercase() {
            return Err(RecoveryError::Invalid(
                "workflow artifact content hash mismatch".to_owned(),
            ));
        }
        let document: WorkflowDocument =
            serde_json::from_slice(&bytes).map_err(RecoveryError::Json)?;
        if document.id.as_ref() != workflow.workflow_id
            || document.version.as_ref() != workflow.workflow_version
        {
            return Err(RecoveryError::Invalid(
                "workflow artifact identity does not match recovery manifest".to_owned(),
            ));
        }
        Ok(document)
    }

    fn validate_manifest(&self, manifest: &RecoveryManifest) -> Result<(), RecoveryError> {
        if manifest.schema_version != RECOVERY_MANIFEST_SCHEMA_VERSION {
            return Err(RecoveryError::Invalid(format!(
                "unsupported recovery manifest schema {}",
                manifest.schema_version
            )));
        }
        if manifest.workflow_data_schema_version != WORKFLOW_DATA_SCHEMA_VERSION {
            return Err(RecoveryError::Invalid(format!(
                "unsupported workflow data schema {}",
                manifest.workflow_data_schema_version
            )));
        }
        for value in [
            &manifest.langchart_run_id,
            &manifest.policy_version,
            &manifest.prompt_version,
            &manifest.langchart_runtime_version,
            &manifest.provider.provider,
            &manifest.provider.provider_version,
            &manifest.provider.model,
            &manifest.provider.model_version,
        ] {
            if value.trim().is_empty() || value.trim() != *value {
                return Err(RecoveryError::Invalid(
                    "recovery identity fields must be non-empty and normalized".to_owned(),
                ));
            }
        }
        if manifest.evidence_revision == 0 {
            return Err(RecoveryError::Invalid(
                "recovery manifest requires a non-zero evidence revision".to_owned(),
            ));
        }
        let expected = self.actor_identities(&manifest.workflow)?;
        let supplied = manifest
            .actors
            .iter()
            .map(|actor| (actor.state_id.clone(), actor.clone()))
            .collect::<BTreeMap<_, _>>();
        if supplied.len() != manifest.actors.len()
            || expected.len() != manifest.actors.len()
            || expected
                .iter()
                .any(|actor| supplied.get(&actor.state_id) != Some(actor))
        {
            return Err(RecoveryError::Invalid(
                "recovery actor set does not exactly match the workflow document".to_owned(),
            ));
        }
        Ok(())
    }

    fn workflow_path(&self, content_hash: &str) -> PathBuf {
        self.state_directory
            .join("workflows")
            .join(format!("{content_hash}.json"))
    }

    fn manifest_path(&self, run_id: &str) -> PathBuf {
        let digest = blake3::hash(run_id.as_bytes()).to_hex().to_string();
        self.state_directory
            .join("recovery")
            .join(format!("{digest}.json"))
    }
}

fn collect_actor_identities(
    states: &[StateDefinition],
    actors: &mut Vec<ActorIdentity>,
) -> Result<(), RecoveryError> {
    for state in states {
        if state.state_type == StateType::Agentic {
            let actor = state.agent.as_ref().ok_or_else(|| {
                RecoveryError::Invalid(format!(
                    "agentic state `{}` has no actor identity",
                    state.id
                ))
            })?;
            actors.push(ActorIdentity {
                state_id: state.id.as_ref().to_owned(),
                actor_id: actor.id.as_ref().to_owned(),
                actor_version: actor.version.as_ref().to_owned(),
            });
        }
        collect_actor_identities(&state.states, actors)?;
        for region in &state.regions {
            collect_actor_identities(&region.states, actors)?;
        }
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), RecoveryError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RecoveryError::Invalid(
            "workflow artifact hash is not a hexadecimal digest".to_owned(),
        ));
    }
    Ok(())
}

fn write_immutable(path: &Path, bytes: &[u8]) -> Result<(), RecoveryError> {
    if path.exists() {
        let existing = fs::read(path).map_err(RecoveryError::Io)?;
        return if existing == bytes {
            Ok(())
        } else {
            Err(RecoveryError::Conflict(path.to_owned()))
        };
    }
    let parent = path
        .parent()
        .ok_or_else(|| RecoveryError::Invalid("immutable record has no parent".to_owned()))?;
    fs::create_dir_all(parent).map_err(RecoveryError::Io)?;
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".argus-write-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(RecoveryError::Io)?;
    file.write_all(bytes).map_err(RecoveryError::Io)?;
    file.sync_all().map_err(RecoveryError::Io)?;
    let publish = fs::hard_link(&temporary, path).or_else(|_| fs::rename(&temporary, path));
    let _ = fs::remove_file(&temporary);
    match publish {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => {
            if fs::read(path).map_err(RecoveryError::Io)? == bytes {
                Ok(())
            } else {
                Err(RecoveryError::Conflict(path.to_owned()))
            }
        }
        Err(error) => Err(RecoveryError::Io(error)),
    }
}

#[derive(Debug)]
pub enum RecoveryError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Compile(langchart_model::error::CompileError),
    Invalid(String),
    Conflict(PathBuf),
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "recovery storage I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "recovery JSON is invalid: {error}"),
            Self::Compile(error) => write!(formatter, "recovery workflow is invalid: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid recovery state: {message}"),
            Self::Conflict(path) => write!(
                formatter,
                "immutable recovery record conflicts at {}",
                path.display()
            ),
        }
    }
}

impl Error for RecoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Compile(error) => Some(error),
            Self::Invalid(_) | Self::Conflict(_) => None,
        }
    }
}
