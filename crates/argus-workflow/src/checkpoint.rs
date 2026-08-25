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
