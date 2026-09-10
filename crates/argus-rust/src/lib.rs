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

//! Rust adapter built from captured Cargo and Rust-tooling outputs.

mod cargo;
mod diagnostics;
mod execution;
mod native_relationships;
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
pub use native_relationships::{NativeRustRelationshipInventory, NativeRustRelationshipProvider};
pub use relationships::{
    RejectedRelationship, RustRelationshipInventory, RustRelationshipProvider,
};
pub use rustdoc::{RustdocEvidenceInventory, RustdocJsonProvider};
pub use syntax::{RustEdition, RustSyntaxInventory, RustSyntaxProvider};
pub use tool_evidence::{RustToolEvidenceInventory, RustToolEvidenceProvider};
pub use workspace::RustWorkspaceAdapter;
