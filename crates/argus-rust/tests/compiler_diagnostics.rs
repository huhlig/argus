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
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    ResolutionQuality, SnapshotId, SourceLocation, SourcePath, Target, TargetId, TargetKind,
};
use argus_language::SourceAccess;
use argus_rust::{
    CompilerDiagnosticProvider, ControlledExecutor, ExecutionControl, ExecutorCapabilities,
    NetworkAccess, RustTool, ToolExecutionPolicy, ToolLimits, ToolOutput, ToolRequest,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

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
            .ok_or_else(|| argus_core::ArgusError::invalid_input("missing"))
    }
}

fn target(name: &str, kind: PortableTargetKind, start: u64, end: u64) -> Target {
    Target {
        id: TargetId::derive([name.as_bytes()]),
        kind: TargetKind::Portable { kind },
        visibility: argus_core::TargetVisibility::Unknown,
        name: name.to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: SourcePath::new("src/lib.rs").unwrap(),
            bytes: ByteSpan::new(start, end).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: vec![Capability {
            name: "fixture".to_owned(),
            status: CapabilityStatus::Complete,
            detail: None,
            provider: Some("fixture".to_owned()),
        }],
        diagnostic: None,
    }
}

#[test]
fn maps_captured_diagnostics_to_narrowest_target_and_retains_build_failure() {
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"diagnostics".as_slice()]),
        files: BTreeMap::from([(SourcePath::new("src/lib.rs").unwrap(), vec![b' '; 100])]),
    };
    let targets = vec![
        target("src/lib.rs", PortableTargetKind::File, 0, 100),
        target("function", PortableTargetKind::Callable, 10, 30),
    ];
    let stream = br#"{"reason":"compiler-message","message":{"message":"unused variable","code":{"code":"unused_variables"},"level":"warning","spans":[{"file_name":"C:/workspace/src/lib.rs","byte_start":12,"byte_end":15,"is_primary":true}],"rendered":"warning: unused variable"}}
{"reason":"build-finished","success":false}
not-json
"#;
    let provider = CompilerDiagnosticProvider::new(
        stream.to_vec(),
        PathBuf::from("C:/workspace"),
        ConfigurationId::derive([b"default".as_slice()]),
        "cargo-check",
        "rustc 1.85.0",
    );

    let first = provider.ingest(&source, &targets).unwrap();
    let second = provider.ingest(&source, &targets).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.evidence.len(), 2);
    assert_eq!(first.malformed_lines.len(), 1);
    let diagnostic = first
        .evidence
        .iter()
        .find(|record| record.location.is_some())
        .unwrap();
    assert_eq!(diagnostic.target.as_ref(), Some(&targets[1].id));
    assert_eq!(
        diagnostic.provenance.resolution,
        ResolutionQuality::ContainingTarget
    );
    assert!(diagnostic.provenance.ingest_only);
    let failure = first
        .evidence
        .iter()
        .find(|record| record.location.is_none())
        .unwrap();
    assert!(failure.target.is_none());
    assert_eq!(failure.provenance.resolution, ResolutionQuality::Unmapped);
}

#[test]
fn falls_back_to_file_target_when_no_declaration_contains_span() {
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"fallback".as_slice()]),
        files: BTreeMap::from([(SourcePath::new("src/lib.rs").unwrap(), vec![b' '; 100])]),
    };
    let targets = vec![target("src/lib.rs", PortableTargetKind::File, 0, 100)];
    let stream = br#"{"reason":"compiler-message","message":{"message":"crate warning","level":"warning","spans":[{"file_name":"src/lib.rs","byte_start":1,"byte_end":2,"is_primary":true}]}}
"#;
    let provider = CompilerDiagnosticProvider::new(
        stream.to_vec(),
        PathBuf::from("C:/workspace"),
        ConfigurationId::derive([b"default".as_slice()]),
        "clippy",
        "0.1",
    );
    let inventory = provider.ingest(&source, &targets).unwrap();

    assert_eq!(inventory.evidence[0].target.as_ref(), Some(&targets[0].id));
    assert_eq!(
        inventory.evidence[0].provenance.resolution,
        ResolutionQuality::FileFallback
    );
}

struct FakeExecutor {
    capabilities: ExecutorCapabilities,
    output: ToolOutput,
}

impl ControlledExecutor for FakeExecutor {
    fn capabilities(&self) -> ExecutorCapabilities {
        self.capabilities.clone()
    }

    fn execute(&self, _request: &ToolRequest) -> Result<ToolOutput, argus_core::ArgusError> {
        Ok(self.output.clone())
    }
}

fn full_capabilities() -> ExecutorCapabilities {
    ExecutorCapabilities {
        enforced: BTreeSet::from([
            ExecutionControl::ClearEnvironment,
            ExecutionControl::RestrictFilesystem,
            ExecutionControl::RestrictNetwork,
            ExecutionControl::WallTime,
            ExecutionControl::CpuTime,
            ExecutionControl::Memory,
            ExecutionControl::Output,
        ]),
    }
}

fn restricted_request() -> ToolRequest {
    ToolRequest {
        tool: RustTool::CargoCheck,
        workspace_root: PathBuf::from("C:/workspace"),
        policy: ToolExecutionPolicy {
            limits: ToolLimits {
                wall_time_millis: 30_000,
                cpu_time_millis: 20_000,
                memory_bytes: 512 * 1024 * 1024,
                output_bytes: 1024 * 1024,
            },
            network: NetworkAccess::Denied,
            environment: BTreeMap::from([("CARGO_TERM_COLOR".to_owned(), "never".to_owned())]),
            writable_roots: vec![PathBuf::from("C:/workspace/target")],
        },
    }
}

#[test]
fn controlled_execution_matches_ingested_normalization() {
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"active".as_slice()]),
        files: BTreeMap::from([(SourcePath::new("src/lib.rs").unwrap(), vec![b' '; 100])]),
    };
    let targets = vec![target("src/lib.rs", PortableTargetKind::File, 0, 100)];
    let stream = br#"{"reason":"compiler-message","message":{"message":"warning","level":"warning","spans":[{"file_name":"src/lib.rs","byte_start":1,"byte_end":2,"is_primary":true}]}}
"#
    .to_vec();
    let provider = CompilerDiagnosticProvider::new(
        stream.clone(),
        PathBuf::from("C:/workspace"),
        ConfigurationId::derive([b"controlled".as_slice()]),
        "cargo-check",
        "rustc 1.85.0",
    );
    let ingested = provider.ingest(&source, &targets).unwrap();
    let executor = FakeExecutor {
        capabilities: full_capabilities(),
        output: ToolOutput {
            success: true,
            stdout: stream,
            stderr: Vec::new(),
            timed_out: false,
            resource_limit_exceeded: false,
        },
    };
    let active = provider
        .execute(&executor, &restricted_request(), &source, &targets)
        .unwrap();

    assert_eq!(ingested.evidence.len(), active.evidence.len());
    assert_eq!(ingested.evidence[0].id, active.evidence[0].id);
    assert_eq!(ingested.evidence[0].target, active.evidence[0].target);
    assert!(ingested.evidence[0].provenance.ingest_only);
    assert!(!active.evidence[0].provenance.ingest_only);
}

#[test]
fn restricted_execution_fails_closed_when_network_control_is_missing() {
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"fail-closed".as_slice()]),
        files: BTreeMap::new(),
    };
    let provider = CompilerDiagnosticProvider::new(
        Vec::new(),
        PathBuf::from("C:/workspace"),
        ConfigurationId::derive([b"controlled".as_slice()]),
        "cargo-check",
        "rustc 1.85.0",
    );
    let mut capabilities = full_capabilities();
    capabilities
        .enforced
        .remove(&ExecutionControl::RestrictNetwork);
    let executor = FakeExecutor {
        capabilities,
        output: ToolOutput {
            success: true,
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
            resource_limit_exceeded: false,
        },
    };

    let error = provider
        .execute(&executor, &restricted_request(), &source, &[])
        .unwrap_err();
    assert_eq!(error.code(), argus_core::ErrorCode::Unsupported);
}
