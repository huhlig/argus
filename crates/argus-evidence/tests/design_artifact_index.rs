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

use argus_core::SourcePath;
use argus_evidence::{
    DesignArtifactIndex, DesignArtifactKind, DesignArtifactParser, DesignStatus,
    DocumentHealthIssueKind, DocumentHealthSeverity, RequirementLevel,
};
use std::fs;
use std::path::Path;

#[test]
fn ingest_repository_adrs() {
    let adr_dir = Path::new("../../docs/adr");
    if !adr_dir.exists() {
        // In case tests are executed from workspace root
        let adr_dir = Path::new("docs/adr");
        if !adr_dir.exists() {
            return;
        }
    }

    let mut index = DesignArtifactIndex::new();
    let mut count = 0;

    for entry in fs::read_dir(adr_dir).expect("cannot read docs/adr") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            let file_name = path.file_name().unwrap().to_str().unwrap();
            let source_path = SourcePath::new(format!("docs/adr/{file_name}")).unwrap();
            let content = fs::read_to_string(&path).expect("cannot read ADR file");
            let artifact = DesignArtifactParser::parse(source_path, &content).unwrap();
            assert_eq!(artifact.kind, DesignArtifactKind::Adr);
            assert!(artifact.status.is_active());
            assert_eq!(artifact.date.as_deref(), Some("2026-08-24"));
            assert!(!artifact.sections.is_empty());
            index.insert(artifact);
            count += 1;
        }
    }

    assert_eq!(count, 5);
    assert_eq!(index.len(), 5);
    assert_eq!(index.active_adrs().len(), 5);

    // Verify lookup by identifier
    let adr_0001 = index.get_by_identifier("ADR-0001");
    assert!(adr_0001.is_some());
    let adr_0001 = adr_0001.unwrap();
    assert!(adr_0001.title.contains("Stable identifiers"));

    let adr_0005 = index.get_by_identifier("0005");
    assert!(adr_0005.is_some());
    assert!(adr_0005.unwrap().title.contains("Langchart"));

    // Verify clean health
    let issues = index.validate_health();
    let errors: Vec<_> = issues
        .into_iter()
        .filter(|i| i.severity == DocumentHealthSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "ADRs in docs/adr should have zero health errors: {errors:?}");
}

#[test]
fn parse_frontmatter_and_requirements() {
    let content = r#"---
title: "PRD: Core Engine Architecture"
id: PRD-001
status: accepted
date: 2026-09-01
governs:
  - crates/argus-core
  - crates/argus-evidence
authors:
  - Hans W. Uhlig
tags:
  - core
  - architecture
---

# Core Engine Architecture

## Overview
This is the core specification.

## Normative Requirements
- The storage system MUST enforce atomic transactions.
- Implementations SHOULD log warnings when approaching resource limits.
- Callers MAY specify custom timeout parameters.
"#;

    let path = SourcePath::new("docs/prd/core.md").unwrap();
    let artifact = DesignArtifactParser::parse(path, content).unwrap();

    assert_eq!(artifact.kind, DesignArtifactKind::Prd);
    assert_eq!(artifact.title, "PRD: Core Engine Architecture");
    assert_eq!(artifact.identifier.as_deref(), Some("PRD-001"));
    assert_eq!(artifact.status, DesignStatus::Accepted);
    assert_eq!(artifact.date.as_deref(), Some("2026-09-01"));
    assert_eq!(artifact.authors, vec!["Hans W. Uhlig".to_owned()]);
    assert_eq!(
        artifact.governs,
        vec!["crates/argus-core".to_owned(), "crates/argus-evidence".to_owned()]
    );
    assert!(artifact.tags.contains("core"));
    assert!(artifact.tags.contains("architecture"));

    let req_section = artifact
        .sections
        .iter()
        .find(|s| s.id == "normative-requirements")
        .expect("normative-requirements section");

    assert_eq!(req_section.requirements.len(), 3);
    assert_eq!(req_section.requirements[0].level, RequirementLevel::Must);
    assert_eq!(req_section.requirements[1].level, RequirementLevel::Should);
    assert_eq!(req_section.requirements[2].level, RequirementLevel::May);
}

#[test]
fn health_check_duplicate_identifiers() {
    let mut index = DesignArtifactIndex::new();
    let adr_1 = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0001-first.md").unwrap(),
        "# ADR 0001: First Decision\n- Status: Accepted\n- Date: 2026-08-24\n## Decision\nDo it.",
    )
    .unwrap();

    let adr_2 = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0001-second.md").unwrap(),
        "# ADR 0001: Duplicate Decision\n- Status: Accepted\n- Date: 2026-08-24\n## Decision\nDo it again.",
    )
    .unwrap();

    index.insert(adr_1);
    index.insert(adr_2);

    let issues = index.validate_health();
    assert!(issues.iter().any(|i| i.kind == DocumentHealthIssueKind::DuplicateIdentifier));
}

#[test]
fn health_check_missing_metadata() {
    let mut index = DesignArtifactIndex::new();
    let adr = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0010-no-status.md").unwrap(),
        "# ADR 0010: Decision Without Status\n## Decision\nSomething.",
    )
    .unwrap();

    index.insert(adr);
    let issues = index.validate_health();
    assert!(issues.iter().any(|i| i.kind == DocumentHealthIssueKind::MissingRequiredMetadata));
}

#[test]
fn health_check_broken_supersession() {
    let mut index = DesignArtifactIndex::new();
    let adr = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0002-superseding.md").unwrap(),
        "# ADR 0002: New Storage\n- Status: Accepted\n- Date: 2026-08-24\n- Supersedes: ADR 9999\n## Decision\nNew.",
    )
    .unwrap();

    index.insert(adr);
    let issues = index.validate_health();
    assert!(issues.iter().any(|i| i.kind == DocumentHealthIssueKind::BrokenSupersession));
}

#[test]
fn health_check_supersession_cycle_and_chain() {
    let mut index = DesignArtifactIndex::new();
    let adr_1 = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0001.md").unwrap(),
        "# ADR 0001: Decision 1\n- Status: Superseded by ADR 0002\n- Date: 2026-08-24\n- Supersedes: ADR 0002\n## Decision\nA.",
    )
    .unwrap();

    let adr_2 = DesignArtifactParser::parse(
        SourcePath::new("docs/adr/0002.md").unwrap(),
        "# ADR 0002: Decision 2\n- Status: Accepted\n- Date: 2026-08-25\n- Supersedes: ADR 0001\n## Decision\nB.",
    )
    .unwrap();

    index.insert(adr_1);
    index.insert(adr_2);

    let issues = index.validate_health();
    assert!(issues.iter().any(|i| i.kind == DocumentHealthIssueKind::SupersessionCycle));
}
