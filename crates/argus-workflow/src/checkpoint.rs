use langchart_checkpoint_redb::RedbCheckpointStore;
use std::{error::Error, fmt, fs, path::Path};

pub const CHECKPOINT_DATABASE_FILE: &str = "langchart-checkpoints.redb";

/// Opens the Langchart checkpoint database owned beneath Argus working state.
pub fn open_checkpoint_store(
    state_directory: &Path,
) -> Result<RedbCheckpointStore, CheckpointOpenError> {
    fs::create_dir_all(state_directory).map_err(CheckpointOpenError::CreateDirectory)?;
    RedbCheckpointStore::open(state_directory.join(CHECKPOINT_DATABASE_FILE))
        .map_err(CheckpointOpenError::Open)
}

#[derive(Debug)]
pub enum CheckpointOpenError {
    CreateDirectory(std::io::Error),
    Open(langchart_checkpoint_redb::StoreError),
}

impl fmt::Display for CheckpointOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDirectory(error) => {
                write!(formatter, "cannot create Argus state directory: {error}")
            }
            Self::Open(error) => write!(formatter, "cannot open Langchart checkpoints: {error}"),
        }
    }
}

impl Error for CheckpointOpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CreateDirectory(error) => Some(error),
            Self::Open(error) => Some(error),
        }
    }
}
