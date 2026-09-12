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

use argus_core::{
    ByteSpan, InventoryState, PortableTargetKind, SourceLocation, SourcePath, Target, TargetId,
    TargetKind, TargetVisibility,
};
use argus_evidence::{
    DesignArtifactIndex, DesignArtifactParser, DesignLinkKind, DesignLinkageEngine,
};
use std::fs;
use std::path::Path;

fn dummy_target(id_str: &str, name: &str, path_str: &str) -> Target {
    let source_path = SourcePath::new(path_str).unwrap();
    Target {
        id: TargetId::derive([id_str.as_bytes(), path_str.as_bytes()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Type,
        },
        visibility: TargetVisibility::Public,
        name: name.to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: source_path,
            bytes: ByteSpan::new(0, 100).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    }
}

#[test]
fn explicit_governs_linkage() {
    let mut artifact_index = DesignArtifactIndex::new();
    let adr_content = r"---
id: ADR-0002
status: accepted
date: 2026-08-24
governs:
  - crates/argus-storage
---
# ADR 0002: Storage Engine
## Decision
Use redb.
";
    let artifact = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0002-redb.md").unwrap(),
        adr_content,
    )
    .unwrap();
    artifact_index.insert(artifact);

    let target_storage = dummy_target(
        "storage-target",
        "argus_storage::store::DurableStore",
        "crates/argus-storage/src/store.rs",
    );
    let target_cli = dummy_target("cli-target", "argus_cli::main", "crates/argus-cli/src/main.rs");

    let targets = vec![target_storage.clone(), target_cli.clone()];
    let engine = DesignLinkageEngine::new();
    let linkage = engine.link(&artifact_index, &targets).unwrap();

    assert_eq!(linkage.len(), 1);

    let storage_links = linkage.links_for_target(&target_storage.id);
    assert_eq!(storage_links.len(), 1);
    let link = storage_links[0];
    assert!(link.origin.is_explicit());
    assert_eq!(link.confidence.basis_points(), 10_000);
    assert_eq!(link.kind, DesignLinkKind::Governs);

    let cli_links = linkage.links_for_target(&target_cli.id);
    assert!(cli_links.is_empty());

    let governed = linkage.targets_governed_by(&link.artifact_id);
    assert_eq!(governed.len(), 1);
    assert_eq!(governed[0], &target_storage.id);
}

#[test]
fn inferred_symbol_and_concept_linkage() {
    let mut artifact_index = DesignArtifactIndex::new();
    let adr_content = r"# ADR 0005: Use Langchart adapters for LLM wire transport
- Status: Accepted
- Date: 2026-08-24

## Decision
Provider wire transports implement Langchart's LlmAdapter contract.
The PrimaryReviewActor binds its sealed invocation capability envelope.
";
    let artifact = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0005-llm-transport.md").unwrap(),
        adr_content,
    )
    .unwrap();
    let artifact_id = artifact.id.clone();
    artifact_index.insert(artifact);

    let target_adapter = dummy_target(
        "adapter-target",
        "argus_provider::LlmAdapter",
        "crates/argus-provider/src/adapter.rs",
    );
    let target_actor = dummy_target(
        "actor-target",
        "argus_workflow::PrimaryReviewActor",
        "crates/argus-workflow/src/actor.rs",
    );
    let target_unrelated = dummy_target(
        "unrelated-target",
        "argus_core::UnrelatedSymbol",
        "crates/argus-core/src/other.rs",
    );

    let targets = vec![
        target_adapter.clone(),
        target_actor.clone(),
        target_unrelated.clone(),
    ];
    let engine = DesignLinkageEngine::new();
    let linkage = engine.link(&artifact_index, &targets).unwrap();

    // LlmAdapter and PrimaryReviewActor should both be linked via inferred symbol matching
    let adapter_links = linkage.links_for_target(&target_adapter.id);
    assert_eq!(adapter_links.len(), 1);
    assert!(adapter_links[0].origin.is_inferred());
    assert_eq!(adapter_links[0].confidence.basis_points(), 8_500);

    let actor_links = linkage.links_for_target(&target_actor.id);
    assert_eq!(actor_links.len(), 1);
    assert!(actor_links[0].origin.is_inferred());
    assert_eq!(actor_links[0].confidence.basis_points(), 8_500);

    // Unrelated should have no link
    assert!(linkage.links_for_target(&target_unrelated.id).is_empty());

    // Both should be returned by targets_governed_by
    let governed = linkage.targets_governed_by(&artifact_id);
    assert_eq!(governed.len(), 2);
}

#[test]
fn suppression_below_confidence_threshold() {
    let mut artifact_index = DesignArtifactIndex::new();
    let adr_content = r"# ADR 0001: General Rules
- Status: Accepted
- Date: 2026-08-24

## Decision
Nothing specific here.
";
    let artifact = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0001-general.md").unwrap(),
        adr_content,
    )
    .unwrap();
    artifact_index.insert(artifact);

    let target = dummy_target(
        "irrelevant-target",
        "crate::some::unrelated::component",
        "crates/other/src/component.rs",
    );

    let engine = DesignLinkageEngine::with_min_confidence(8_000);
    let linkage = engine.link(&artifact_index, &[target]).unwrap();
    assert!(linkage.is_empty());
}

#[test]
fn real_repository_adrs_linkage() {
    let adr_dir = Path::new("../../docs/adr");
    if !adr_dir.exists() {
        let adr_dir = Path::new("docs/adr");
        if !adr_dir.exists() {
            return;
        }
    }

    let mut artifact_index = DesignArtifactIndex::new();
    for entry in fs::read_dir(adr_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "md") {
            let file_name = path.file_name().unwrap().to_str().unwrap();
            let source_path = SourcePath::new(format!("docs/adr/{file_name}")).unwrap();
            let content = fs::read_to_string(&path).unwrap();
            let artifact = DesignArtifactParser::parse(source_path, &content).unwrap();
            artifact_index.insert(artifact);
        }
    }

    // Targets representing concepts from ADR 0001 (identifiers) and ADR 0005 (llm transport)
    let target_id_type = dummy_target(
        "target-id-type",
        "argus_core::TargetId",
        "crates/argus-core/src/id.rs",
    );
    let target_transport = dummy_target(
        "target-llm-adapter",
        "argus_provider::LlmAdapter",
        "crates/argus-provider/src/adapter.rs",
    );

    let targets = vec![target_id_type.clone(), target_transport.clone()];
    let engine = DesignLinkageEngine::new();
    let linkage = engine.link(&artifact_index, &targets).unwrap();

    assert!(!linkage.is_empty());

    // target_transport should link to ADR 0005
    let transport_links = linkage.links_for_target(&target_transport.id);
    assert!(!transport_links.is_empty());
    assert!(transport_links.iter().any(|l| l.artifact_identifier.as_deref() == Some("ADR-0005")));
}
