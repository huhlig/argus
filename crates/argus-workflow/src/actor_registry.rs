use crate::{ActorIdentity, RecoveryManifest};
use langchart_model::id::StateId;
use langchart_runtime::AgentActor;
use std::{
    collections::{HashMap, hash_map::Entry},
    error::Error,
    fmt,
    sync::Arc,
};

pub trait ActorFactory: Send + Sync {
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

#[derive(Default)]
pub struct ActorRegistry {
    factories: HashMap<(String, String), Arc<dyn ActorFactory>>,
}

impl ActorRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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

#[derive(Debug)]
pub enum ActorRegistryError {
    Invalid(String),
    Duplicate {
        actor_id: String,
        actor_version: String,
    },
    MissingFactory {
        actor_id: String,
        actor_version: String,
    },
    DuplicateState(String),
    Build {
        identity: ActorIdentity,
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
