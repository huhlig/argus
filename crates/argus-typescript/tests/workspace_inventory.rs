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

use argus_core::{ConfigurationId, EvidenceKind, PortableTargetKind, SnapshotId, SourcePath, TargetKind};
use argus_language::{LanguageAdapter, SourceAccess, normalize_inventory};
use argus_typescript::TypeScriptWorkspaceAdapter;
use std::collections::BTreeMap;

struct MemorySource {
    snapshot: SnapshotId,
    files: BTreeMap<SourcePath, Vec<u8>>,
}

impl SourceAccess for MemorySource {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.snapshot
    }

    fn contains(&self, path: &SourcePath) -> bool {
        self.files.contains_key(path)
    }

    fn read(&self, path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| argus_core::ArgusError::invalid_input("missing source"))
    }
}

#[test]
fn end_to_end_workspace_inventory_normalization_and_idempotency() {
    let pkg_json = r#"{
        "name": "sample-app",
        "version": "1.0.0",
        "main": "src/index.ts"
    }"#;

    let index_ts = r#"
import { Logger } from './logger';

/**
 * Service orchestrator.
 */
export class AppService {
    private logger: Logger;

    constructor() {
        this.logger = new Logger();
    }

    /**
     * Starts the service.
     */
    public start(): void {
        this.logger.log("Service starting");
    }
}
"#;

    let logger_ts = r#"
/**
 * Application logger.
 */
export class Logger {
    public log(msg: string): void {
        console.log(msg);
    }
}
"#;

    let pkg_path = SourcePath::new("package.json").unwrap();
    let index_path = SourcePath::new("src/index.ts").unwrap();
    let logger_path = SourcePath::new("src/logger.ts").unwrap();

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-workspace-e2e".as_slice()]),
        files: BTreeMap::from([
            (pkg_path.clone(), pkg_json.as_bytes().to_vec()),
            (index_path.clone(), index_ts.as_bytes().to_vec()),
            (logger_path.clone(), logger_ts.as_bytes().to_vec()),
        ]),
    };

    let cfg = ConfigurationId::derive([b"default".as_slice()]);
    let adapter = TypeScriptWorkspaceAdapter::new(
        cfg,
        vec![pkg_path, index_path, logger_path],
    );

    let first = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();
    let second = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();

    // Verify idempotency
    assert_eq!(first, second);
    assert!(first.conflicts.is_empty());

    // Verify workspace target, package target, file targets, classes, methods
    assert!(first.targets.iter().any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Workspace })));
    assert!(first.targets.iter().any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Package })));
    assert!(first.targets.iter().any(|t| t.name == "AppService"));
    assert!(first.targets.iter().any(|t| t.name == "Logger"));
    assert!(first.targets.iter().any(|t| t.name == "start"));
    assert!(first.targets.iter().any(|t| t.name == "log"));

    // Verify documentation evidence exists
    let app_service_target = first.targets.iter().find(|t| t.name == "AppService").unwrap();
    let doc_evidence = first
        .evidence
        .iter()
        .find(|e| e.target.as_ref() == Some(&app_service_target.id) && e.kind == EvidenceKind::Documentation)
        .expect("doc evidence exists");
    assert!(doc_evidence.detail.as_deref().unwrap().contains("Service orchestrator."));

    // Verify relations:
    // - AppService contains start
    // - Logger contains log
    // - src/index.ts imports src/logger.ts
    let start_target = first.targets.iter().find(|t| t.name == "start").unwrap();
    assert!(
        first
            .relations
            .iter()
            .any(|r| r.kind == "core:contains"
                && r.source == app_service_target.id
                && r.target == start_target.id),
        "AppService contains start relation exists"
    );

    let import_rel = first
        .relations
        .iter()
        .find(|r| r.kind == "core:imports")
        .expect("import relation exists");
    let index_file = first.targets.iter().find(|t| t.name == "src/index.ts").unwrap();
    let logger_file = first.targets.iter().find(|t| t.name == "src/logger.ts").unwrap();
    assert_eq!(import_rel.source, index_file.id);
    assert_eq!(import_rel.target, logger_file.id);
}
