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

use crate::{ControlledExecutor, ToolRequest, validate_execution_request, validate_tool_output};
use argus_core::{
    ByteSpan, ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin, EvidenceProvenance,
    EvidenceRecord, PortableTargetKind, ResolutionQuality, SourceLocation, SourcePath, Target,
    TargetKind,
};
use argus_language::SourceAccess;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustDiagnosticInventory {
    pub evidence: Vec<EvidenceRecord>,
    pub malformed_lines: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CompilerDiagnosticProvider {
    json_lines: Vec<u8>,
    workspace_root: PathBuf,
    configuration: ConfigurationId,
    provider: String,
    provider_version: String,
    ingest_only: bool,
}

impl CompilerDiagnosticProvider {
    #[must_use]
    pub fn new(
        json_lines: Vec<u8>,
        workspace_root: PathBuf,
        configuration: ConfigurationId,
        provider: impl Into<String>,
        provider_version: impl Into<String>,
    ) -> Self {
        Self {
            json_lines,
            workspace_root,
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
    ) -> Result<RustDiagnosticInventory, argus_core::ArgusError> {
        if request.workspace_root != self.workspace_root {
            return Err(argus_core::ArgusError::invalid_input(
                "tool request workspace does not match diagnostic provider",
            ));
        }
        let capabilities = executor.capabilities();
        validate_execution_request(&capabilities, request)?;
        let output = executor.execute(request)?;
        validate_tool_output(request, &output)?;
        let mut stream = output.stdout;
        if !output.success {
            if !stream.ends_with(b"\n") {
                stream.push(b'\n');
            }
            stream.extend_from_slice(b"{\"reason\":\"build-finished\",\"success\":false}\n");
        }
        let active = Self {
            json_lines: stream,
            workspace_root: self.workspace_root.clone(),
            configuration: self.configuration.clone(),
            provider: self.provider.clone(),
            provider_version: self.provider_version.clone(),
            ingest_only: false,
        };
        active.ingest(source, targets)
    }

    pub fn ingest(
        &self,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<RustDiagnosticInventory, argus_core::ArgusError> {
        let text = std::str::from_utf8(&self.json_lines).map_err(|error| {
            argus_core::ArgusError::invalid_input("compiler diagnostic stream is not UTF-8")
                .with_source(error)
        })?;
        let mut evidence = Vec::new();
        let mut malformed_lines = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let envelope = match serde_json::from_str::<CargoEnvelope>(line) {
                Ok(envelope) => envelope,
                Err(error) => {
                    malformed_lines.push(format!("line {}: {error}", index + 1));
                    continue;
                }
            };
            match envelope.reason.as_str() {
                "compiler-message" => {
                    if let Some(message) = envelope.message {
                        evidence.push(self.compiler_message(source, targets, message)?);
                    }
                }
                "build-finished" if envelope.success == Some(false) => {
                    evidence.push(self.build_failure(source));
                }
                _ => {}
            }
        }
        evidence.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(RustDiagnosticInventory {
            evidence,
            malformed_lines,
        })
    }

    fn compiler_message(
        &self,
        source: &dyn SourceAccess,
        targets: &[Target],
        message: CompilerMessage,
    ) -> Result<EvidenceRecord, argus_core::ArgusError> {
        let primary = message.spans.iter().find(|span| span.is_primary);
        let location = primary
            .and_then(|span| self.source_path(&span.file_name).map(|path| (path, span)))
            .filter(|(path, _)| source.contains(path))
            .map(|(path, span)| {
                Ok(SourceLocation {
                    path,
                    bytes: ByteSpan::new(span.byte_start, span.byte_end)?,
                    start: None,
                    end: None,
                })
            })
            .transpose()?;
        let (target, resolution) = location
            .as_ref()
            .map_or((None, ResolutionQuality::Unmapped), |location| {
                map_target(targets, location)
            });
        let code = message.code.as_ref().map_or("", |code| code.code.as_str());
        let summary = if code.is_empty() {
            format!("{}: {}", message.level, message.message)
        } else {
            format!("{}[{code}]: {}", message.level, message.message)
        };
        let path = location
            .as_ref()
            .map_or("", |location| location.path.as_str());
        let start = location.as_ref().map_or(0, |location| location.bytes.start);
        let end = location.as_ref().map_or(0, |location| location.bytes.end);
        let id = EvidenceId::derive([
            b"rust-compiler-diagnostic".as_slice(),
            source.snapshot_id().as_str().as_bytes(),
            self.configuration.as_str().as_bytes(),
            self.provider.as_bytes(),
            self.provider_version.as_bytes(),
            summary.as_bytes(),
            path.as_bytes(),
            &start.to_be_bytes(),
            &end.to_be_bytes(),
        ]);
        let record = EvidenceRecord {
            id,
            kind: EvidenceKind::CompilerDiagnostic,
            origin: EvidenceOrigin::Direct,
            target,
            location,
            summary,
            detail: message.rendered,
            provenance: self.provenance(resolution),
        };
        record.validate()?;
        Ok(record)
    }

    fn build_failure(&self, source: &dyn SourceAccess) -> EvidenceRecord {
        EvidenceRecord {
            id: EvidenceId::derive([
                b"rust-build-failure".as_slice(),
                source.snapshot_id().as_str().as_bytes(),
                self.configuration.as_str().as_bytes(),
                self.provider.as_bytes(),
                self.provider_version.as_bytes(),
            ]),
            kind: EvidenceKind::CompilerDiagnostic,
            origin: EvidenceOrigin::Direct,
            target: None,
            location: None,
            summary: format!("{} reported an unsuccessful build", self.provider),
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

    fn source_path(&self, value: &str) -> Option<SourcePath> {
        let path = Path::new(value);
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.workspace_root).ok()?
        } else {
            path
        };
        SourcePath::new(relative.to_string_lossy().replace('\\', "/")).ok()
    }
}

fn map_target(
    targets: &[Target],
    evidence: &SourceLocation,
) -> (Option<argus_core::TargetId>, ResolutionQuality) {
    let mut containing = targets
        .iter()
        .filter(|target| {
            target.location.as_ref().is_some_and(|location| {
                location.path == evidence.path
                    && location.bytes.start <= evidence.bytes.start
                    && location.bytes.end >= evidence.bytes.end
            })
        })
        .collect::<Vec<_>>();
    containing.sort_by_key(|target| {
        target.location.as_ref().map_or(u64::MAX, |location| {
            location.bytes.end.saturating_sub(location.bytes.start)
        })
    });
    if let Some(target) = containing.iter().find(|target| {
        !matches!(
            target.kind,
            TargetKind::Portable {
                kind: PortableTargetKind::File
            }
        )
    }) {
        let exact = target
            .location
            .as_ref()
            .is_some_and(|location| location.bytes == evidence.bytes);
        return (
            Some(target.id.clone()),
            if exact {
                ResolutionQuality::Exact
            } else {
                ResolutionQuality::ContainingTarget
            },
        );
    }
    containing
        .first()
        .map_or((None, ResolutionQuality::Unmapped), |target| {
            (Some(target.id.clone()), ResolutionQuality::FileFallback)
        })
}

#[derive(Deserialize)]
struct CargoEnvelope {
    reason: String,
    #[serde(default)]
    message: Option<CompilerMessage>,
    #[serde(default)]
    success: Option<bool>,
}

#[derive(Deserialize)]
struct CompilerMessage {
    message: String,
    level: String,
    #[serde(default)]
    code: Option<DiagnosticCode>,
    #[serde(default)]
    spans: Vec<DiagnosticSpan>,
    #[serde(default)]
    rendered: Option<String>,
}

#[derive(Deserialize)]
struct DiagnosticCode {
    code: String,
}

#[derive(Deserialize)]
struct DiagnosticSpan {
    file_name: String,
    byte_start: u64,
    byte_end: u64,
    is_primary: bool,
}
