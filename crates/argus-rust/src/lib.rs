//! Rust adapter built from captured Cargo and Rust-tooling outputs.

mod cargo;
mod diagnostics;
mod execution;
mod relationships;
mod rustdoc;
mod syntax;
mod tool_evidence;
mod workspace;

pub use cargo::CargoMetadataAdapter;
pub use diagnostics::{CompilerDiagnosticProvider, RustDiagnosticInventory};
pub use execution::{
    ControlledExecutor, ExecutionControl, ExecutorCapabilities, NetworkAccess, RustTool,
    ToolExecutionPolicy, ToolLimits, ToolOutput, ToolRequest, validate_execution_request,
    validate_tool_output,
};
pub use relationships::{
    RejectedRelationship, RustRelationshipInventory, RustRelationshipProvider,
};
pub use rustdoc::{RustdocEvidenceInventory, RustdocJsonProvider};
pub use syntax::{RustEdition, RustSyntaxInventory, RustSyntaxProvider};
pub use tool_evidence::{RustToolEvidenceInventory, RustToolEvidenceProvider};
pub use workspace::RustWorkspaceAdapter;
