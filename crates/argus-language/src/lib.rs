//! Language-neutral adapter contracts and normalization.

mod contract;
mod registry;
mod synthetic;

pub use contract::{
    AdapterIdentity, AdapterInventory, AdapterProvider, CollectingInventorySink, ConflictRecord,
    DiscoveryPartition, InventorySink, LanguageAdapter, ProviderRole, SourceAccess,
    normalize_inventory,
};
pub use registry::{AdapterFailure, AdapterRegistry, CombinedInventory};
pub use synthetic::{SyntheticAdapter, SyntheticMode};
