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
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, EvidenceId, EvidenceKind,
    EvidenceOrigin, EvidenceProvenance, EvidenceRecord, InventoryState, PortableTargetKind,
    ResolutionQuality, SourceLocation, SourcePath, Target, TargetId, TargetKind, TargetVisibility,
};
use argus_language::SourceAccess;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use std::collections::BTreeMap;

const PROVIDER: &str = "ruff-syntax";
const PROVIDER_VERSION: &str = "0.0.13";

/// Discovered import statement in a Python source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredImport {
    pub imported_name: String,
    pub module_path: Option<String>,
    pub is_relative: bool,
    pub relative_level: u32,
    pub span: ByteSpan,
}

/// Discovered class inheritance relationship.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredInheritance {
    pub sub_target_id: TargetId,
    pub base_name: String,
    pub span: ByteSpan,
}

/// Discovered function or method call site.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredCallSite {
    pub caller_target_id: TargetId,
    pub callee_name: String,
    pub span: ByteSpan,
}

/// Parsed Python syntax inventory for a single source file.
#[derive(Clone, Debug, Default)]
pub struct PythonSyntaxInventory {
    pub targets: Vec<Target>,
    pub diagnostics: Vec<String>,
    pub documentation: BTreeMap<TargetId, String>,
    pub imports: Vec<DiscoveredImport>,
    pub inheritances: Vec<DiscoveredInheritance>,
    pub calls: Vec<DiscoveredCallSite>,
}

/// Provider that extracts AST targets, documentation, and relationships from Python files.
#[derive(Clone, Debug)]
pub struct PythonSyntaxProvider {
    configuration: ConfigurationId,
}

impl PythonSyntaxProvider {
    #[must_use]
    pub const fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    /// Parses a Python source file and extracts all targets, docstrings, imports, and inheritances.
    #[allow(clippy::too_many_lines)]
    pub fn parse_file(
        &self,
        path: &SourcePath,
        text: &str,
        parent: Option<TargetId>,
    ) -> Result<PythonSyntaxInventory, argus_core::ArgusError> {
        let mut diagnostics = Vec::new();
        let parsed = match ruff_python_parser::parse(text, ruff_python_parser::Mode::Module.into()) {
            Ok(p) => p,
            Err(err) => {
                diagnostics.push(format!("parse error in {}: {err}", path.as_str()));
                return Ok(PythonSyntaxInventory {
                    targets: Vec::new(),
                    diagnostics,
                    documentation: BTreeMap::new(),
                    imports: Vec::new(),
                    inheritances: Vec::new(),
                    calls: Vec::new(),
                });
            }
        };

        let file_span = ByteSpan::new(0, u64::try_from(text.len()).unwrap_or(0))?;
        let file_id = TargetId::derive([
            b"python".as_slice(),
            b"file".as_slice(),
            path.as_str().as_bytes(),
        ]);

        let mut targets = vec![Target {
            id: file_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::File,
            },
            visibility: TargetVisibility::NotApplicable,
            name: path.as_str().to_owned(),
            parent,
            location: Some(SourceLocation {
                path: path.clone(),
                bytes: file_span,
                start: None,
                end: None,
            }),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(!diagnostics.is_empty())],
            diagnostic: None,
        }];

        let mut documentation = BTreeMap::new();
        let mut imports = Vec::new();
        let mut inheritances = Vec::new();
        let mut calls = Vec::new();

        let ast_mod = parsed.into_syntax();
        #[allow(clippy::match_wildcard_for_single_variants)]
        let body = match ast_mod {
            ast::Mod::Module(m) => m.body,
            _ => Vec::new().into(),
        };

        // Module-level docstring
        if let Some(doc) = extract_docstring(&body) {
            documentation.insert(file_id.clone(), doc);
        }

        let mut collector = TargetCollector {
            path,
            targets: &mut targets,
            documentation: &mut documentation,
            imports: &mut imports,
            inheritances: &mut inheritances,
            calls: &mut calls,
        };

        collector.collect_statements(&body, &file_id, None)?;

        Ok(PythonSyntaxInventory {
            targets,
            diagnostics,
            documentation,
            imports,
            inheritances,
            calls,
        })
    }

    /// Generates evidence records (Source and Documentation) for all targets in the syntax inventory.
    #[allow(clippy::missing_panics_doc)]
    pub fn review_evidence(
        &self,
        source: &dyn SourceAccess,
        syntax: &PythonSyntaxInventory,
    ) -> Result<Vec<EvidenceRecord>, argus_core::ArgusError> {
        let mut records = Vec::with_capacity(syntax.targets.len().saturating_mul(2));
        let mut sources = BTreeMap::new();

        for target in &syntax.targets {
            let documentation = syntax.documentation.get(&target.id);
            let presence = if documentation.is_some() {
                b"present".as_slice()
            } else {
                b"absent".as_slice()
            };

            // Documentation evidence
            records.push(EvidenceRecord {
                id: EvidenceId::derive([
                    b"python-syntax-documentation".as_slice(),
                    target.id.as_str().as_bytes(),
                    presence,
                    documentation.map_or(b"".as_slice(), |t| t.as_bytes()),
                ]),
                kind: EvidenceKind::Documentation,
                origin: EvidenceOrigin::Direct,
                target: Some(target.id.clone()),
                location: target.location.clone(),
                summary: if documentation.is_some() {
                    format!("Python documentation for {}", target.name)
                } else {
                    format!("No Python documentation is attached to {}", target.name)
                },
                detail: documentation.cloned(),
                provenance: EvidenceProvenance {
                    provider: PROVIDER.to_owned(),
                    provider_version: PROVIDER_VERSION.to_owned(),
                    configuration: self.configuration.clone(),
                    ingest_only: true,
                    resolution: ResolutionQuality::Exact,
                },
            });

            // Source evidence
            let Some(location) = &target.location else {
                continue;
            };

            if !sources.contains_key(&location.path) {
                sources.insert(location.path.clone(), source.read(&location.path)?);
            }
            let bytes = sources
                .get(&location.path)
                .expect("source was inserted for location");

            let start = usize::try_from(location.bytes.start).map_err(|e| {
                argus_core::ArgusError::invariant("source span start exceeds platform limits")
                    .with_source(e)
            })?;
            let end = usize::try_from(location.bytes.end).map_err(|e| {
                argus_core::ArgusError::invariant("source span end exceeds platform limits")
                    .with_source(e)
            })?;

            let span = bytes.get(start..end).ok_or_else(|| {
                argus_core::ArgusError::invariant("syntax target source span is out of bounds")
            })?;

            let detail = std::str::from_utf8(span).map_err(|e| {
                argus_core::ArgusError::invariant("syntax target source span is not UTF-8")
                    .with_source(e)
            })?;

            records.push(EvidenceRecord {
                id: EvidenceId::derive([
                    b"python-syntax-source".as_slice(),
                    target.id.as_str().as_bytes(),
                    span,
                ]),
                kind: EvidenceKind::Source,
                origin: EvidenceOrigin::Direct,
                target: Some(target.id.clone()),
                location: Some(location.clone()),
                summary: format!("Python source for {}", target.name),
                detail: Some(detail.to_owned()),
                provenance: EvidenceProvenance {
                    provider: PROVIDER.to_owned(),
                    provider_version: PROVIDER_VERSION.to_owned(),
                    configuration: self.configuration.clone(),
                    ingest_only: true,
                    resolution: ResolutionQuality::Exact,
                },
            });
        }

        Ok(records)
    }
}

struct TargetCollector<'a> {
    path: &'a SourcePath,
    targets: &'a mut Vec<Target>,
    documentation: &'a mut BTreeMap<TargetId, String>,
    imports: &'a mut Vec<DiscoveredImport>,
    inheritances: &'a mut Vec<DiscoveredInheritance>,
    calls: &'a mut Vec<DiscoveredCallSite>,
}

impl TargetCollector<'_> {
    fn collect_statements(
        &mut self,
        statements: &[Stmt],
        current_scope_id: &TargetId,
        current_class: Option<&str>,
    ) -> Result<(), argus_core::ArgusError> {
        for stmt in statements {
            self.collect_statement(stmt, current_scope_id, current_class)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn collect_statement(
        &mut self,
        stmt: &Stmt,
        current_scope_id: &TargetId,
        current_class: Option<&str>,
    ) -> Result<(), argus_core::ArgusError> {
        match stmt {
            Stmt::ClassDef(class_def) => {
                let class_name = class_def.name.as_str();
                let span = text_range_to_bytespan(class_def.range())?;
                let class_id = TargetId::derive([
                    b"python".as_slice(),
                    b"class".as_slice(),
                    self.path.as_str().as_bytes(),
                    class_name.as_bytes(),
                ]);

                let visibility = if class_name.starts_with('_') && !is_dunder(class_name) {
                    TargetVisibility::Private
                } else {
                    TargetVisibility::Public
                };

                self.targets.push(Target {
                    id: class_id.clone(),
                    kind: TargetKind::Portable {
                        kind: PortableTargetKind::Type,
                    },
                    visibility,
                    name: class_name.to_owned(),
                    parent: Some(current_scope_id.clone()),
                    location: Some(SourceLocation {
                        path: self.path.clone(),
                        bytes: span,
                        start: None,
                        end: None,
                    }),
                    inventory: InventoryState::Represented,
                    capabilities: vec![syntax_capability(false)],
                    diagnostic: None,
                });

                if let Some(doc) = extract_docstring(&class_def.body) {
                    self.documentation.insert(class_id.clone(), doc);
                }

                // Collect bases for inheritance
                if let Some(args) = &class_def.arguments {
                    for arg in &args.args {
                        let base_name = expr_to_name(arg);
                        if !base_name.is_empty() {
                            let arg_span = text_range_to_bytespan(arg.range())?;
                            self.inheritances.push(DiscoveredInheritance {
                                sub_target_id: class_id.clone(),
                                base_name,
                                span: arg_span,
                            });
                        }
                    }
                }

                // Process class body
                self.collect_statements(&class_def.body, &class_id, Some(class_name))?;
            }

            Stmt::FunctionDef(fn_def) => {
                let fn_name = fn_def.name.as_str();
                let span = text_range_to_bytespan(fn_def.range())?;
                let fn_id = match current_class {
                    Some(cls) => TargetId::derive([
                        b"python".as_slice(),
                        b"method".as_slice(),
                        self.path.as_str().as_bytes(),
                        cls.as_bytes(),
                        fn_name.as_bytes(),
                    ]),
                    None => TargetId::derive([
                        b"python".as_slice(),
                        b"function".as_slice(),
                        self.path.as_str().as_bytes(),
                        fn_name.as_bytes(),
                    ]),
                };

                let visibility = if fn_name.starts_with('_') && !is_dunder(fn_name) {
                    TargetVisibility::Private
                } else {
                    TargetVisibility::Public
                };

                self.targets.push(Target {
                    id: fn_id.clone(),
                    kind: TargetKind::Portable {
                        kind: PortableTargetKind::Callable,
                    },
                    visibility,
                    name: fn_name.to_owned(),
                    parent: Some(current_scope_id.clone()),
                    location: Some(SourceLocation {
                        path: self.path.clone(),
                        bytes: span,
                        start: None,
                        end: None,
                    }),
                    inventory: InventoryState::Represented,
                    capabilities: vec![syntax_capability(false)],
                    diagnostic: None,
                });

                if let Some(doc) = extract_docstring(&fn_def.body) {
                    self.documentation.insert(fn_id.clone(), doc);
                }

                self.collect_statements(&fn_def.body, &fn_id, current_class)?;
            }

            Stmt::Assign(assign_stmt) => {
                for target_expr in &assign_stmt.targets {
                    if let Expr::Name(name_expr) = target_expr {
                        let var_name = name_expr.id.as_str();
                        let is_const = var_name.chars().all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit());
                        let kind = if is_const {
                            TargetKind::Portable {
                                kind: PortableTargetKind::Constant,
                            }
                        } else {
                            TargetKind::LanguageSpecific {
                                language: "python".to_owned(),
                                kind: "variable".to_owned(),
                            }
                        };

                        let span = text_range_to_bytespan(assign_stmt.range())?;
                        let var_id = TargetId::derive([
                            b"python".as_slice(),
                            if is_const { b"constant".as_slice() } else { b"variable".as_slice() },
                            self.path.as_str().as_bytes(),
                            var_name.as_bytes(),
                        ]);

                        let visibility = if var_name.starts_with('_') && !is_dunder(var_name) {
                            TargetVisibility::Private
                        } else {
                            TargetVisibility::Public
                        };

                        self.targets.push(Target {
                            id: var_id,
                            kind,
                            visibility,
                            name: var_name.to_owned(),
                            parent: Some(current_scope_id.clone()),
                            location: Some(SourceLocation {
                                path: self.path.clone(),
                                bytes: span,
                                start: None,
                                end: None,
                            }),
                            inventory: InventoryState::Represented,
                            capabilities: vec![syntax_capability(false)],
                            diagnostic: None,
                        });
                    }
                }
                self.scan_expr_for_calls(&assign_stmt.value, current_scope_id)?;
            }

            Stmt::AnnAssign(ann_assign) => {
                if let Expr::Name(name_expr) = &*ann_assign.target {
                    let var_name = name_expr.id.as_str();
                    let is_const = var_name.chars().all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit());
                    let kind = if is_const {
                        TargetKind::Portable {
                            kind: PortableTargetKind::Constant,
                        }
                    } else {
                        TargetKind::LanguageSpecific {
                            language: "python".to_owned(),
                            kind: "variable".to_owned(),
                        }
                    };

                    let span = text_range_to_bytespan(ann_assign.range())?;
                    let var_id = TargetId::derive([
                        b"python".as_slice(),
                        if is_const { b"constant".as_slice() } else { b"variable".as_slice() },
                        self.path.as_str().as_bytes(),
                        var_name.as_bytes(),
                    ]);

                    let visibility = if var_name.starts_with('_') && !is_dunder(var_name) {
                        TargetVisibility::Private
                    } else {
                        TargetVisibility::Public
                    };

                    self.targets.push(Target {
                        id: var_id,
                        kind,
                        visibility,
                        name: var_name.to_owned(),
                        parent: Some(current_scope_id.clone()),
                        location: Some(SourceLocation {
                            path: self.path.clone(),
                            bytes: span,
                            start: None,
                            end: None,
                        }),
                        inventory: InventoryState::Represented,
                        capabilities: vec![syntax_capability(false)],
                        diagnostic: None,
                    });
                }
                if let Some(val) = &ann_assign.value {
                    self.scan_expr_for_calls(val, current_scope_id)?;
                }
            }

            Stmt::Import(import_stmt) => {
                for alias in &import_stmt.names {
                    let name = alias.name.as_str();
                    let span = text_range_to_bytespan(import_stmt.range())?;
                    self.imports.push(DiscoveredImport {
                        imported_name: name.to_owned(),
                        module_path: Some(name.to_owned()),
                        is_relative: false,
                        relative_level: 0,
                        span,
                    });
                }
            }

            Stmt::ImportFrom(import_from) => {
                let mod_name = import_from.module.as_ref().map(|m| m.as_str().to_owned());
                let level = import_from.level;
                let span = text_range_to_bytespan(import_from.range())?;
                for alias in &import_from.names {
                    let imported = alias.name.as_str().to_owned();
                    self.imports.push(DiscoveredImport {
                        imported_name: imported,
                        module_path: mod_name.clone(),
                        is_relative: level > 0,
                        relative_level: level,
                        span,
                    });
                }
            }

            Stmt::Expr(expr_stmt) => {
                self.scan_expr_for_calls(&expr_stmt.value, current_scope_id)?;
            }

            Stmt::Return(ret_stmt) => {
                if let Some(val) = &ret_stmt.value {
                    self.scan_expr_for_calls(val, current_scope_id)?;
                }
            }

            Stmt::If(if_stmt) => {
                self.scan_expr_for_calls(&if_stmt.test, current_scope_id)?;
                self.collect_statements(&if_stmt.body, current_scope_id, current_class)?;
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.scan_expr_for_calls(test, current_scope_id)?;
                    }
                    self.collect_statements(&clause.body, current_scope_id, current_class)?;
                }
            }

            Stmt::For(for_stmt) => {
                self.scan_expr_for_calls(&for_stmt.iter, current_scope_id)?;
                self.collect_statements(&for_stmt.body, current_scope_id, current_class)?;
            }

            Stmt::While(while_stmt) => {
                self.scan_expr_for_calls(&while_stmt.test, current_scope_id)?;
                self.collect_statements(&while_stmt.body, current_scope_id, current_class)?;
            }

            Stmt::With(with_stmt) => {
                for item in &with_stmt.items {
                    self.scan_expr_for_calls(&item.context_expr, current_scope_id)?;
                }
                self.collect_statements(&with_stmt.body, current_scope_id, current_class)?;
            }

            Stmt::Try(try_stmt) => {
                self.collect_statements(&try_stmt.body, current_scope_id, current_class)?;
                for handler in &try_stmt.handlers {
                    match handler {
                        ast::ExceptHandler::ExceptHandler(h) => {
                            self.collect_statements(&h.body, current_scope_id, current_class)?;
                        }
                    }
                }
                self.collect_statements(&try_stmt.finalbody, current_scope_id, current_class)?;
            }

            _ => {}
        }
        Ok(())
    }

    fn scan_expr_for_calls(
        &mut self,
        expr: &Expr,
        current_scope_id: &TargetId,
    ) -> Result<(), argus_core::ArgusError> {
        match expr {
            Expr::Call(call_expr) => {
                let callee_name = expr_to_name(&call_expr.func);
                if !callee_name.is_empty() {
                    let span = text_range_to_bytespan(call_expr.range())?;
                    self.calls.push(DiscoveredCallSite {
                        caller_target_id: current_scope_id.clone(),
                        callee_name,
                        span,
                    });
                }
                for arg in &call_expr.arguments.args {
                    self.scan_expr_for_calls(arg, current_scope_id)?;
                }
                for kw in &call_expr.arguments.keywords {
                    self.scan_expr_for_calls(&kw.value, current_scope_id)?;
                }
            }
            Expr::Attribute(attr_expr) => {
                self.scan_expr_for_calls(&attr_expr.value, current_scope_id)?;
            }
            Expr::BinOp(bin_op) => {
                self.scan_expr_for_calls(&bin_op.left, current_scope_id)?;
                self.scan_expr_for_calls(&bin_op.right, current_scope_id)?;
            }
            Expr::UnaryOp(un_op) => {
                self.scan_expr_for_calls(&un_op.operand, current_scope_id)?;
            }
            Expr::List(list_expr) => {
                for el in &list_expr.elts {
                    self.scan_expr_for_calls(el, current_scope_id)?;
                }
            }
            Expr::Tuple(tuple_expr) => {
                for el in &tuple_expr.elts {
                    self.scan_expr_for_calls(el, current_scope_id)?;
                }
            }
            Expr::Dict(dict_expr) => {
                for item in &dict_expr.items {
                    if let Some(k) = &item.key {
                        self.scan_expr_for_calls(k, current_scope_id)?;
                    }
                    self.scan_expr_for_calls(&item.value, current_scope_id)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn text_range_to_bytespan(range: TextRange) -> Result<ByteSpan, argus_core::ArgusError> {
    let start = u64::from(range.start().to_u32());
    let end = u64::from(range.end().to_u32());
    ByteSpan::new(start, end)
}

fn syntax_capability(failed: bool) -> Capability {
    Capability {
        name: "python-syntax".to_owned(),
        status: if failed {
            CapabilityStatus::Failed
        } else {
            CapabilityStatus::Complete
        },
        detail: None,
        provider: Some(PROVIDER.to_owned()),
    }
}

fn is_dunder(name: &str) -> bool {
    name.starts_with("__") && name.ends_with("__") && name.len() >= 4
}

fn extract_docstring(body: &[Stmt]) -> Option<String> {
    let first = body.first()?;
    if let Stmt::Expr(expr_stmt) = first {
        if let Expr::StringLiteral(str_lit) = &*expr_stmt.value {
            return Some(str_lit.value.to_str().to_owned());
        }
    }
    None
}

fn expr_to_name(expr: &Expr) -> String {
    match expr {
        Expr::Name(name_expr) => name_expr.id.as_str().to_owned(),
        Expr::Attribute(attr_expr) => {
            let base = expr_to_name(&attr_expr.value);
            if base.is_empty() {
                attr_expr.attr.as_str().to_owned()
            } else {
                format!("{}.{}", base, attr_expr.attr.as_str())
            }
        }
        _ => String::new(),
    }
}
