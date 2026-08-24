use crate::{CargoMetadataAdapter, RustEdition, RustSyntaxProvider};
use argus_core::{
    CapabilityStatus, ConfigurationId, InventoryState, Relation, RelationId, RelationProvenance,
    SourcePath, Target, TargetId, TargetKind,
};
use argus_language::{
    AdapterIdentity, AdapterInventory, AdapterProvider, CollectingInventorySink, ConflictRecord,
    DiscoveryPartition, InventorySink, LanguageAdapter, ProviderRole, SourceAccess,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct RustWorkspaceAdapter {
    cargo: CargoMetadataAdapter,
    syntax: RustSyntaxProvider,
    configuration: ConfigurationId,
}

impl RustWorkspaceAdapter {
    #[must_use]
    pub fn new(
        metadata_json: Vec<u8>,
        configuration: ConfigurationId,
        edition: RustEdition,
    ) -> Self {
        Self {
            cargo: CargoMetadataAdapter::new(metadata_json, configuration.clone()),
            syntax: RustSyntaxProvider::new(configuration.clone(), edition),
            configuration,
        }
    }

    fn emit_inventory(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<(), argus_core::ArgusError> {
        let cargo_inventory = self.cargo.inventory(source)?;
        let entries = cargo_inventory
            .targets
            .iter()
            .filter_map(cargo_entry)
            .collect::<Vec<_>>();
        let mut seen_targets = BTreeSet::new();
        let mut seen_relations = BTreeSet::new();
        sink.begin(self.identity(), source.snapshot_id().clone())?;
        for partition in cargo_inventory.partitions {
            sink.partition(partition)?;
        }
        for target in cargo_inventory.targets {
            seen_targets.insert(target.id.clone());
            sink.target(target)?;
        }
        for relation in cargo_inventory.relations {
            seen_relations.insert(relation.id.clone());
            sink.relation(relation)?;
        }
        for conflict in cargo_inventory.conflicts {
            sink.conflict(conflict)?;
        }

        for (cargo_target, path) in entries {
            let syntax = self
                .syntax
                .inventory_crate(source, &path, Some(cargo_target))?;
            sink.partition(syntax_partition(&path, &syntax))?;
            let mut crate_relations = Vec::new();
            for target in syntax.targets {
                if let Some(parent) = &target.parent {
                    crate_relations.push(contains_relation(
                        parent.clone(),
                        target.id.clone(),
                        self.configuration.clone(),
                    ));
                }
                if seen_targets.insert(target.id.clone()) {
                    sink.target(target)?;
                } else {
                    sink.conflict(ConflictRecord {
                        subject: target.id.to_string(),
                        providers: vec!["ra_ap_syntax".to_owned()],
                        detail: "the same syntax identity was reached from multiple Cargo roots"
                            .to_owned(),
                    })?;
                }
            }
            for relation in crate_relations {
                if seen_relations.insert(relation.id.clone()) {
                    sink.relation(relation)?;
                }
            }
        }
        sink.finish()
    }
}

impl LanguageAdapter for RustWorkspaceAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: "rust".to_owned(),
            version: "workspace-syntax-v1".to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![
            AdapterProvider {
                identity: "cargo-metadata".to_owned(),
                role: ProviderRole::Project,
                capabilities: vec![
                    "workspace".to_owned(),
                    "packages".to_owned(),
                    "targets".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "ra_ap_syntax".to_owned(),
                role: ProviderRole::Syntax,
                capabilities: vec![
                    "syntax".to_owned(),
                    "documentation-association".to_owned(),
                    "module-discovery".to_owned(),
                ],
            },
        ]
    }

    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError> {
        let mut sink = CollectingInventorySink::default();
        self.emit_inventory(source, &mut sink)?;
        sink.into_inventory()
    }

    fn inventory_into(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<(), argus_core::ArgusError> {
        self.emit_inventory(source, sink)
    }
}

fn syntax_partition(path: &SourcePath, syntax: &crate::RustSyntaxInventory) -> DiscoveryPartition {
    let unresolved_modules = syntax.targets.iter().any(|target| {
        matches!(
            target.inventory,
            InventoryState::Unsupported | InventoryState::Failed
        )
    });
    let unresolved_macros = syntax.targets.iter().any(|target| {
        target.capabilities.iter().any(|capability| {
            capability.name == "rust-macro-expansion"
                && capability.status != CapabilityStatus::Complete
        })
    });
    let mut gap_details = syntax.diagnostics.clone();
    if unresolved_modules {
        gap_details.push("one or more Rust modules could not be resolved".to_owned());
    }
    if !syntax.conditions.is_empty() {
        gap_details.push("configuration predicates require compiler evaluation".to_owned());
    }
    if unresolved_macros {
        gap_details.push("macro expansions are not represented".to_owned());
    }
    DiscoveryPartition {
        name: format!("rust-syntax:{}", path.as_str()),
        status: if gap_details.is_empty() {
            CapabilityStatus::Complete
        } else {
            CapabilityStatus::Partial
        },
        diagnostic: (!gap_details.is_empty()).then(|| gap_details.join("; ")),
    }
}

fn cargo_entry(target: &Target) -> Option<(TargetId, SourcePath)> {
    let is_cargo_target = matches!(
        &target.kind,
        TargetKind::LanguageSpecific { language, kind }
            if language == "rust" && kind.starts_with("cargo_target:")
    );
    (is_cargo_target && target.inventory == InventoryState::Represented)
        .then(|| {
            target
                .location
                .as_ref()
                .map(|location| (target.id.clone(), location.path.clone()))
        })
        .flatten()
}

fn contains_relation(
    source: TargetId,
    target: TargetId,
    configuration: ConfigurationId,
) -> Relation {
    Relation {
        id: RelationId::derive([
            source.as_str().as_bytes(),
            target.as_str().as_bytes(),
            b"contains",
        ]),
        source,
        target,
        kind: "core:contains".to_owned(),
        provenance: RelationProvenance {
            provider: "ra_ap_syntax".to_owned(),
            provider_version: "0.0.270".to_owned(),
            configuration: Some(configuration),
            ingest_only: true,
            resolution: argus_core::ResolutionQuality::Exact,
            detail: None,
        },
    }
}
