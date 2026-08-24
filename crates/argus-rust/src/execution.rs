use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustTool {
    CargoCheck,
    Clippy,
    Tests,
    Doctests,
    Rustdoc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkAccess {
    Denied,
    Allowed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolLimits {
    pub wall_time_millis: u64,
    pub cpu_time_millis: u64,
    pub memory_bytes: u64,
    pub output_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolExecutionPolicy {
    pub limits: ToolLimits,
    pub network: NetworkAccess,
    /// The complete environment supplied after clearing the inherited environment.
    pub environment: BTreeMap<String, String>,
    pub writable_roots: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExecutionControl {
    ClearEnvironment,
    RestrictFilesystem,
    RestrictNetwork,
    WallTime,
    CpuTime,
    Memory,
    Output,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutorCapabilities {
    pub enforced: BTreeSet<ExecutionControl>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolRequest {
    pub tool: RustTool,
    pub workspace_root: PathBuf,
    pub policy: ToolExecutionPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub resource_limit_exceeded: bool,
}

pub trait ControlledExecutor: Send + Sync {
    fn capabilities(&self) -> ExecutorCapabilities;
    fn execute(&self, request: &ToolRequest) -> Result<ToolOutput, argus_core::ArgusError>;
}

pub fn validate_execution_request(
    capabilities: &ExecutorCapabilities,
    request: &ToolRequest,
) -> Result<(), argus_core::ArgusError> {
    let policy = &request.policy;
    let controls = [
        (ExecutionControl::ClearEnvironment, "environment clearing"),
        (
            ExecutionControl::RestrictFilesystem,
            "filesystem restriction",
        ),
        (ExecutionControl::WallTime, "wall-time limit"),
        (ExecutionControl::CpuTime, "CPU-time limit"),
        (ExecutionControl::Memory, "memory limit"),
        (ExecutionControl::Output, "output limit"),
    ];
    if let Some((_, label)) = controls
        .into_iter()
        .find(|(control, _)| !capabilities.enforced.contains(control))
    {
        return Err(argus_core::ArgusError::unsupported(format!(
            "executor cannot enforce required {label}"
        )));
    }
    if policy.network == NetworkAccess::Denied
        && !capabilities
            .enforced
            .contains(&ExecutionControl::RestrictNetwork)
    {
        return Err(argus_core::ArgusError::unsupported(
            "executor cannot enforce denied network access",
        ));
    }
    if policy.limits.wall_time_millis == 0
        || policy.limits.cpu_time_millis == 0
        || policy.limits.memory_bytes == 0
        || policy.limits.output_bytes == 0
        || policy.writable_roots.is_empty()
        || policy.writable_roots.iter().any(|path| !path.is_absolute())
    {
        return Err(argus_core::ArgusError::invalid_input(
            "active tool policy requires positive limits and absolute writable roots",
        ));
    }
    if policy.environment.keys().any(|name| {
        name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }) {
        return Err(argus_core::ArgusError::invalid_input(
            "active tool environment contains an invalid variable name",
        ));
    }
    Ok(())
}

pub fn validate_tool_output(
    request: &ToolRequest,
    output: &ToolOutput,
) -> Result<(), argus_core::ArgusError> {
    if output.timed_out {
        return Err(argus_core::ArgusError::unsupported(
            "repository tool exceeded its wall-time limit",
        ));
    }
    if output.resource_limit_exceeded {
        return Err(argus_core::ArgusError::unsupported(
            "repository tool exceeded a resource limit",
        ));
    }
    if output.stdout.len().saturating_add(output.stderr.len()) > request.policy.limits.output_bytes
    {
        return Err(argus_core::ArgusError::unsupported(
            "repository tool exceeded its output limit",
        ));
    }
    Ok(())
}
