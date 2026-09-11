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
    EvidenceRecord, ResolutionQuality, SourceLocation, SourcePath, Target,
};
use argus_language::SourceAccess;
use std::collections::BTreeMap;

const DEFAULT_PROVIDER: &str = "tsc";
const DEFAULT_VERSION: &str = "1";

/// Inventory of diagnostics parsed from TypeScript tooling output (e.g. `tsc`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypeScriptDiagnosticInventory {
    pub evidence: Vec<EvidenceRecord>,
    pub unparsed_lines: Vec<String>,
}

/// Ingests compiler/linter diagnostic output for TypeScript targets.
#[derive(Clone, Debug)]
pub struct TypeScriptDiagnosticProvider {
    configuration: ConfigurationId,
    provider: String,
    provider_version: String,
}

impl TypeScriptDiagnosticProvider {
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

    /// Ingests textual `tsc` output lines formatted like:
    /// `path/to/file.ts(line,col): error TS1234: Message text`
    pub fn ingest_tsc(
        &self,
        output: &str,
        source: &dyn SourceAccess,
        targets: &[Target],
    ) -> Result<TypeScriptDiagnosticInventory, argus_core::ArgusError> {
        let mut evidence = Vec::new();
        let mut unparsed_lines = Vec::new();

        // Index file targets by path
        let mut file_targets = BTreeMap::new();
        for target in targets {
            if let Some(loc) = &target.location {
                file_targets.insert(loc.path.as_str().replace('\\', "/"), target);
            }
        }

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(parsed) = parse_tsc_line(trimmed) {
                let normalized_path = parsed.path.replace('\\', "/");
                let matched_target = file_targets.get(&normalized_path).copied();

                let source_path = SourcePath::new(&normalized_path).ok();
                let location = if let Some(sp) = &source_path {
                    if source.contains(sp) {
                        Some(SourceLocation {
                            path: sp.clone(),
                            bytes: ByteSpan::new(0, 0)?,
                            start: None,
                            end: None,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                let id = EvidenceId::derive([
                    b"typescript-compiler-diagnostic".as_slice(),
                    normalized_path.as_bytes(),
                    parsed.line.to_string().as_bytes(),
                    parsed.column.to_string().as_bytes(),
                    parsed.code.as_bytes(),
                    parsed.message.as_bytes(),
                ]);

                evidence.push(EvidenceRecord {
                    id,
                    kind: EvidenceKind::CompilerDiagnostic,
                    origin: EvidenceOrigin::Direct,
                    target: matched_target.map(|t| t.id.clone()),
                    location,
                    summary: format!("{} {}: {}", parsed.severity, parsed.code, parsed.message),
                    detail: Some(trimmed.to_owned()),
                    provenance: EvidenceProvenance {
                        provider: self.provider.clone(),
                        provider_version: self.provider_version.clone(),
                        configuration: self.configuration.clone(),
                        ingest_only: true,
                        resolution: ResolutionQuality::Exact,
                    },
                });
            } else {
                unparsed_lines.push(trimmed.to_owned());
            }
        }

        Ok(TypeScriptDiagnosticInventory {
            evidence,
            unparsed_lines,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedTscDiagnostic<'a> {
    path: &'a str,
    line: u32,
    column: u32,
    severity: &'a str,
    code: &'a str,
    message: &'a str,
}

fn parse_tsc_line(line: &str) -> Option<ParsedTscDiagnostic<'_>> {
    // Format: path/file.ts(12,34): error TS1234: Something went wrong
    let paren_open = line.find('(')?;
    let paren_close = line[paren_open..].find(')')? + paren_open;
    let colon_after_paren = line[paren_close..].find(':')? + paren_close;

    let path = &line[..paren_open];
    let pos_str = &line[paren_open + 1..paren_close];
    let (line_str, col_str) = pos_str.split_once(',')?;
    let line_num: u32 = line_str.trim().parse().ok()?;
    let col_num: u32 = col_str.trim().parse().ok()?;

    let remainder = line[colon_after_paren + 1..].trim();
    // remainder is: "error TS1234: Something went wrong"
    let (sev_code, msg) = remainder.split_once(':')?;
    let (severity, code) = sev_code.trim().split_once(' ')?;

    Some(ParsedTscDiagnostic {
        path: path.trim(),
        line: line_num,
        column: col_num,
        severity: severity.trim(),
        code: code.trim(),
        message: msg.trim(),
    })
}
