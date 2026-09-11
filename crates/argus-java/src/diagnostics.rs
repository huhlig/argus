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

const DEFAULT_PROVIDER: &str = "javac-diagnostics";
const DEFAULT_VERSION: &str = "1";

/// Inventory of diagnostics parsed from Java tooling output (e.g. `javac`, Checkstyle, `SpotBugs`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JavaDiagnosticInventory {
    pub evidence: Vec<EvidenceRecord>,
    pub unparsed_lines: Vec<String>,
}

/// Ingests compiler/linter diagnostic output for Java targets.
#[derive(Clone, Debug)]
pub struct JavaDiagnosticProvider {
    configuration: ConfigurationId,
    provider: String,
    provider_version: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename = "checkstyle")]
struct CheckstyleReport {
    #[serde(rename = "file", default)]
    files: Vec<CheckstyleFile>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CheckstyleFile {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "error", default)]
    errors: Vec<CheckstyleError>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CheckstyleError {
    #[serde(rename = "@line")]
    line: Option<usize>,
    #[serde(rename = "@column")]
    column: Option<usize>,
    #[serde(rename = "@severity")]
    severity: Option<String>,
    #[serde(rename = "@message")]
    message: Option<String>,
    #[serde(rename = "@source")]
    source: Option<String>,
}

impl JavaDiagnosticProvider {
    #[must_use]
    pub fn new(
        configuration: ConfigurationId,
        provider: Option<String>,
        provider_version: Option<String>,
    ) -> Self {
        Self {
            configuration,
            provider: provider.unwrap_or_else(|| DEFAULT_PROVIDER.to_string()),
            provider_version: provider_version.unwrap_or_else(|| DEFAULT_VERSION.to_string()),
        }
    }

    /// Ingests standard line-oriented `javac` compiler output.
    /// Format: `path/to/File.java:12: error: cannot find symbol`
    /// or: `path/to/File.java:15:20: warning: [deprecation] ...`
    pub fn ingest_javac_lines(
        &self,
        output: &str,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<JavaDiagnosticInventory, argus_core::ArgusError> {
        let mut evidence = Vec::new();
        let mut unparsed_lines = Vec::new();

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(diag) = parse_javac_line(trimmed) {
                if let Some(target) = find_target_for_file(&diag.filename, targets) {
                    let Some(location) = &target.location else {
                        continue;
                    };
                    let file_bytes = source.read(&location.path).unwrap_or_default();
                    let span = compute_line_byte_span(&file_bytes, diag.line, diag.column)
                        .unwrap_or(location.bytes);

                    let evidence_id = EvidenceId::derive([
                        b"javac-diagnostic".as_slice(),
                        target.id.as_str().as_bytes(),
                        diag.filename.as_bytes(),
                        diag.line.to_string().as_bytes(),
                        span.start.to_le_bytes().as_slice(),
                    ]);

                    evidence.push(EvidenceRecord {
                        id: evidence_id,
                        kind: EvidenceKind::CompilerDiagnostic,
                        origin: EvidenceOrigin::Direct,
                        target: Some(target.id.clone()),
                        location: Some(SourceLocation {
                            path: location.path.clone(),
                            bytes: span,
                            start: None,
                            end: None,
                        }),
                        summary: format!("{}: {}", diag.severity, diag.message),
                        detail: Some(diag.message),
                        provenance: EvidenceProvenance {
                            provider: self.provider.clone(),
                            provider_version: self.provider_version.clone(),
                            configuration: self.configuration.clone(),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                        },
                    });
                } else {
                    unparsed_lines.push(line.to_string());
                }
            } else {
                unparsed_lines.push(line.to_string());
            }
        }

        Ok(JavaDiagnosticInventory {
            evidence,
            unparsed_lines,
        })
    }

    /// Ingests Checkstyle XML output.
    pub fn ingest_checkstyle_xml(
        &self,
        xml_str: &str,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<JavaDiagnosticInventory, argus_core::ArgusError> {
        let mut evidence = Vec::new();
        let mut unparsed_lines = Vec::new();

        let report: CheckstyleReport = if let Ok(r) = quick_xml::de::from_str(xml_str) { r } else {
            unparsed_lines.push(xml_str.to_string());
            return Ok(JavaDiagnosticInventory {
                evidence,
                unparsed_lines,
            });
        };

        for file in report.files {
            let target = find_target_for_file(&file.name, targets);
            if let Some(target) = target {
                let Some(location) = &target.location else {
                    continue;
                };
                let file_bytes = source.read(&location.path).unwrap_or_default();

                for err in file.errors {
                    let line_num = err.line.unwrap_or(1);
                    let col_num = err.column;
                    let span = compute_line_byte_span(&file_bytes, line_num, col_num)
                        .unwrap_or(location.bytes);

                    let msg = err.message.unwrap_or_else(|| "checkstyle warning".to_string());
                    let sev = err.severity.unwrap_or_else(|| "warning".to_string());

                    let evidence_id = EvidenceId::derive([
                        b"checkstyle-diagnostic".as_slice(),
                        target.id.as_str().as_bytes(),
                        file.name.as_bytes(),
                        line_num.to_string().as_bytes(),
                        span.start.to_le_bytes().as_slice(),
                    ]);

                    evidence.push(EvidenceRecord {
                        id: evidence_id,
                        kind: EvidenceKind::CompilerDiagnostic,
                        origin: EvidenceOrigin::Direct,
                        target: Some(target.id.clone()),
                        location: Some(SourceLocation {
                            path: location.path.clone(),
                            bytes: span,
                            start: None,
                            end: None,
                        }),
                        summary: format!("{sev}: {msg}"),
                        detail: err.source.clone().or(Some(msg)),
                        provenance: EvidenceProvenance {
                            provider: "checkstyle".to_string(),
                            provider_version: "1".to_string(),
                            configuration: self.configuration.clone(),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                        },
                    });
                }
            }
        }

        Ok(JavaDiagnosticInventory {
            evidence,
            unparsed_lines,
        })
    }
}

struct ParsedJavacDiag {
    filename: String,
    line: usize,
    column: Option<usize>,
    severity: String,
    message: String,
}

fn parse_javac_line(line: &str) -> Option<ParsedJavacDiag> {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() < 3 {
        return None;
    }

    let filename = parts[0].trim();
    if !std::path::Path::new(filename)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("java"))
    {
        return None;
    }

    let line_num: usize = parts[1].trim().parse().ok()?;

    if parts.len() == 4 {
        if let Ok(col) = parts[2].trim().parse::<usize>() {
            let rest = parts[3].trim();
            let (sev, msg) = split_severity_message(rest);
            return Some(ParsedJavacDiag {
                filename: filename.to_string(),
                line: line_num,
                column: Some(col),
                severity: sev,
                message: msg,
            });
        }
    }

    let rest = line.splitn(3, ':').nth(2)?.trim();
    let (sev, msg) = split_severity_message(rest);
    Some(ParsedJavacDiag {
        filename: filename.to_string(),
        line: line_num,
        column: None,
        severity: sev,
        message: msg,
    })
}

fn split_severity_message(rest: &str) -> (String, String) {
    if let Some(msg) = rest.strip_prefix("error:") {
        ("error".to_string(), msg.trim().to_string())
    } else if let Some(msg) = rest.strip_prefix("warning:") {
        ("warning".to_string(), msg.trim().to_string())
    } else if let Some(msg) = rest.strip_prefix("note:") {
        ("note".to_string(), msg.trim().to_string())
    } else {
        ("error".to_string(), rest.trim().to_string())
    }
}

fn find_target_for_file<'a>(file_path: &str, targets: &'a [Target]) -> Option<&'a Target> {
    let norm = file_path.replace('\\', "/");
    targets.iter().find(|t| {
        t.location.as_ref().is_some_and(|loc| {
            let loc_path = loc.path.as_str().replace('\\', "/");
            norm.ends_with(&loc_path) || loc_path.ends_with(&norm)
        })
    })
}

fn compute_line_byte_span(bytes: &[u8], line_1_indexed: usize, col_1_indexed: Option<usize>) -> Option<ByteSpan> {
    let mut current_line = 1;
    let mut line_start = 0;

    for (idx, &b) in bytes.iter().enumerate() {
        if current_line == line_1_indexed {
            let mut line_end = idx;
            while line_end < bytes.len() && bytes[line_end] != b'\n' {
                line_end += 1;
            }

            let start = if let Some(col) = col_1_indexed {
                (line_start + col.saturating_sub(1)).min(line_end)
            } else {
                line_start
            };

            return ByteSpan::new(start as u64, line_end as u64).ok();
        }

        if b == b'\n' {
            current_line += 1;
            line_start = idx + 1;
        }
    }

    None
}
