use argus_core::{
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    Relation, RelationId, SourceLocation, SourcePath, Target, TargetId, TargetKind,
    TargetVisibility,
};
use argus_language::{
    AdapterIdentity, AdapterInventory, AdapterProvider, DiscoveryPartition, LanguageAdapter,
    ProviderRole, SourceAccess,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct CargoMetadataAdapter {
    metadata_json: Vec<u8>,
    configuration: ConfigurationId,
}

impl CargoMetadataAdapter {
    #[must_use]
    pub fn new(metadata_json: Vec<u8>, configuration: ConfigurationId) -> Self {
        Self {
            metadata_json,
            configuration,
        }
    }
}

impl LanguageAdapter for CargoMetadataAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: "rust".to_owned(),
            version: "cargo-metadata-v1".to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![AdapterProvider {
            identity: "cargo-metadata".to_owned(),
            role: ProviderRole::Project,
            capabilities: vec![
                "workspace".to_owned(),
                "packages".to_owned(),
                "targets".to_owned(),
            ],
        }]
    }

    #[allow(clippy::too_many_lines)]
    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError> {
        let metadata: Metadata = serde_json::from_slice(&self.metadata_json).map_err(|error| {
            argus_core::ArgusError::invalid_input("invalid cargo metadata JSON").with_source(error)
        })?;
        let root = PathBuf::from(&metadata.workspace_root);
        let members = metadata
            .workspace_members
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let package_ids = metadata
            .packages
            .iter()
            .filter(|package| members.contains(&package.id))
            .map(|package| {
                (
                    package.id.clone(),
                    TargetId::derive([
                        b"rust".as_slice(),
                        source.snapshot_id().as_str().as_bytes(),
                        self.configuration.as_str().as_bytes(),
                        b"package".as_slice(),
                        package.id.as_bytes(),
                    ]),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut targets = Vec::new();
        let mut relations = Vec::new();
        for package in metadata
            .packages
            .into_iter()
            .filter(|package| members.contains(&package.id))
        {
            let package_id = package_ids[&package.id].clone();
            targets.push(Target {
                id: package_id.clone(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Package,
                },
                visibility: TargetVisibility::NotApplicable,
                name: package.name,
                parent: None,
                location: normalized_location(&root, &package.manifest_path, source)?,
                inventory: InventoryState::Represented,
                capabilities: vec![cargo_capability()],
                diagnostic: None,
            });
            for feature in package.features.keys() {
                let feature_id = TargetId::derive([
                    b"rust".as_slice(),
                    source.snapshot_id().as_str().as_bytes(),
                    self.configuration.as_str().as_bytes(),
                    b"cargo-feature".as_slice(),
                    package.id.as_bytes(),
                    feature.as_bytes(),
                ]);
                targets.push(Target {
                    id: feature_id.clone(),
                    kind: TargetKind::LanguageSpecific {
                        language: "rust".to_owned(),
                        kind: "cargo_feature".to_owned(),
                    },
                    visibility: TargetVisibility::NotApplicable,
                    name: feature.clone(),
                    parent: Some(package_id.clone()),
                    location: normalized_location(&root, &package.manifest_path, source)?,
                    inventory: InventoryState::Represented,
                    capabilities: vec![cargo_capability()],
                    diagnostic: None,
                });
                relations.push(cargo_relation(
                    package_id.clone(),
                    feature_id,
                    "core:contains",
                    self.configuration.clone(),
                ));
            }
            for cargo_target in package.targets {
                let target_id = TargetId::derive([
                    b"rust".as_slice(),
                    source.snapshot_id().as_str().as_bytes(),
                    self.configuration.as_str().as_bytes(),
                    b"cargo-target".as_slice(),
                    package.id.as_bytes(),
                    cargo_target.name.as_bytes(),
                    cargo_target.kind.join(",").as_bytes(),
                ]);
                let location = normalized_location(&root, &cargo_target.src_path, source)?;
                let (inventory, diagnostic) = if location.is_some() {
                    (InventoryState::Represented, None)
                } else {
                    (
                        InventoryState::Unsupported,
                        Some(
                            "Cargo target source is absent from the immutable snapshot".to_owned(),
                        ),
                    )
                };
                targets.push(Target {
                    id: target_id.clone(),
                    kind: TargetKind::LanguageSpecific {
                        language: "rust".to_owned(),
                        kind: format!("cargo_target:{}", cargo_target.kind.join(",")),
                    },
                    visibility: TargetVisibility::NotApplicable,
                    name: cargo_target.name,
                    parent: Some(package_id.clone()),
                    location,
                    inventory,
                    capabilities: vec![cargo_capability()],
                    diagnostic,
                });
                relations.push(cargo_relation(
                    package_id.clone(),
                    target_id,
                    "core:contains",
                    self.configuration.clone(),
                ));
            }
        }
        if let Some(resolve) = metadata.resolve {
            for node in resolve.nodes {
                let Some(source_id) = package_ids.get(&node.id) else {
                    continue;
                };
                for dependency in node.dependencies {
                    if let Some(target_id) = package_ids.get(&dependency) {
                        relations.push(cargo_relation(
                            source_id.clone(),
                            target_id.clone(),
                            "rust:depends_on",
                            self.configuration.clone(),
                        ));
                    }
                }
            }
        }
        Ok(AdapterInventory {
            adapter: self.identity(),
            snapshot: source.snapshot_id().clone(),
            partitions: vec![DiscoveryPartition {
                name: "cargo-workspace".to_owned(),
                status: CapabilityStatus::Complete,
                diagnostic: None,
            }],
            targets,
            evidence: Vec::new(),
            relations,
            conflicts: vec![],
        })
    }
}

fn cargo_relation(
    source: TargetId,
    target: TargetId,
    kind: &str,
    configuration: ConfigurationId,
) -> Relation {
    let identity_kind = if kind == "core:contains" {
        "contains"
    } else {
        kind
    };
    Relation {
        id: RelationId::derive([
            source.as_str().as_bytes(),
            target.as_str().as_bytes(),
            identity_kind.as_bytes(),
        ]),
        source,
        target,
        kind: kind.to_owned(),
        provenance: argus_core::RelationProvenance {
            provider: "cargo-metadata".to_owned(),
            provider_version: "1".to_owned(),
            configuration: Some(configuration),
            ingest_only: true,
            resolution: argus_core::ResolutionQuality::Exact,
            detail: None,
        },
    }
}

fn cargo_capability() -> Capability {
    Capability {
        name: "cargo-project-model".to_owned(),
        status: CapabilityStatus::Complete,
        detail: None,
        provider: Some("cargo-metadata".to_owned()),
    }
}

fn normalized_location(
    root: &Path,
    absolute: &str,
    source: &dyn SourceAccess,
) -> Result<Option<SourceLocation>, argus_core::ArgusError> {
    let path = Path::new(absolute);
    let relative = path.strip_prefix(root).map_err(|_| {
        argus_core::ArgusError::invalid_input("cargo metadata path escapes workspace root")
    })?;
    let source_path = SourcePath::new(relative.to_string_lossy().replace('\\', "/"))?;
    Ok(source.contains(&source_path).then_some(SourceLocation {
        path: source_path,
        bytes: ByteSpan::new(0, 0)?,
        start: None,
        end: None,
    }))
}

#[derive(Deserialize)]
struct Metadata {
    workspace_root: String,
    workspace_members: Vec<String>,
    packages: Vec<Package>,
    #[serde(default)]
    resolve: Option<Resolve>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: String,
    targets: Vec<CargoTarget>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<ResolveNode>,
}

#[derive(Deserialize)]
struct ResolveNode {
    id: String,
    #[serde(default)]
    dependencies: Vec<String>,
}
