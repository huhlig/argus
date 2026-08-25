use crate::{ModelSubstitution, ProviderError, ProviderIdentity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelAssignment {
    pub partition: String,
    pub provider: ProviderIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelAssignmentBook {
    substitution: ModelSubstitution,
    assignments: BTreeMap<String, ProviderIdentity>,
}

impl ModelAssignmentBook {
    #[must_use]
    pub fn new(substitution: ModelSubstitution) -> Self {
        Self {
            substitution,
            assignments: BTreeMap::new(),
        }
    }

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
