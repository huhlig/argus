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

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum PortableTargetKind {
    Workspace,
    Package,
    Module,
    Type,
    Callable,
    Constant,
    Test,
    File,
}

/// Portable target classification with an open language-specific namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum TargetKind {
    Portable { kind: PortableTargetKind },
    LanguageSpecific { language: String, kind: String },
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetVisibility {
    Public,
    Restricted,
    Private,
    Inherited,
    NotApplicable,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Complete,
    Partial,
    Unavailable,
    Failed,
}

/// Resolution of one named capability for one target or discovery partition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub name: String,
    pub status: CapabilityStatus,
    pub detail: Option<String>,
    pub provider: Option<String>,
}

impl Capability {
    pub fn validate(&self) -> Result<(), crate::ArgusError> {
        if self.name.trim().is_empty() {
            return Err(crate::ArgusError::invariant("capability name is empty"));
        }
        if matches!(
            self.status,
            CapabilityStatus::Partial | CapabilityStatus::Failed
        ) && self.detail.as_deref().is_none_or(str::is_empty)
        {
            return Err(crate::ArgusError::invariant(
                "partial and failed capabilities require detail",
            ));
        }
        Ok(())
    }
}
