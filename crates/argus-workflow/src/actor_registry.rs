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

use crate::{ActorIdentity, RecoveryManifest};
use langchart_model::id::StateId;
use langchart_runtime::AgentActor;
use std::{
    collections::{HashMap, hash_map::Entry},
    error::Error,
    fmt,
    sync::Arc,
};

/// Factory trait for dynamically instantiating [`AgentActor`] instances for workflow states.
///
/// Implementations must be thread-safe (`Send + Sync`). Any closure matching
/// `Fn(&str) -> Result<Arc<dyn AgentActor>, String> + Send + Sync` automatically implements `ActorFactory`.
pub trait ActorFactory: Send + Sync {
    /// Builds an actor instance configured for the given workflow state ID.
    ///
    /// # Errors
    /// Returns an error message string if the actor cannot be instantiated for the state.
    fn build(&self, state_id: &str) -> Result<Arc<dyn AgentActor>, String>;
}

impl<F> ActorFactory for F
where
    F: Fn(&str) -> Result<Arc<dyn AgentActor>, String> + Send + Sync,
{
    fn build(&self, state_id: &str) -> Result<Arc<dyn AgentActor>, String> {
        self(state_id)
    }
}

/// Registry mapping `(actor_id, actor_version)` pairs to [`ActorFactory`] implementations.
///
/// `ActorRegistry` is used during workflow recovery to deterministically reconstruct the
/// runtime actor topology declared in a [`RecoveryManifest`].
///
/// # Invariants
/// - Every `(actor_id, actor_version)` pair in the registry is unique.
/// - Actor IDs and versions must be non-empty and trimmed.
/// - During reconstruction, each state ID must map to at most one actor.
///
/// # Examples
///
/// ```
/// use argus_workflow::{ActorRegistry, ActorFactory};
/// use std::sync::Arc;
///
/// let mut registry = ActorRegistry::new();
/// ```
#[derive(Default)]
pub struct ActorRegistry {
    factories: HashMap<(String, String), Arc<dyn ActorFactory>>,
}

impl ActorRegistry {
    /// Creates an empty actor registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an [`ActorFactory`] for the given actor identity and version.
    ///
    /// # Errors
    /// Returns [`ActorRegistryError::Invalid`] if the actor ID or version is empty or contains
    /// leading/trailing whitespace.
    /// Returns [`ActorRegistryError::Duplicate`] if a factory is already registered for this identity.
    pub fn register(
        &mut self,
        actor_id: impl Into<String>,
        actor_version: impl Into<String>,
        factory: Arc<dyn ActorFactory>,
    ) -> Result<(), ActorRegistryError> {
        let key = (actor_id.into(), actor_version.into());
        validate_identity(&key.0, "actor ID")?;
        validate_identity(&key.1, "actor version")?;
        match self.factories.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(factory);
            }
            Entry::Occupied(entry) => {
                return Err(ActorRegistryError::Duplicate {
                    actor_id: entry.key().0.clone(),
                    actor_version: entry.key().1.clone(),
                });
            }
        }
        Ok(())
    }

    /// Reconstructs actors for all states declared in a [`RecoveryManifest`].
    ///
    /// # Errors
    /// Returns:
    /// - [`ActorRegistryError::MissingFactory`] if any actor identity required by the manifest
    ///   is not registered.
    /// - [`ActorRegistryError::Build`] if an actor factory fails to construct an actor.
    /// - [`ActorRegistryError::DuplicateState`] if the manifest assigns multiple actors to the same state ID.
    pub fn reconstruct(
        &self,
        manifest: &RecoveryManifest,
    ) -> Result<HashMap<StateId, Arc<dyn AgentActor>>, ActorRegistryError> {
        let mut actors = HashMap::with_capacity(manifest.actors.len());
        for identity in &manifest.actors {
            let key = (identity.actor_id.clone(), identity.actor_version.clone());
            let factory =
                self.factories
                    .get(&key)
                    .ok_or_else(|| ActorRegistryError::MissingFactory {
                        actor_id: key.0.clone(),
                        actor_version: key.1.clone(),
                    })?;
            let actor =
                factory
                    .build(&identity.state_id)
                    .map_err(|message| ActorRegistryError::Build {
                        identity: identity.clone(),
                        message,
                    })?;
            if actors
                .insert(StateId::new(identity.state_id.clone()), actor)
                .is_some()
            {
                return Err(ActorRegistryError::DuplicateState(
                    identity.state_id.clone(),
                ));
            }
        }
        Ok(actors)
    }
}

fn validate_identity(value: &str, name: &str) -> Result<(), ActorRegistryError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(ActorRegistryError::Invalid(format!(
            "{name} must be non-empty and normalized"
        )));
    }
    Ok(())
}

/// Errors that can occur when registering actors or reconstructing a workflow from a manifest.
#[derive(Debug)]
pub enum ActorRegistryError {
    /// An actor ID or version failed validation (e.g. empty or unnormalized whitespace).
    Invalid(String),
    /// An actor factory is already registered for the specified ID and version.
    Duplicate {
        /// Identifier of the duplicated actor.
        actor_id: String,
        /// Version of the duplicated actor.
        actor_version: String,
    },
    /// No actor factory was registered for the required actor ID and version.
    MissingFactory {
        /// Identifier of the missing actor.
        actor_id: String,
        /// Version of the missing actor.
        actor_version: String,
    },
    /// The recovery manifest repeated a state ID with multiple actors.
    DuplicateState(String),
    /// An actor factory failed to build an actor instance.
    Build {
        /// Identity of the actor being built.
        identity: ActorIdentity,
        /// Underlying failure message from the factory.
        message: String,
    },
}

impl fmt::Display for ActorRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid actor registry entry: {message}"),
            Self::Duplicate {
                actor_id,
                actor_version,
            } => write!(
                formatter,
                "actor factory `{actor_id}@{actor_version}` is already registered"
            ),
            Self::MissingFactory {
                actor_id,
                actor_version,
            } => write!(
                formatter,
                "no actor factory is registered for `{actor_id}@{actor_version}`"
            ),
            Self::DuplicateState(state_id) => {
                write!(formatter, "actor manifest repeats state `{state_id}`")
            }
            Self::Build { identity, message } => write!(
                formatter,
                "cannot reconstruct actor `{}` for state `{}`: {message}",
                identity.actor_id, identity.state_id
            ),
        }
    }
}

impl Error for ActorRegistryError {}
