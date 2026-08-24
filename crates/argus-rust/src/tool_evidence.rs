use crate::{
    ControlledExecutor, RustTool, ToolRequest, validate_execution_request, validate_tool_output,
};
use argus_core::{
    ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin, EvidenceProvenance, EvidenceRecord,
    ResolutionQuality, Target, TargetId,
};
use argus_language::SourceAccess;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RustToolEvidenceInventory {
    pub evidence: Vec<EvidenceRecord>,
    pub rejected: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResultStatus {
    Passed,
    Failed,
    Ignored,
}

impl ResultStatus {
    const fn name(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Ignored => "ignored",
        }
    }
}

#[derive(Debug, Deserialize)]
struct CapturedResult {
    #[serde(default)]
    target: Option<TargetId>,
    status: ResultStatus,
    summary: String,
    #[serde(default)]
    detail: Option<String>,
}

/// Normalizes Argus JSONL emitted by adapters around Rust engineering tools.
#[derive(Clone, Debug)]
pub struct RustToolEvidenceProvider {
    tool: RustTool,
    json_lines: Vec<u8>,
    configuration: ConfigurationId,
    provider: String,
    provider_version: String,
    ingest_only: bool,
}

impl RustToolEvidenceProvider {
    #[must_use]
    pub fn new(
        tool: RustTool,
        json_lines: Vec<u8>,
        configuration: ConfigurationId,
        provider: impl Into<String>,
        provider_version: impl Into<String>,
    ) -> Self {
        Self {
            tool,
            json_lines,
            configuration,
            provider: provider.into(),
            provider_version: provider_version.into(),
            ingest_only: true,
        }
    }

    pub fn execute(
        &self,
        executor: &dyn ControlledExecutor,
        request: &ToolRequest,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<RustToolEvidenceInventory, argus_core::ArgusError> {
        if request.tool != self.tool {
            return Err(argus_core::ArgusError::invalid_input(
                "tool request does not match evidence provider",
            ));
        }
        validate_execution_request(&executor.capabilities(), request)?;
        let output = executor.execute(request)?;
        validate_tool_output(request, &output)?;
        let success = output.success;
        let active = Self {
            tool: self.tool,
            json_lines: output.stdout,
            configuration: self.configuration.clone(),
            provider: self.provider.clone(),
            provider_version: self.provider_version.clone(),
            ingest_only: false,
        };
        let mut inventory = active.ingest(source, targets)?;
        if !success {
            inventory.evidence.push(active.failed_run(source));
            inventory
                .evidence
                .sort_by(|left, right| left.id.cmp(&right.id));
        }
        Ok(inventory)
    }

    pub fn ingest(
        &self,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<RustToolEvidenceInventory, argus_core::ArgusError> {
        let text = std::str::from_utf8(&self.json_lines).map_err(|error| {
            argus_core::ArgusError::invalid_input("Rust tool result stream is not UTF-8")
                .with_source(error)
        })?;
        let known = targets
            .iter()
            .map(|target| target.id.clone())
            .collect::<BTreeSet<_>>();
        let mut inventory = RustToolEvidenceInventory::default();
        for (offset, line) in text.lines().enumerate() {
            let result = match serde_json::from_str::<CapturedResult>(line) {
                Ok(result) => result,
                Err(error) => {
                    inventory
                        .rejected
                        .push(format!("line {}: {error}", offset + 1));
                    continue;
                }
            };
            if result.summary.trim().is_empty() {
                inventory
                    .rejected
                    .push(format!("line {}: result summary is empty", offset + 1));
                continue;
            }
            if result
                .target
                .as_ref()
                .is_some_and(|target| !known.contains(target))
            {
                inventory.rejected.push(format!(
                    "line {}: result references an unknown target",
                    offset + 1
                ));
                continue;
            }
            inventory.evidence.push(self.record(source, result)?);
        }
        inventory
            .evidence
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(inventory)
    }

    fn record(
        &self,
        source: &dyn SourceAccess,
        result: CapturedResult,
    ) -> Result<EvidenceRecord, argus_core::ArgusError> {
        let target = result.target.as_ref().map_or("", TargetId::as_str);
        let resolution = if target.is_empty() {
            ResolutionQuality::Unmapped
        } else {
            ResolutionQuality::Exact
        };
        let record = EvidenceRecord {
            id: EvidenceId::derive([
                b"rust-tool-result".as_slice(),
                source.snapshot_id().as_str().as_bytes(),
                self.configuration.as_str().as_bytes(),
                tool_name(self.tool).as_bytes(),
                self.provider.as_bytes(),
                self.provider_version.as_bytes(),
                target.as_bytes(),
                result.status.name().as_bytes(),
                result.summary.as_bytes(),
                result.detail.as_deref().unwrap_or_default().as_bytes(),
            ]),
            kind: evidence_kind(self.tool),
            origin: EvidenceOrigin::Direct,
            target: result.target,
            location: None,
            summary: format!("{}: {}", result.status.name(), result.summary),
            detail: result.detail,
            provenance: self.provenance(resolution),
        };
        record.validate()?;
        Ok(record)
    }

    fn failed_run(&self, source: &dyn SourceAccess) -> EvidenceRecord {
        EvidenceRecord {
            id: EvidenceId::derive([
                b"rust-tool-failed-run".as_slice(),
                source.snapshot_id().as_str().as_bytes(),
                self.configuration.as_str().as_bytes(),
                tool_name(self.tool).as_bytes(),
                self.provider.as_bytes(),
                self.provider_version.as_bytes(),
            ]),
            kind: evidence_kind(self.tool),
            origin: EvidenceOrigin::Direct,
            target: None,
            location: None,
            summary: format!("{} reported an unsuccessful run", self.provider),
            detail: None,
            provenance: self.provenance(ResolutionQuality::Unmapped),
        }
    }

    fn provenance(&self, resolution: ResolutionQuality) -> EvidenceProvenance {
        EvidenceProvenance {
            provider: self.provider.clone(),
            provider_version: self.provider_version.clone(),
            configuration: self.configuration.clone(),
            ingest_only: self.ingest_only,
            resolution,
        }
    }
}

const fn evidence_kind(tool: RustTool) -> EvidenceKind {
    match tool {
        RustTool::CargoCheck | RustTool::Clippy => EvidenceKind::StaticAnalysis,
        RustTool::Tests | RustTool::Doctests => EvidenceKind::Test,
        RustTool::Rustdoc => EvidenceKind::Documentation,
    }
}

const fn tool_name(tool: RustTool) -> &'static str {
    match tool {
        RustTool::CargoCheck => "cargo_check",
        RustTool::Clippy => "clippy",
        RustTool::Tests => "tests",
        RustTool::Doctests => "doctests",
        RustTool::Rustdoc => "rustdoc",
    }
}
