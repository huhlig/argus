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
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPatternKind, Class, ClassElement, Declaration, ExportDefaultDeclarationKind, Function,
    MethodDefinitionKind, PropertyKey, Statement, TSAccessibility, TSEnumDeclaration,
    TSInterfaceDeclaration, TSModuleDeclaration, TSModuleDeclarationBody, TSModuleDeclarationName,
    TSTypeAliasDeclaration, VariableDeclaration, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use std::{collections::BTreeMap, path::Path};

const PROVIDER: &str = "oxc-syntax";
const PROVIDER_VERSION: &str = "0.75.1";

/// Dialect classification for TypeScript / JavaScript files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeScriptDialect {
    TypeScript,
    TypeScriptJsx,
    JavaScript,
    JavaScriptJsx,
}

impl TypeScriptDialect {
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::TypeScriptJsx),
            "js" | "mjs" | "cjs" => Some(Self::JavaScript),
            "jsx" => Some(Self::JavaScriptJsx),
            _ => None,
        }
    }
}

/// Discovered import statement in a source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredImport {
    pub file_path: SourcePath,
    pub source_module: String,
    pub imported_symbols: Vec<String>,
    pub is_type_only: bool,
    pub span: Span,
}

/// Discovered inheritance or implementation link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredInheritance {
    pub sub_target_id: TargetId,
    pub sub_name: String,
    pub super_name: String,
    pub is_implements: bool,
}

/// Result of parsing a TypeScript / JavaScript file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypeScriptSyntaxInventory {
    pub targets: Vec<Target>,
    pub diagnostics: Vec<String>,
    pub documentation: BTreeMap<TargetId, String>,
    pub imports: Vec<DiscoveredImport>,
    pub inheritances: Vec<DiscoveredInheritance>,
}

/// Syntax analyzer for TypeScript and JavaScript using oxc.
#[derive(Clone, Debug)]
pub struct TypeScriptSyntaxProvider {
    configuration: ConfigurationId,
}

impl TypeScriptSyntaxProvider {
    #[must_use]
    pub fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    /// Checks if a file path is a supported TypeScript/JavaScript source file and not ignored.
    #[must_use]
    pub fn is_supported_source(path: &SourcePath) -> bool {
        let s = path.as_str();
        if is_ignored_path(s) {
            return false;
        }
        TypeScriptDialect::from_path(s).is_some()
    }

    /// Parses a single source file and collects syntax targets, docs, imports, and diagnostics.
    pub fn inventory_file(
        &self,
        source: &dyn SourceAccess,
        path: &SourcePath,
        parent: Option<TargetId>,
    ) -> Result<TypeScriptSyntaxInventory, argus_core::ArgusError> {
        let bytes = source.read(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            argus_core::ArgusError::invalid_input(format!(
                "TypeScript syntax provider requires UTF-8 source for `{}`",
                path.as_str()
            ))
            .with_source(error)
        })?;

        let source_type = SourceType::from_path(path.as_str()).unwrap_or_else(|_| {
            SourceType::default().with_typescript(true).with_module(true)
        });

        let allocator = Allocator::default();
        let ret = Parser::new(&allocator, text, source_type).parse();

        let diagnostics = ret
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        // Extract JSDoc comments: map end byte position of comment to comment text
        let mut jsdoc_comments = BTreeMap::new();
        for comment in &ret.program.comments {
            let start = usize::try_from(comment.span.start).unwrap_or(0);
            let end = usize::try_from(comment.span.end).unwrap_or(0);
            if let Some(comment_str) = text.get(start..end) {
                if comment_str.starts_with("/**") {
                    jsdoc_comments.insert(comment.span.end, comment_str.to_owned());
                }
            }
        }

        let file_span = Span::new(0, u32::try_from(text.len()).unwrap_or(0));
        let file_id = target_id(path, "file", path.as_str());

        let mut targets = vec![Target {
            id: file_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::File,
            },
            visibility: TargetVisibility::NotApplicable,
            name: path.as_str().to_owned(),
            parent,
            location: Some(create_location(path, file_span)?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(!diagnostics.is_empty())],
            diagnostic: None,
        }];

        let mut documentation = BTreeMap::new();
        let mut imports = Vec::new();
        let mut inheritances = Vec::new();

        let mut collector = TargetCollector {
            path,
            text,
            file_id: &file_id,
            jsdoc_comments: &jsdoc_comments,
            targets: &mut targets,
            documentation: &mut documentation,
            imports: &mut imports,
            inheritances: &mut inheritances,
        };

        collector.collect_statements(&ret.program.body, &file_id, "")?;

        Ok(TypeScriptSyntaxInventory {
            targets,
            diagnostics,
            documentation,
            imports,
            inheritances,
        })
    }

    /// Generates evidence records (Source and Documentation) for all targets in the syntax inventory.
    #[allow(clippy::missing_panics_doc)]
    pub fn review_evidence(
        &self,
        source: &dyn SourceAccess,
        syntax: &TypeScriptSyntaxInventory,
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
                    b"typescript-syntax-documentation".as_slice(),
                    target.id.as_str().as_bytes(),
                    presence,
                    documentation.map_or(b"".as_slice(), |t| t.as_bytes()),
                ]),
                kind: EvidenceKind::Documentation,
                origin: EvidenceOrigin::Direct,
                target: Some(target.id.clone()),
                location: target.location.clone(),
                summary: if documentation.is_some() {
                    format!("TypeScript documentation for {}", target.name)
                } else {
                    format!("No TypeScript documentation is attached to {}", target.name)
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
                    b"typescript-syntax-source".as_slice(),
                    target.id.as_str().as_bytes(),
                    span,
                ]),
                kind: EvidenceKind::Source,
                origin: EvidenceOrigin::Direct,
                target: Some(target.id.clone()),
                location: Some(location.clone()),
                summary: format!("TypeScript source for {}", target.name),
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
    text: &'a str,
    #[allow(dead_code)]
    file_id: &'a TargetId,
    jsdoc_comments: &'a BTreeMap<u32, String>,
    targets: &'a mut Vec<Target>,
    documentation: &'a mut BTreeMap<TargetId, String>,
    imports: &'a mut Vec<DiscoveredImport>,
    inheritances: &'a mut Vec<DiscoveredInheritance>,
}

impl TargetCollector<'_> {
    fn collect_statements(
        &mut self,
        statements: &[Statement<'_>],
        parent_id: &TargetId,
        prefix: &str,
    ) -> Result<(), argus_core::ArgusError> {
        for stmt in statements {
            self.collect_statement(stmt, parent_id, prefix, false)?;
        }
        Ok(())
    }

    fn collect_statement(
        &mut self,
        stmt: &Statement<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
    ) -> Result<(), argus_core::ArgusError> {
        let outer_start = stmt.span().start;
        match stmt {
            Statement::FunctionDeclaration(func) => {
                self.collect_function(func, parent_id, prefix, exported, Some(outer_start))?;
            }
            Statement::ClassDeclaration(class) => {
                self.collect_class(class, parent_id, prefix, exported, Some(outer_start))?;
            }
            Statement::TSInterfaceDeclaration(interface) => {
                self.collect_interface(interface, parent_id, prefix, exported, Some(outer_start))?;
            }
            Statement::TSTypeAliasDeclaration(alias) => {
                self.collect_type_alias(alias, parent_id, prefix, exported, Some(outer_start))?;
            }
            Statement::TSEnumDeclaration(enum_decl) => {
                self.collect_enum(enum_decl, parent_id, prefix, exported, Some(outer_start))?;
            }
            Statement::TSModuleDeclaration(module_decl) => {
                self.collect_module(module_decl, parent_id, prefix, exported, Some(outer_start))?;
            }
            Statement::VariableDeclaration(var_decl) => {
                self.collect_variable(var_decl, parent_id, prefix, exported, Some(outer_start))?;
            }
            Statement::ExportNamedDeclaration(export_decl) => {
                if let Some(decl) = &export_decl.declaration {
                    self.collect_declaration(decl, parent_id, prefix, true, Some(outer_start))?;
                }
            }
            Statement::ExportDefaultDeclaration(export_default) => match &export_default.declaration
            {
                ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                    self.collect_function(func, parent_id, prefix, true, Some(outer_start))?;
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    self.collect_class(class, parent_id, prefix, true, Some(outer_start))?;
                }
                _ => {}
            },
            Statement::ImportDeclaration(import_decl) => {
                let source_module = import_decl.source.value.to_string();
                let is_type_only = import_decl.import_kind.is_type();
                let mut imported_symbols = Vec::new();
                if let Some(specifiers) = &import_decl.specifiers {
                    for spec in specifiers {
                        match spec {
                            oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                imported_symbols.push(s.imported.name().to_string());
                            }
                            oxc_ast::ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                imported_symbols.push(s.local.name.to_string());
                            }
                            oxc_ast::ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(
                                s,
                            ) => {
                                imported_symbols.push(format!("* as {}", s.local.name));
                            }
                        }
                    }
                }
                self.imports.push(DiscoveredImport {
                    file_path: self.path.clone(),
                    source_module,
                    imported_symbols,
                    is_type_only,
                    span: import_decl.span,
                });
            }
            _ => {}
        }
        Ok(())
    }

    #[allow(clippy::match_wildcard_for_single_variants)]
    fn collect_declaration(
        &mut self,
        decl: &Declaration<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        match decl {
            Declaration::FunctionDeclaration(func) => {
                self.collect_function(func, parent_id, prefix, exported, outer_start)
            }
            Declaration::ClassDeclaration(class) => {
                self.collect_class(class, parent_id, prefix, exported, outer_start)
            }
            Declaration::TSInterfaceDeclaration(interface) => {
                self.collect_interface(interface, parent_id, prefix, exported, outer_start)
            }
            Declaration::TSTypeAliasDeclaration(alias) => {
                self.collect_type_alias(alias, parent_id, prefix, exported, outer_start)
            }
            Declaration::TSEnumDeclaration(enum_decl) => {
                self.collect_enum(enum_decl, parent_id, prefix, exported, outer_start)
            }
            Declaration::TSModuleDeclaration(module_decl) => {
                self.collect_module(module_decl, parent_id, prefix, exported, outer_start)
            }
            Declaration::VariableDeclaration(var_decl) => {
                self.collect_variable(var_decl, parent_id, prefix, exported, outer_start)
            }
            _ => Ok(()),
        }
    }

    fn collect_function(
        &mut self,
        func: &Function<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        let name = func
            .id
            .as_ref()
            .map_or("default", |ident| ident.name.as_str());
        let qualified = qualified_name(prefix, name);
        let id = target_id(self.path, "function", &qualified);
        let visibility = if exported {
            TargetVisibility::Public
        } else {
            TargetVisibility::Private
        };

        self.add_target(Target {
            id: id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Callable,
            },
            visibility,
            name: name.to_owned(),
            parent: Some(parent_id.clone()),
            location: Some(create_location(self.path, func.span)?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(false)],
            diagnostic: None,
        });
        self.attach_doc(&id, func.span.start, outer_start);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn collect_class(
        &mut self,
        class: &Class<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        let name = class
            .id
            .as_ref()
            .map_or("default", |ident| ident.name.as_str());
        let qualified = qualified_name(prefix, name);
        let id = target_id(self.path, "class", &qualified);
        let visibility = if exported {
            TargetVisibility::Public
        } else {
            TargetVisibility::Private
        };

        self.add_target(Target {
            id: id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            visibility,
            name: name.to_owned(),
            parent: Some(parent_id.clone()),
            location: Some(create_location(self.path, class.span)?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(false)],
            diagnostic: None,
        });
        self.attach_doc(&id, class.span.start, outer_start);

        // Record super class
        if let Some(super_class) = &class.super_class {
            let start = usize::try_from(super_class.span().start).unwrap_or(0);
            let end = usize::try_from(super_class.span().end).unwrap_or(0);
            let super_name = self.text.get(start..end).unwrap_or("").to_owned();
            self.inheritances.push(DiscoveredInheritance {
                sub_target_id: id.clone(),
                sub_name: name.to_owned(),
                super_name,
                is_implements: false,
            });
        }

        // Record implemented interfaces
        for imp in &class.implements {
            let start = usize::try_from(imp.expression.span().start).unwrap_or(0);
            let end = usize::try_from(imp.expression.span().end).unwrap_or(0);
            let interface_name = self.text.get(start..end).unwrap_or("").to_owned();
            self.inheritances.push(DiscoveredInheritance {
                sub_target_id: id.clone(),
                sub_name: name.to_owned(),
                super_name: interface_name,
                is_implements: true,
            });
        }

        // Collect class members
        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method_name = property_key_name(&method.key);
                    let method_qualified = format!("{qualified}.{method_name}");
                    let is_constructor = method.kind == MethodDefinitionKind::Constructor;
                    let kind_key = if is_constructor {
                        "constructor"
                    } else {
                        "method"
                    };
                    let method_id = target_id(self.path, kind_key, &method_qualified);
                    let visibility = match method.accessibility {
                        Some(TSAccessibility::Public) | None => TargetVisibility::Public,
                        Some(TSAccessibility::Protected) => TargetVisibility::Restricted,
                        Some(TSAccessibility::Private) => TargetVisibility::Private,
                    };

                    self.add_target(Target {
                        id: method_id.clone(),
                        kind: TargetKind::Portable {
                            kind: PortableTargetKind::Callable,
                        },
                        visibility,
                        name: method_name,
                        parent: Some(id.clone()),
                        location: Some(create_location(self.path, method.span)?),
                        inventory: InventoryState::Represented,
                        capabilities: vec![syntax_capability(false)],
                        diagnostic: None,
                    });
                    self.attach_doc(&method_id, method.span.start, None);
                }
                ClassElement::PropertyDefinition(prop) => {
                    let prop_name = property_key_name(&prop.key);
                    let prop_qualified = format!("{qualified}.{prop_name}");
                    let prop_id = target_id(self.path, "property", &prop_qualified);
                    let visibility = match prop.accessibility {
                        Some(TSAccessibility::Public) | None => TargetVisibility::Public,
                        Some(TSAccessibility::Protected) => TargetVisibility::Restricted,
                        Some(TSAccessibility::Private) => TargetVisibility::Private,
                    };

                    self.add_target(Target {
                        id: prop_id.clone(),
                        kind: TargetKind::Portable {
                            kind: PortableTargetKind::Constant,
                        },
                        visibility,
                        name: prop_name,
                        parent: Some(id.clone()),
                        location: Some(create_location(self.path, prop.span)?),
                        inventory: InventoryState::Represented,
                        capabilities: vec![syntax_capability(false)],
                        diagnostic: None,
                    });
                    self.attach_doc(&prop_id, prop.span.start, None);
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn collect_interface(
        &mut self,
        interface: &TSInterfaceDeclaration<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        let name = interface.id.name.as_str();
        let qualified = qualified_name(prefix, name);
        let id = target_id(self.path, "interface", &qualified);
        let visibility = if exported {
            TargetVisibility::Public
        } else {
            TargetVisibility::Private
        };
        self.add_target(Target {
            id: id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            visibility,
            name: name.to_owned(),
            parent: Some(parent_id.clone()),
            location: Some(create_location(self.path, interface.span)?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(false)],
            diagnostic: None,
        });
        self.attach_doc(&id, interface.span.start, outer_start);

        for ext in &interface.extends {
            let start = usize::try_from(ext.expression.span().start).unwrap_or(0);
            let end = usize::try_from(ext.expression.span().end).unwrap_or(0);
            let super_name = self.text.get(start..end).unwrap_or("").to_owned();
            self.inheritances.push(DiscoveredInheritance {
                sub_target_id: id.clone(),
                sub_name: name.to_owned(),
                super_name,
                is_implements: false,
            });
        }
        Ok(())
    }

    fn collect_type_alias(
        &mut self,
        alias: &TSTypeAliasDeclaration<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        let name = alias.id.name.as_str();
        let qualified = qualified_name(prefix, name);
        let id = target_id(self.path, "type_alias", &qualified);
        let visibility = if exported {
            TargetVisibility::Public
        } else {
            TargetVisibility::Private
        };
        self.add_target(Target {
            id: id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            visibility,
            name: name.to_owned(),
            parent: Some(parent_id.clone()),
            location: Some(create_location(self.path, alias.span)?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(false)],
            diagnostic: None,
        });
        self.attach_doc(&id, alias.span.start, outer_start);
        Ok(())
    }

    fn collect_enum(
        &mut self,
        enum_decl: &TSEnumDeclaration<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        let name = enum_decl.id.name.as_str();
        let qualified = qualified_name(prefix, name);
        let id = target_id(self.path, "enum", &qualified);
        let visibility = if exported {
            TargetVisibility::Public
        } else {
            TargetVisibility::Private
        };
        self.add_target(Target {
            id: id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            visibility,
            name: name.to_owned(),
            parent: Some(parent_id.clone()),
            location: Some(create_location(self.path, enum_decl.span)?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(false)],
            diagnostic: None,
        });
        self.attach_doc(&id, enum_decl.span.start, outer_start);

        for member in &enum_decl.body.members {
            let start = usize::try_from(member.id.span().start).unwrap_or(0);
            let end = usize::try_from(member.id.span().end).unwrap_or(0);
            let member_name = self.text.get(start..end).unwrap_or("unknown");
            let member_qualified = format!("{qualified}.{member_name}");
            let member_id = target_id(self.path, "enum_member", &member_qualified);
            self.add_target(Target {
                id: member_id.clone(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Constant,
                },
                visibility: TargetVisibility::Public,
                name: member_name.to_owned(),
                parent: Some(id.clone()),
                location: Some(create_location(self.path, member.span)?),
                inventory: InventoryState::Represented,
                capabilities: vec![syntax_capability(false)],
                diagnostic: None,
            });
            self.attach_doc(&member_id, member.span.start, None);
        }
        Ok(())
    }

    fn collect_module(
        &mut self,
        module_decl: &TSModuleDeclaration<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        let name = match &module_decl.id {
            TSModuleDeclarationName::Identifier(ident) => ident.name.as_str(),
            TSModuleDeclarationName::StringLiteral(lit) => lit.value.as_str(),
        };
        let qualified = qualified_name(prefix, name);
        let id = target_id(self.path, "namespace", &qualified);
        let visibility = if exported {
            TargetVisibility::Public
        } else {
            TargetVisibility::Private
        };
        self.add_target(Target {
            id: id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Module,
            },
            visibility,
            name: name.to_owned(),
            parent: Some(parent_id.clone()),
            location: Some(create_location(self.path, module_decl.span)?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(false)],
            diagnostic: None,
        });
        self.attach_doc(&id, module_decl.span.start, outer_start);

        if let Some(body) = &module_decl.body {
            match body {
                TSModuleDeclarationBody::TSModuleBlock(block) => {
                    self.collect_statements(&block.body, &id, &qualified)?;
                }
                TSModuleDeclarationBody::TSModuleDeclaration(nested) => {
                    self.collect_module(nested, &id, &qualified, false, None)?;
                }
            }
        }
        Ok(())
    }

    fn collect_variable(
        &mut self,
        var_decl: &VariableDeclaration<'_>,
        parent_id: &TargetId,
        prefix: &str,
        exported: bool,
        outer_start: Option<u32>,
    ) -> Result<(), argus_core::ArgusError> {
        for decl in &var_decl.declarations {
            let BindingPatternKind::BindingIdentifier(ident) = &decl.id.kind else {
                continue;
            };
            let name = ident.name.as_str();
            let qualified = qualified_name(prefix, name);

            let is_callable = decl.init.as_ref().is_some_and(|init| {
                matches!(
                    init,
                    oxc_ast::ast::Expression::ArrowFunctionExpression(_)
                        | oxc_ast::ast::Expression::FunctionExpression(_)
                )
            });

            let (portable_kind, kind_key) = if is_callable {
                (PortableTargetKind::Callable, "arrow_function")
            } else if var_decl.kind == VariableDeclarationKind::Const {
                (PortableTargetKind::Constant, "const")
            } else {
                (PortableTargetKind::Constant, "variable")
            };

            let id = target_id(self.path, kind_key, &qualified);
            let visibility = if exported {
                TargetVisibility::Public
            } else {
                TargetVisibility::Private
            };

            self.add_target(Target {
                id: id.clone(),
                kind: TargetKind::Portable {
                    kind: portable_kind,
                },
                visibility,
                name: name.to_owned(),
                parent: Some(parent_id.clone()),
                location: Some(create_location(self.path, decl.span)?),
                inventory: InventoryState::Represented,
                capabilities: vec![syntax_capability(false)],
                diagnostic: None,
            });
            self.attach_doc(&id, decl.span.start, outer_start);
        }
        Ok(())
    }

    fn add_target(&mut self, target: Target) {
        self.targets.push(target);
    }

    fn attach_doc(&mut self, target_id: &TargetId, node_start: u32, outer_start: Option<u32>) {
        let search_pos = outer_start.unwrap_or(node_start);
        let preceding = self
            .jsdoc_comments
            .range(..=search_pos)
            .next_back();

        if let Some((&comment_end, comment_text)) = preceding {
            let gap = usize::try_from(comment_end)
                .ok()
                .zip(usize::try_from(search_pos).ok())
                .and_then(|(s, e)| self.text.get(s..e));

            if let Some(gap_str) = gap {
                if gap_str.trim().is_empty() {
                    self.documentation.insert(target_id.clone(), comment_text.clone());
                }
            }
        }
    }
}

fn property_key_name(key: &PropertyKey<'_>) -> String {
    match key {
        PropertyKey::StaticIdentifier(ident) => ident.name.to_string(),
        PropertyKey::Identifier(ident) => ident.name.to_string(),
        PropertyKey::StringLiteral(lit) => lit.value.to_string(),
        _ => "computed".to_owned(),
    }
}

fn qualified_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}::{name}")
    }
}

fn target_id(path: &SourcePath, kind: &str, name: &str) -> TargetId {
    TargetId::derive([
        b"typescript-syntax".as_slice(),
        path.as_str().as_bytes(),
        kind.as_bytes(),
        name.as_bytes(),
    ])
}

fn create_location(
    path: &SourcePath,
    span: Span,
) -> Result<SourceLocation, argus_core::ArgusError> {
    Ok(SourceLocation {
        path: path.clone(),
        bytes: ByteSpan::new(u64::from(span.start), u64::from(span.end))?,
        start: None,
        end: None,
    })
}

fn syntax_capability(partial: bool) -> Capability {
    Capability {
        name: "typescript-syntax".to_owned(),
        status: if partial {
            CapabilityStatus::Partial
        } else {
            CapabilityStatus::Complete
        },
        detail: partial.then(|| "source contains recoverable syntax errors".to_owned()),
        provider: Some(PROVIDER.to_owned()),
    }
}

fn is_ignored_path(path: &str) -> bool {
    path.contains("node_modules/")
        || path.starts_with("node_modules")
        || path.contains(".git/")
        || path.starts_with(".git")
        || path.contains(".argus/")
        || path.starts_with(".argus")
        || path.contains("dist/")
        || path.starts_with("dist")
        || path.contains("build/")
        || path.starts_with("build")
        || path.contains(".next/")
        || path.starts_with(".next")
        || path.contains("coverage/")
        || path.starts_with("coverage")
}
