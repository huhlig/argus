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
    ByteSpan, ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin, EvidenceProvenance,
    EvidenceRecord, ResolutionQuality, SourceLocation, Target,
};
use argus_language::SourceAccess;
use serde::Deserialize;
use std::collections::BTreeMap;

const DEFAULT_PROVIDER: &str = "ruff-diagnostics";
const DEFAULT_VERSION: &str = "1";

/// Inventory of diagnostics parsed from Python tooling output (e.g. `ruff`, `mypy`, `flake8`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PythonDiagnosticInventory {
    pub evidence: Vec<EvidenceRecord>,
    pub unparsed_lines: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RuffJsonDiagnostic {
    code: Option<String>,
    message: String,
    filename: String,
    location: Option<RuffLocation>,
    end_location: Option<RuffLocation>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct RuffLocation {
    row: usize,
    column: usize,
}

/// Ingests compiler/linter diagnostic output for Python targets.
#[derive(Clone, Debug)]
pub struct PythonDiagnosticProvider {
    configuration: ConfigurationId,
    provider: String,
    provider_version: String,
}

impl PythonDiagnosticProvider {
    #[must_use]
    pub fn new(
        configuration: ConfigurationId,
        provider: Option<String>,
        provider_version: Option<String>,
    ) -> Self {
        Self {
            configuration,
            provider: provider.unwrap_or_else(|| DEFAULT_PROVIDER.to_owned()),
            provider_version: provider_version.unwrap_or_else(|| DEFAULT_VERSION.to_owned()),
        }
    }

    /// Ingests JSON output from `ruff check --output-format json`.
    #[allow(clippy::missing_panics_doc)]
    pub fn ingest_ruff_json(
        &self,
        json_str: &str,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<PythonDiagnosticInventory, argus_core::ArgusError> {
        let mut evidence = Vec::new();
        let mut unparsed_lines = Vec::new();

        let Ok(diagnostics) = serde_json::from_str::<Vec<RuffJsonDiagnostic>>(json_str) else {
            unparsed_lines.push(json_str.to_owned());
            return Ok(PythonDiagnosticInventory {
                evidence,
                unparsed_lines,
            });
        };

        let mut sources = BTreeMap::new();

        for diag in diagnostics {
            let norm_path = diag.filename.replace('\\', "/");
            let target = targets.iter().find(|t| {
                t.location
                    .as_ref()
                    .is_some_and(|loc| norm_path.ends_with(loc.path.as_str()))
            });

            if let Some(target) = target {
                let location = target.location.as_ref().expect("target has location");
                if !sources.contains_key(&location.path) {
                    sources.insert(location.path.clone(), source.read(&location.path)?);
                }
                let bytes = sources.get(&location.path).expect("source loaded");

                let span = if let (Some(loc), Some(end_loc)) = (diag.location, diag.end_location) {
                    compute_byte_span(bytes, loc.row, loc.column, end_loc.row, end_loc.column)
                        .unwrap_or(location.bytes)
                } else {
                    location.bytes
                };

                let message_text = format!(
                    "{}: {}",
                    diag.code.as_deref().unwrap_or("diagnostic"),
                    diag.message
                );

                let evidence_id = EvidenceId::derive([
                    self.configuration.as_str().as_bytes(),
                    target.id.as_str().as_bytes(),
                    b"diagnostic",
                    diag.code.as_deref().unwrap_or("").as_bytes(),
                    span.start.to_le_bytes().as_slice(),
                ]);

                evidence.push(EvidenceRecord {
                    id: evidence_id,
                    target: Some(target.id.clone()),
                    kind: EvidenceKind::CompilerDiagnostic,
                    location: Some(SourceLocation {
                        path: location.path.clone(),
                        bytes: span,
                        start: None,
                        end: None,
                    }),
                    summary: message_text.clone(),
                    detail: Some(message_text),
                    origin: EvidenceOrigin::Direct,
                    provenance: EvidenceProvenance {
                        provider: self.provider.clone(),
                        provider_version: self.provider_version.clone(),
                        configuration: self.configuration.clone(),
                        ingest_only: true,
                        resolution: ResolutionQuality::Exact,
                    },
                });
            } else {
                unparsed_lines.push(format!("{}: {}", diag.filename, diag.message));
            }
        }

        Ok(PythonDiagnosticInventory {
            evidence,
            unparsed_lines,
        })
    }

    /// Ingests textual output from `flake8` or `mypy` formatted like:
    /// `path/to/file.py:line:col: message`
    #[allow(clippy::missing_panics_doc)]
    pub fn ingest_text(
        &self,
        output: &str,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<PythonDiagnosticInventory, argus_core::ArgusError> {
        let mut evidence = Vec::new();
        let mut unparsed_lines = Vec::new();
        let mut sources = BTreeMap::new();

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(parsed) = parse_line_diagnostic(trimmed) {
                let target = targets.iter().find(|t| {
                    t.location
                        .as_ref()
                        .is_some_and(|loc| parsed.path.ends_with(loc.path.as_str()))
                });

                if let Some(target) = target {
                    let location = target.location.as_ref().expect("target has location");
                    if !sources.contains_key(&location.path) {
                        sources.insert(location.path.clone(), source.read(&location.path)?);
                    }
                    let bytes = sources.get(&location.path).expect("source loaded");

                    let span = compute_byte_span(
                        bytes,
                        parsed.line,
                        parsed.column,
                        parsed.line,
                        parsed.column + 1,
                    )
                    .unwrap_or(location.bytes);

                    let evidence_id = EvidenceId::derive([
                        self.configuration.as_str().as_bytes(),
                        target.id.as_str().as_bytes(),
                        b"diagnostic",
                        parsed.message.as_bytes(),
                        span.start.to_le_bytes().as_slice(),
                    ]);

                    evidence.push(EvidenceRecord {
                        id: evidence_id,
                        target: Some(target.id.clone()),
                        kind: EvidenceKind::CompilerDiagnostic,
                        location: Some(SourceLocation {
                            path: location.path.clone(),
                            bytes: span,
                            start: None,
                            end: None,
                        }),
                        summary: format!("Python tool diagnostic for {}", target.name),
                        detail: Some(parsed.message),
                        origin: EvidenceOrigin::Direct,
                        provenance: EvidenceProvenance {
                            provider: self.provider.clone(),
                            provider_version: self.provider_version.clone(),
                            configuration: self.configuration.clone(),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                        },
                    });
                    continue;
                }
            }

            unparsed_lines.push(trimmed.to_owned());
        }

        Ok(PythonDiagnosticInventory {
            evidence,
            unparsed_lines,
        })
    }
}

struct ParsedLineDiagnostic {
    path: String,
    line: usize,
    column: usize,
    message: String,
}

fn parse_line_diagnostic(line: &str) -> Option<ParsedLineDiagnostic> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?.trim().replace('\\', "/");
    let line_num: usize = parts.next()?.trim().parse().ok()?;
    let col_num: usize = parts.next()?.trim().parse().ok()?;
    let message = parts.next()?.trim().to_owned();

    Some(ParsedLineDiagnostic {
        path,
        line: line_num,
        column: col_num,
        message,
    })
}

fn compute_byte_span(
    bytes: &[u8],
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
) -> Option<ByteSpan> {
    if start_line == 0 || start_col == 0 {
        return None;
    }

    let mut current_line = 1;
    let mut current_col = 1;
    let mut start_offset = None;
    let mut end_offset = None;

    for (offset, &b) in bytes.iter().enumerate() {
        if current_line == start_line && current_col == start_col && start_offset.is_none() {
            start_offset = Some(offset);
        }
        if current_line == end_line && current_col == end_col && end_offset.is_none() {
            end_offset = Some(offset);
            break;
        }

        if b == b'\n' {
            current_line += 1;
            current_col = 1;
        } else {
            current_col += 1;
        }
    }

    let start = u64::try_from(start_offset?).ok()?;
    let end = u64::try_from(end_offset.unwrap_or(bytes.len())).ok()?;
    ByteSpan::new(start, end).ok()
}
