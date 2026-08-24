use crate::{
    AdapterIdentity, AdapterInventory, ConflictRecord, DiscoveryPartition, LanguageAdapter,
    SourceAccess, normalize_inventory,
};
use argus_core::{CapabilityStatus, TargetId};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterFailure {
    pub adapter: AdapterIdentity,
    pub partition: DiscoveryPartition,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CombinedInventory {
    pub inventories: Vec<AdapterInventory>,
    pub failures: Vec<AdapterFailure>,
    pub conflicts: Vec<ConflictRecord>,
}

#[derive(Default)]
pub struct AdapterRegistry {
    adapters: BTreeMap<String, Box<dyn LanguageAdapter>>,
}

impl AdapterRegistry {
    pub fn register(
        &mut self,
        adapter: impl LanguageAdapter + 'static,
    ) -> Result<(), argus_core::ArgusError> {
        let identity = adapter.identity();
        if identity.name.trim().is_empty() || identity.version.trim().is_empty() {
            return Err(argus_core::ArgusError::invalid_input(
                "adapter name and version are required",
            ));
        }
        if self.adapters.contains_key(&identity.name) {
            return Err(argus_core::ArgusError::invariant(
                "adapter name is already registered",
            ));
        }
        self.adapters.insert(identity.name, Box::new(adapter));
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    pub fn inventory_all(&self, source: &dyn SourceAccess) -> CombinedInventory {
        let mut combined = CombinedInventory::default();
        let mut target_owners = BTreeMap::<TargetId, String>::new();
        for adapter in self.adapters.values() {
            let identity = adapter.identity();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                adapter
                    .inventory(source)
                    .and_then(|inventory| normalize_inventory(source, inventory))
            }));
            match result {
                Ok(Ok(inventory)) => {
                    for target in &inventory.targets {
                        if let Some(owner) =
                            target_owners.insert(target.id.clone(), identity.name.clone())
                        {
                            combined.conflicts.push(ConflictRecord {
                                subject: target.id.to_string(),
                                providers: vec![owner, identity.name.clone()],
                                detail: "adapters emitted the same normalized target ID".to_owned(),
                            });
                        }
                    }
                    combined
                        .conflicts
                        .extend(inventory.conflicts.iter().cloned());
                    combined.inventories.push(inventory);
                }
                Ok(Err(error)) => combined.failures.push(failure(identity, error.to_string())),
                Err(_) => combined.failures.push(failure(
                    identity,
                    "adapter panicked during inventory".to_owned(),
                )),
            }
        }
        combined
            .conflicts
            .sort_by(|left, right| left.subject.cmp(&right.subject));
        combined
    }
}

fn failure(adapter: AdapterIdentity, diagnostic: String) -> AdapterFailure {
    AdapterFailure {
        partition: DiscoveryPartition {
            name: format!("adapter:{}", adapter.name),
            status: CapabilityStatus::Failed,
            diagnostic: Some(diagnostic),
        },
        adapter,
    }
}
