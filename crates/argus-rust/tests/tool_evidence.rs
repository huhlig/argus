use argus_core::{
    Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    ResolutionQuality, SnapshotId, Target, TargetId, TargetKind,
};
use argus_language::SourceAccess;
use argus_rust::{
    ControlledExecutor, ExecutionControl, ExecutorCapabilities, NetworkAccess, RustTool,
    RustToolEvidenceProvider, ToolExecutionPolicy, ToolLimits, ToolOutput, ToolRequest,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

struct EmptySource(SnapshotId);

impl SourceAccess for EmptySource {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.0
    }
    fn contains(&self, _path: &argus_core::SourcePath) -> bool {
        false
    }
    fn read(&self, _path: &argus_core::SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        Err(argus_core::ArgusError::invalid_input("missing"))
    }
}

fn target(name: &str) -> Target {
    Target {
        id: TargetId::derive([name.as_bytes()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Test,
        },
        visibility: argus_core::TargetVisibility::Unknown,
        name: name.to_owned(),
        parent: None,
        location: None,
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

struct FakeExecutor(ToolOutput);

impl ControlledExecutor for FakeExecutor {
    fn capabilities(&self) -> ExecutorCapabilities {
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

    fn execute(&self, _request: &ToolRequest) -> Result<ToolOutput, argus_core::ArgusError> {
        Ok(self.0.clone())
    }
}

fn request(tool: RustTool) -> ToolRequest {
    ToolRequest {
        tool,
        workspace_root: PathBuf::from("C:/workspace"),
        policy: ToolExecutionPolicy {
            limits: ToolLimits {
                wall_time_millis: 1_000,
                cpu_time_millis: 1_000,
                memory_bytes: 1_000_000,
                output_bytes: 10_000,
            },
            network: NetworkAccess::Denied,
            environment: BTreeMap::default(),
            writable_roots: vec![PathBuf::from("C:/workspace/target")],
        },
    }
}

#[test]
fn normalizes_targeted_results_and_isolates_invalid_lines() {
    let source = EmptySource(SnapshotId::derive([b"tools".as_slice()]));
    let target = target("unit_test");
    let unknown = TargetId::derive([b"unknown".as_slice()]);
    let stream = format!(
        "{{\"target\":\"{}\",\"status\":\"failed\",\"summary\":\"assertion failed\"}}\nnot-json\n{{\"target\":\"{}\",\"status\":\"passed\",\"summary\":\"unknown\"}}\n",
        target.id, unknown
    );
    let inventory = RustToolEvidenceProvider::new(
        RustTool::Tests,
        stream.into_bytes(),
        ConfigurationId::derive([b"default".as_slice()]),
        "cargo-test-adapter",
        "1",
    )
    .ingest(&source, std::slice::from_ref(&target))
    .unwrap();

    assert_eq!(inventory.evidence.len(), 1);
    assert_eq!(inventory.rejected.len(), 2);
    assert_eq!(inventory.evidence[0].target, Some(target.id));
    assert_eq!(
        inventory.evidence[0].provenance.resolution,
        ResolutionQuality::Exact
    );
    assert!(inventory.evidence[0].provenance.ingest_only);
}

#[test]
fn active_and_ingest_only_results_share_identity_and_failed_runs_are_evidence() {
    let source = EmptySource(SnapshotId::derive([b"tools".as_slice()]));
    let target = target("doctest");
    let stream = format!(
        "{{\"target\":\"{}\",\"status\":\"failed\",\"summary\":\"example failed\"}}\n",
        target.id
    )
    .into_bytes();
    let provider = RustToolEvidenceProvider::new(
        RustTool::Doctests,
        stream.clone(),
        ConfigurationId::derive([b"default".as_slice()]),
        "cargo-test-adapter",
        "1",
    );
    let ingested = provider
        .ingest(&source, std::slice::from_ref(&target))
        .unwrap();
    let active = provider
        .execute(
            &FakeExecutor(ToolOutput {
                success: false,
                stdout: stream,
                stderr: Vec::new(),
                timed_out: false,
                resource_limit_exceeded: false,
            }),
            &request(RustTool::Doctests),
            &source,
            &[target],
        )
        .unwrap();

    assert!(
        active
            .evidence
            .iter()
            .any(|record| record.id == ingested.evidence[0].id)
    );
    assert_eq!(active.evidence.len(), 2);
    assert!(
        active
            .evidence
            .iter()
            .all(|record| !record.provenance.ingest_only)
    );
    assert!(
        active
            .evidence
            .iter()
            .any(|record| record.summary.contains("unsuccessful run"))
    );
}

#[test]
fn rejects_a_mismatched_active_tool_request() {
    let source = EmptySource(SnapshotId::derive([b"tools".as_slice()]));
    let provider = RustToolEvidenceProvider::new(
        RustTool::Rustdoc,
        Vec::new(),
        ConfigurationId::derive([b"default".as_slice()]),
        "rustdoc-adapter",
        "1",
    );
    let error = provider
        .execute(
            &FakeExecutor(ToolOutput {
                success: true,
                stdout: Vec::new(),
                stderr: Vec::new(),
                timed_out: false,
                resource_limit_exceeded: false,
            }),
            &request(RustTool::Tests),
            &source,
            &[],
        )
        .unwrap_err();
    assert!(error.to_string().contains("does not match"));
}
