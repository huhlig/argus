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

use crate::{ModelSubstitution, ProviderError, ProviderIdentity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Binding between a workflow partition (or review role) and a specific provider/model identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelAssignment {
    /// Name of the workflow partition (e.g. "primary", "verification").
    pub partition: String,
    /// Provider identity assigned to execute this partition.
    pub provider: ProviderIdentity,
}

/// Ledger tracking model-to-partition assignments across a review run according to substitution policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelAssignmentBook {
    substitution: ModelSubstitution,
    assignments: BTreeMap<String, ProviderIdentity>,
}

impl ModelAssignmentBook {
    /// Creates a new assignment book enforcing the specified substitution rule.
    #[must_use]
    pub fn new(substitution: ModelSubstitution) -> Self {
        Self {
            substitution,
            assignments: BTreeMap::new(),
        }
    }

    /// Assigns a provider to a partition, enforcing stability and substitution constraints.
    ///
    /// Returns `Ok(true)` if a new assignment was recorded, or `Ok(false)` if the partition was
    /// already assigned to the exact same provider.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::SubstitutionDenied`] if the partition is already pinned to a different
    /// provider, or if the run is [`ModelSubstitution::Pinned`] and a different model is attempted.
    pub fn assign(
        &mut self,
        partition: &str,
        provider: ProviderIdentity,
    ) -> Result<bool, ProviderError> {
        validate_partition(partition)?;
        provider.validate()?;
        if let Some(existing) = self.assignments.get(partition) {
            return if *existing == provider {
                Ok(false)
            } else {
                Err(ProviderError::SubstitutionDenied(format!(
                    "partition `{partition}` is already pinned to a different provider/model"
                )))
            };
        }
        if self.substitution == ModelSubstitution::Pinned
            && self
                .assignments
                .values()
                .next()
                .is_some_and(|existing| *existing != provider)
        {
            return Err(ProviderError::SubstitutionDenied(
                "the run is pinned to its first provider/model identity".to_owned(),
            ));
        }
        self.assignments.insert(partition.to_owned(), provider);
        Ok(true)
    }

    /// Returns a list of all active partition-to-provider assignments.
    #[must_use]
    pub fn assignments(&self) -> Vec<ModelAssignment> {
        self.assignments
            .iter()
            .map(|(partition, provider)| ModelAssignment {
                partition: partition.clone(),
                provider: provider.clone(),
            })
            .collect()
    }
}

fn validate_partition(partition: &str) -> Result<(), ProviderError> {
    if partition.trim().is_empty() || partition.trim() != partition {
        return Err(ProviderError::InvalidPolicy(
            "model partition must be non-empty and normalized".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(model: &str) -> ProviderIdentity {
        ProviderIdentity {
            provider: "fixture".to_owned(),
            provider_version: "1".to_owned(),
            model: model.to_owned(),
            model_version: "pinned".to_owned(),
        }
    }

    #[test]
    fn pinned_run_rejects_any_identity_change() {
        let mut book = ModelAssignmentBook::new(ModelSubstitution::Pinned);
        assert!(book.assign("primary", identity("small")).unwrap());
        assert!(matches!(
            book.assign("verification", identity("large")),
            Err(ProviderError::SubstitutionDenied(_))
        ));
    }

    #[test]
    fn partitioned_run_allows_visible_but_stable_assignments() {
        let mut book = ModelAssignmentBook::new(ModelSubstitution::Partitioned);
        book.assign("primary", identity("small")).unwrap();
        book.assign("verification", identity("large")).unwrap();
        assert!(!book.assign("primary", identity("small")).unwrap());
        assert!(book.assign("primary", identity("large")).is_err());
        assert_eq!(book.assignments().len(), 2);
    }
}
