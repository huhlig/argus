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
use java_lang::ast::{
    Comment,
    compilation_unit::{CompilationUnit, ImportDecl},
    expr::Expr,
    item::{
        ClassBodyDecl, ClassDecl, CompactConstructorDecl, ConstructorDecl, EnumConstant, EnumDecl,
        FieldDecl, InterfaceBody, InterfaceDecl, InterfaceMemberDecl, MethodDecl, Modifier,
        RecordBodyDecl, RecordDecl, TypeDecl,
    },
    stmt::{Block, Stmt},
    ty::Type,
};

const PROVIDER: &str = "java-syntax";
const PROVIDER_VERSION: &str = "0.3.2";

/// Discovered import statement in a Java source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredJavaImport {
    pub path: String,
    pub is_static: bool,
    pub is_wildcard: bool,
    pub span: ByteSpan,
}

/// Discovered class inheritance / interface implementation relationship.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredJavaInheritance {
    pub sub_target_id: TargetId,
    pub base_name: String,
    pub is_interface: bool,
    pub span: ByteSpan,
}

/// Discovered method call site.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredJavaCallSite {
    pub caller_target_id: TargetId,
    pub callee_name: String,
    pub receiver: Option<String>,
    pub span: ByteSpan,
}

/// Parsed Java syntax inventory for a single source file.
#[derive(Clone, Debug, Default)]
pub struct JavaSyntaxInventory {
    pub targets: Vec<Target>,
    pub evidence: Vec<EvidenceRecord>,
    pub diagnostics: Vec<String>,
    pub imports: Vec<DiscoveredJavaImport>,
    pub inheritances: Vec<DiscoveredJavaInheritance>,
    pub calls: Vec<DiscoveredJavaCallSite>,
    pub containments: Vec<(TargetId, TargetId)>,
}

/// Provider that extracts AST targets, documentation, and relationships from Java files.
#[derive(Clone, Debug)]
pub struct JavaSyntaxProvider {
    configuration: ConfigurationId,
}

impl JavaSyntaxProvider {
    #[must_use]
    pub const fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    /// Parses a Java source file and extracts all targets, Javadoc evidence, imports, inheritances, and calls.
    #[allow(clippy::too_many_lines)]
    pub fn parse_file(
        &self,
        path: &SourcePath,
        text: &str,
        parent: Option<&TargetId>,
    ) -> Result<JavaSyntaxInventory, argus_core::ArgusError> {
        let mut inventory = JavaSyntaxInventory::default();

        let cu: CompilationUnit = match java_lang::parse_str(text) {
            Ok(unit) => unit,
            Err(err) => {
                inventory
                    .diagnostics
                    .push(format!("parse error in {}: {err}", path.as_str()));
                return Ok(inventory);
            }
        };

        // 1. Emit File target
        let file_span = ByteSpan::new(0, text.len() as u64)?;
        let file_target_id = TargetId::derive([
            b"java".as_slice(),
            b"file".as_slice(),
            path.as_str().as_bytes(),
        ]);
        let file_location = SourceLocation {
            path: path.clone(),
            bytes: file_span,
            start: None,
            end: None,
        };

        let file_target = Target {
            id: file_target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::File,
            },
            name: path.as_str().to_string(),
            parent: parent.cloned(),
            location: Some(file_location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility: TargetVisibility::NotApplicable,
            diagnostic: None,
        };
        inventory.targets.push(file_target);

        if let Some(p) = parent {
            inventory.containments.push((p.clone(), file_target_id.clone()));
        }

        // 2. Package declaration
        let package_name = cu.package.as_ref().map(|p| p.name.to_string());

        // 3. Imports
        for imp in &cu.imports {
            let span = ByteSpan::new(imp.span().start() as u64, imp.span().end() as u64)?;
            match imp {
                ImportDecl::SingleType { path: p, .. } => {
                    inventory.imports.push(DiscoveredJavaImport {
                        path: p.to_string(),
                        is_static: false,
                        is_wildcard: false,
                        span,
                    });
                }
                ImportDecl::TypeOnDemand { path: p, .. } => {
                    inventory.imports.push(DiscoveredJavaImport {
                        path: p.to_string(),
                        is_static: false,
                        is_wildcard: true,
                        span,
                    });
                }
                ImportDecl::SingleStatic { path: p, member, .. } => {
                    inventory.imports.push(DiscoveredJavaImport {
                        path: format!("{}.{}", p, member.name),
                        is_static: true,
                        is_wildcard: false,
                        span,
                    });
                }
                ImportDecl::StaticOnDemand { path: p, .. } => {
                    inventory.imports.push(DiscoveredJavaImport {
                        path: p.to_string(),
                        is_static: true,
                        is_wildcard: true,
                        span,
                    });
                }
            }
        }

        // 4. Top-level Type declarations
        for decl in &cu.type_decls {
            match decl {
                TypeDecl::Class(c) => {
                    self.extract_class(path, text, package_name.as_deref(), None, c, &file_target_id, &mut inventory)?;
                }
                TypeDecl::Interface(i) => {
                    self.extract_interface(path, text, package_name.as_deref(), None, i, &file_target_id, &mut inventory)?;
                }
                TypeDecl::Record(r) => {
                    self.extract_record(path, text, package_name.as_deref(), None, r, &file_target_id, &mut inventory)?;
                }
                TypeDecl::Enum(e) => {
                    self.extract_enum(path, text, package_name.as_deref(), None, e, &file_target_id, &mut inventory)?;
                }
                TypeDecl::AnnotationType(a) => {
                    self.extract_annotation_type(path, text, package_name.as_deref(), None, a, &file_target_id, &mut inventory)?;
                }
                TypeDecl::Empty(_) => {}
            }
        }

        Ok(inventory)
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_class(
        &self,
        path: &SourcePath,
        text: &str,
        package_name: Option<&str>,
        enclosing_type: Option<&str>,
        class: &ClassDecl,
        parent_id: &TargetId,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let class_name = class.name.name.clone();
        let qualified_name = match (package_name, enclosing_type) {
            (Some(pkg), Some(enc)) => format!("{pkg}.{enc}.{class_name}"),
            (Some(pkg), None) => format!("{pkg}.{class_name}"),
            (None, Some(enc)) => format!("{enc}.{class_name}"),
            (None, None) => class_name.clone(),
        };

        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"type".as_slice(),
            qualified_name.as_bytes(),
        ]);
        let span = ByteSpan::new(class.span().start() as u64, class.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&class.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            name: class_name.clone(),
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        // Documentation
        self.extract_doc_comments(path, text, &target_id, &class.doc_comment, inventory)?;

        // Extends clause
        if let Some(ref ext) = class.extends_clause {
            let base_name = type_to_string(&ext.supertype);
            let ext_span = ByteSpan::new(ext.extends_span.start() as u64, ext.supertype.span().end() as u64)?;
            inventory.inheritances.push(DiscoveredJavaInheritance {
                sub_target_id: target_id.clone(),
                base_name,
                is_interface: false,
                span: ext_span,
            });
        }

        // Implements clause
        if let Some(ref imp) = class.implements_clause {
            for supertype in &imp.supertypes {
                let base_name = type_to_string(supertype);
                let imp_span = ByteSpan::new(supertype.span().start() as u64, supertype.span().end() as u64)?;
                inventory.inheritances.push(DiscoveredJavaInheritance {
                    sub_target_id: target_id.clone(),
                    base_name,
                    is_interface: true,
                    span: imp_span,
                });
            }
        }

        // Body declarations
        let enc_str = enclosing_type.map_or_else(|| class_name.clone(), |e| format!("{e}.{class_name}"));
        for decl in &class.body.declarations {
            self.extract_class_body_decl(path, text, package_name, &enc_str, &target_id, decl, inventory)?;
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_interface(
        &self,
        path: &SourcePath,
        text: &str,
        package_name: Option<&str>,
        enclosing_type: Option<&str>,
        iface: &InterfaceDecl,
        parent_id: &TargetId,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let iface_name = iface.name.name.clone();
        let qualified_name = match (package_name, enclosing_type) {
            (Some(pkg), Some(enc)) => format!("{pkg}.{enc}.{iface_name}"),
            (Some(pkg), None) => format!("{pkg}.{iface_name}"),
            (None, Some(enc)) => format!("{enc}.{iface_name}"),
            (None, None) => iface_name.clone(),
        };

        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"type".as_slice(),
            qualified_name.as_bytes(),
        ]);
        let span = ByteSpan::new(iface.span().start() as u64, iface.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&iface.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            name: iface_name.clone(),
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        self.extract_doc_comments(path, text, &target_id, &iface.doc_comment, inventory)?;

        if let Some(ref ext) = iface.extends_clause {
            for supertype in &ext.supertypes {
                let base_name = type_to_string(supertype);
                let ext_span = ByteSpan::new(supertype.span().start() as u64, supertype.span().end() as u64)?;
                inventory.inheritances.push(DiscoveredJavaInheritance {
                    sub_target_id: target_id.clone(),
                    base_name,
                    is_interface: true,
                    span: ext_span,
                });
            }
        }

        let enc_str = enclosing_type.map_or_else(|| iface_name.clone(), |e| format!("{e}.{iface_name}"));
        self.extract_interface_body(path, text, package_name, &enc_str, &target_id, &iface.body, inventory)?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_record(
        &self,
        path: &SourcePath,
        text: &str,
        package_name: Option<&str>,
        enclosing_type: Option<&str>,
        record: &RecordDecl,
        parent_id: &TargetId,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let rec_name = record.name.name.clone();
        let qualified_name = match (package_name, enclosing_type) {
            (Some(pkg), Some(enc)) => format!("{pkg}.{enc}.{rec_name}"),
            (Some(pkg), None) => format!("{pkg}.{rec_name}"),
            (None, Some(enc)) => format!("{enc}.{rec_name}"),
            (None, None) => rec_name.clone(),
        };

        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"type".as_slice(),
            qualified_name.as_bytes(),
        ]);
        let span = ByteSpan::new(record.span().start() as u64, record.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&record.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            name: rec_name.clone(),
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        self.extract_doc_comments(path, text, &target_id, &record.doc_comment, inventory)?;

        if let Some(ref imp) = record.implements_clause {
            for supertype in &imp.supertypes {
                let base_name = type_to_string(supertype);
                let imp_span = ByteSpan::new(supertype.span().start() as u64, supertype.span().end() as u64)?;
                inventory.inheritances.push(DiscoveredJavaInheritance {
                    sub_target_id: target_id.clone(),
                    base_name,
                    is_interface: true,
                    span: imp_span,
                });
            }
        }

        let enc_str = enclosing_type.map_or_else(|| rec_name.clone(), |e| format!("{e}.{rec_name}"));
        for member in &record.body.members {
            match member {
                RecordBodyDecl::Method(m) => {
                    self.extract_method(path, text, &enc_str, &target_id, m, inventory)?;
                }
                RecordBodyDecl::Constructor(ctor) => {
                    self.extract_constructor(path, text, &enc_str, &target_id, ctor, inventory)?;
                }
                RecordBodyDecl::CompactConstructor(cctor) => {
                    self.extract_compact_constructor(path, text, &enc_str, &target_id, cctor, inventory)?;
                }
                RecordBodyDecl::Field(f) => {
                    self.extract_field(path, text, &enc_str, &target_id, f, inventory)?;
                }
                RecordBodyDecl::Class(c) => {
                    self.extract_class(path, text, package_name, Some(&enc_str), c, &target_id, inventory)?;
                }
                RecordBodyDecl::Interface(i) => {
                    self.extract_interface(path, text, package_name, Some(&enc_str), i, &target_id, inventory)?;
                }
                RecordBodyDecl::Record(r) => {
                    self.extract_record(path, text, package_name, Some(&enc_str), r, &target_id, inventory)?;
                }
                RecordBodyDecl::Enum(e) => {
                    self.extract_enum(path, text, package_name, Some(&enc_str), e, &target_id, inventory)?;
                }
                RecordBodyDecl::InstanceInit(_) | RecordBodyDecl::StaticInit(_) | RecordBodyDecl::Empty(_) => {}
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_enum(
        &self,
        path: &SourcePath,
        text: &str,
        package_name: Option<&str>,
        enclosing_type: Option<&str>,
        en: &EnumDecl,
        parent_id: &TargetId,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let enum_name = en.name.name.clone();
        let qualified_name = match (package_name, enclosing_type) {
            (Some(pkg), Some(enc)) => format!("{pkg}.{enc}.{enum_name}"),
            (Some(pkg), None) => format!("{pkg}.{enum_name}"),
            (None, Some(enc)) => format!("{enc}.{enum_name}"),
            (None, None) => enum_name.clone(),
        };

        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"type".as_slice(),
            qualified_name.as_bytes(),
        ]);
        let span = ByteSpan::new(en.span().start() as u64, en.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&en.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            name: enum_name.clone(),
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        self.extract_doc_comments(path, text, &target_id, &en.doc_comment, inventory)?;

        if let Some(ref imp) = en.implements_clause {
            for supertype in &imp.supertypes {
                let base_name = type_to_string(supertype);
                let imp_span = ByteSpan::new(supertype.span().start() as u64, supertype.span().end() as u64)?;
                inventory.inheritances.push(DiscoveredJavaInheritance {
                    sub_target_id: target_id.clone(),
                    base_name,
                    is_interface: true,
                    span: imp_span,
                });
            }
        }

        let enc_str = enclosing_type.map_or_else(|| enum_name.clone(), |e| format!("{e}.{enum_name}"));
        for c in &en.body.constants {
            self.extract_enum_constant(path, &enc_str, &target_id, c, inventory)?;
        }
        for member in &en.body.members {
            self.extract_class_body_decl(path, text, package_name, &enc_str, &target_id, member, inventory)?;
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_annotation_type(
        &self,
        path: &SourcePath,
        text: &str,
        package_name: Option<&str>,
        enclosing_type: Option<&str>,
        anno: &java_lang::ast::item::AnnotationInterfaceDecl,
        parent_id: &TargetId,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let anno_name = anno.name.name.clone();
        let qualified_name = match (package_name, enclosing_type) {
            (Some(pkg), Some(enc)) => format!("{pkg}.{enc}.{anno_name}"),
            (Some(pkg), None) => format!("{pkg}.{anno_name}"),
            (None, Some(enc)) => format!("{enc}.{anno_name}"),
            (None, None) => anno_name.clone(),
        };

        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"type".as_slice(),
            qualified_name.as_bytes(),
        ]);
        let span = ByteSpan::new(anno.span().start() as u64, anno.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&anno.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Type,
            },
            name: anno_name,
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        self.extract_doc_comments(path, text, &target_id, &anno.doc_comment, inventory)?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_class_body_decl(
        &self,
        path: &SourcePath,
        text: &str,
        package_name: Option<&str>,
        enclosing_type: &str,
        parent_id: &TargetId,
        decl: &ClassBodyDecl,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        match decl {
            ClassBodyDecl::Method(m) => {
                self.extract_method(path, text, enclosing_type, parent_id, m, inventory)?;
            }
            ClassBodyDecl::Constructor(ctor) => {
                self.extract_constructor(path, text, enclosing_type, parent_id, ctor, inventory)?;
            }
            ClassBodyDecl::Field(f) => {
                self.extract_field(path, text, enclosing_type, parent_id, f, inventory)?;
            }
            ClassBodyDecl::Class(c) => {
                self.extract_class(path, text, package_name, Some(enclosing_type), c, parent_id, inventory)?;
            }
            ClassBodyDecl::Interface(i) => {
                self.extract_interface(path, text, package_name, Some(enclosing_type), i, parent_id, inventory)?;
            }
            ClassBodyDecl::Record(r) => {
                self.extract_record(path, text, package_name, Some(enclosing_type), r, parent_id, inventory)?;
            }
            ClassBodyDecl::Enum(e) => {
                self.extract_enum(path, text, package_name, Some(enclosing_type), e, parent_id, inventory)?;
            }
            ClassBodyDecl::AnnotationType(a) => {
                self.extract_annotation_type(path, text, package_name, Some(enclosing_type), a, parent_id, inventory)?;
            }
            ClassBodyDecl::StaticInit(_) | ClassBodyDecl::InstanceInit(_) | ClassBodyDecl::Empty(_) => {}
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_interface_body(
        &self,
        path: &SourcePath,
        text: &str,
        package_name: Option<&str>,
        enclosing_type: &str,
        parent_id: &TargetId,
        body: &InterfaceBody,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        for member in &body.members {
            match member {
                InterfaceMemberDecl::Method(m) => {
                    self.extract_method(path, text, enclosing_type, parent_id, m, inventory)?;
                }
                InterfaceMemberDecl::Field(f) => {
                    self.extract_field(path, text, enclosing_type, parent_id, f, inventory)?;
                }
                InterfaceMemberDecl::Class(c) => {
                    self.extract_class(path, text, package_name, Some(enclosing_type), c, parent_id, inventory)?;
                }
                InterfaceMemberDecl::Interface(i) => {
                    self.extract_interface(path, text, package_name, Some(enclosing_type), i, parent_id, inventory)?;
                }
                InterfaceMemberDecl::Record(r) => {
                    self.extract_record(path, text, package_name, Some(enclosing_type), r, parent_id, inventory)?;
                }
                InterfaceMemberDecl::Enum(e) => {
                    self.extract_enum(path, text, package_name, Some(enclosing_type), e, parent_id, inventory)?;
                }
                InterfaceMemberDecl::AnnotationInterface(a) => {
                    self.extract_annotation_type(path, text, package_name, Some(enclosing_type), a, parent_id, inventory)?;
                }
                InterfaceMemberDecl::Empty(_) => {}
            }
        }
        Ok(())
    }

    fn extract_method(
        &self,
        path: &SourcePath,
        text: &str,
        enclosing_type: &str,
        parent_id: &TargetId,
        method: &MethodDecl,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let name = method.name.name.clone();
        let param_types = method
            .params
            .iter()
            .map(|p| match p {
                java_lang::ast::item::FormalParameter::Normal { ty, .. } => type_to_string(ty),
                java_lang::ast::item::FormalParameter::VarArgs { ty, .. } => format!("{}...", type_to_string(ty)),
            })
            .collect::<Vec<_>>()
            .join(",");

        let sig = format!("{name}({param_types})");
        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"callable".as_slice(),
            enclosing_type.as_bytes(),
            sig.as_bytes(),
        ]);
        let span = ByteSpan::new(method.span().start() as u64, method.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&method.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Callable,
            },
            name,
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        self.extract_doc_comments(path, text, &target_id, &method.doc_comment, inventory)?;

        // Scan method body for calls
        if let Some(ref body) = method.body {
            self.scan_block_for_calls(&target_id, body, inventory)?;
        }

        Ok(())
    }

    fn extract_constructor(
        &self,
        path: &SourcePath,
        text: &str,
        enclosing_type: &str,
        parent_id: &TargetId,
        ctor: &ConstructorDecl,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let name = ctor.name.name.clone();
        let param_types = ctor
            .params
            .iter()
            .map(|p| match p {
                java_lang::ast::item::FormalParameter::Normal { ty, .. } => type_to_string(ty),
                java_lang::ast::item::FormalParameter::VarArgs { ty, .. } => format!("{}...", type_to_string(ty)),
            })
            .collect::<Vec<_>>()
            .join(",");

        let sig = format!("{name}({param_types})");
        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"callable".as_slice(),
            enclosing_type.as_bytes(),
            sig.as_bytes(),
        ]);
        let span = ByteSpan::new(ctor.span().start() as u64, ctor.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&ctor.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Callable,
            },
            name,
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        self.extract_doc_comments(path, text, &target_id, &ctor.doc_comment, inventory)?;

        for stmt in &ctor.body.stmts {
            self.scan_stmt_for_calls(&target_id, stmt, inventory)?;
        }

        Ok(())
    }

    fn extract_compact_constructor(
        &self,
        path: &SourcePath,
        text: &str,
        enclosing_type: &str,
        parent_id: &TargetId,
        cctor: &CompactConstructorDecl,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let name = cctor.name.name.clone();
        let sig = format!("{name}()");
        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"callable".as_slice(),
            enclosing_type.as_bytes(),
            sig.as_bytes(),
        ]);
        let span = ByteSpan::new(cctor.body.brace_span.0.start() as u64, cctor.body.brace_span.1.end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let visibility = visibility_from_modifiers(&cctor.modifiers);

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Callable,
            },
            name,
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id.clone()));

        self.extract_doc_comments(path, text, &target_id, &cctor.doc_comment, inventory)?;

        for stmt in &cctor.body.stmts {
            self.scan_stmt_for_calls(&target_id, stmt, inventory)?;
        }

        Ok(())
    }

    fn extract_field(
        &self,
        path: &SourcePath,
        text: &str,
        enclosing_type: &str,
        parent_id: &TargetId,
        field: &FieldDecl,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let visibility = visibility_from_modifiers(&field.modifiers);
        let is_const = is_static(&field.modifiers) && is_final(&field.modifiers);

        for d in &field.declarators {
            let field_name = d.name.as_ref().map_or_else(|| "_".to_string(), |i| i.name.clone());
            let target_id = TargetId::derive([
                b"java".as_slice(),
                b"field".as_slice(),
                enclosing_type.as_bytes(),
                field_name.as_bytes(),
            ]);
            let span = ByteSpan::new(field.span().start() as u64, field.span().end() as u64)?;
            let location = SourceLocation {
                path: path.clone(),
                bytes: span,
                start: None,
                end: None,
            };

            let kind = if is_const {
                TargetKind::Portable {
                    kind: PortableTargetKind::Constant,
                }
            } else {
                TargetKind::LanguageSpecific {
                    language: "java".to_string(),
                    kind: "field".to_string(),
                }
            };

            let target = Target {
                id: target_id.clone(),
                kind,
                name: field_name,
                parent: Some(parent_id.clone()),
                location: Some(location),
                inventory: InventoryState::Represented,
                capabilities: vec![Capability {
                    name: "java-syntax".to_string(),
                    status: CapabilityStatus::Complete,
                    detail: None,
                    provider: Some(PROVIDER.to_string()),
                }],
                visibility,
                diagnostic: None,
            };
            inventory.targets.push(target);
            inventory.containments.push((parent_id.clone(), target_id.clone()));

            self.extract_doc_comments(path, text, &target_id, &field.doc_comment, inventory)?;
        }

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn extract_enum_constant(
        &self,
        path: &SourcePath,
        enclosing_type: &str,
        parent_id: &TargetId,
        constant: &EnumConstant,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        let name = constant.name.name.clone();
        let target_id = TargetId::derive([
            b"java".as_slice(),
            b"constant".as_slice(),
            enclosing_type.as_bytes(),
            name.as_bytes(),
        ]);
        let span = ByteSpan::new(constant.name.span().start() as u64, constant.name.span().end() as u64)?;
        let location = SourceLocation {
            path: path.clone(),
            bytes: span,
            start: None,
            end: None,
        };

        let target = Target {
            id: target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Constant,
            },
            name,
            parent: Some(parent_id.clone()),
            location: Some(location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: "java-syntax".to_string(),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(PROVIDER.to_string()),
            }],
            visibility: TargetVisibility::Public,
            diagnostic: None,
        };
        inventory.targets.push(target);
        inventory.containments.push((parent_id.clone(), target_id));

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn extract_doc_comments(
        &self,
        path: &SourcePath,
        text: &str,
        target_id: &TargetId,
        comments: &[Comment],
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        for comment in comments {
            let comment_text = comment.text(text).trim();
            if comment_text.is_empty() {
                continue;
            }
            let span = ByteSpan::new(comment.span.start() as u64, comment.span.end() as u64)?;
            let evidence_id = EvidenceId::derive([
                b"java-syntax-documentation".as_slice(),
                target_id.as_str().as_bytes(),
                comment.span.start().to_string().as_bytes(),
            ]);
            let location = SourceLocation {
                path: path.clone(),
                bytes: span,
                start: None,
                end: None,
            };

            inventory.evidence.push(EvidenceRecord {
                id: evidence_id,
                kind: EvidenceKind::Documentation,
                origin: EvidenceOrigin::Direct,
                target: Some(target_id.clone()),
                location: Some(location),
                summary: format!("Java documentation for {}", target_id.as_str()),
                detail: Some(comment_text.to_string()),
                provenance: EvidenceProvenance {
                    provider: PROVIDER.to_string(),
                    provider_version: PROVIDER_VERSION.to_string(),
                    configuration: self.configuration.clone(),
                    ingest_only: true,
                    resolution: ResolutionQuality::Exact,
                },
            });
        }
        Ok(())
    }

    fn scan_block_for_calls(
        &self,
        caller: &TargetId,
        block: &Block,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        for stmt in &block.stmts {
            self.scan_stmt_for_calls(caller, stmt, inventory)?;
        }
        Ok(())
    }

    fn scan_stmt_for_calls(
        &self,
        caller: &TargetId,
        stmt: &Stmt,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        match stmt {
            Stmt::Expr(expr_stmt) => self.scan_expr_for_calls(caller, &expr_stmt.expr, inventory)?,
            Stmt::LocalVarDecl(decl) => {
                for d in &decl.declarators {
                    if let Some(ref init) = d.initializer {
                        self.scan_expr_for_calls(caller, init, inventory)?;
                    }
                }
            }
            Stmt::If(if_stmt) => {
                self.scan_expr_for_calls(caller, &if_stmt.cond, inventory)?;
                self.scan_stmt_for_calls(caller, &if_stmt.then_stmt, inventory)?;
                if let Some((_, ref else_stmt)) = if_stmt.else_clause {
                    self.scan_stmt_for_calls(caller, else_stmt, inventory)?;
                }
            }
            Stmt::Block(b) => self.scan_block_for_calls(caller, b, inventory)?,
            Stmt::Return(ret) => {
                if let Some(ref e) = ret.value {
                    self.scan_expr_for_calls(caller, e, inventory)?;
                }
            }
            Stmt::For(f) => {
                if let Some(ref c) = f.cond {
                    self.scan_expr_for_calls(caller, c, inventory)?;
                }
                self.scan_stmt_for_calls(caller, &f.body, inventory)?;
            }
            Stmt::EnhancedFor(ef) => {
                self.scan_expr_for_calls(caller, &ef.iterable, inventory)?;
                self.scan_stmt_for_calls(caller, &ef.body, inventory)?;
            }
            Stmt::While(w) => {
                self.scan_expr_for_calls(caller, &w.cond, inventory)?;
                self.scan_stmt_for_calls(caller, &w.body, inventory)?;
            }
            Stmt::DoWhile(dw) => {
                self.scan_stmt_for_calls(caller, &dw.body, inventory)?;
                self.scan_expr_for_calls(caller, &dw.cond, inventory)?;
            }
            Stmt::Try(t) => match t {
                java_lang::ast::stmt::TryStmt::Basic {
                    block,
                    catches,
                    finally_block,
                    ..
                }
                | java_lang::ast::stmt::TryStmt::TryWithResources {
                    block,
                    catches,
                    finally_block,
                    ..
                } => {
                    self.scan_block_for_calls(caller, block, inventory)?;
                    for c in catches {
                        self.scan_block_for_calls(caller, &c.block, inventory)?;
                    }
                    if let Some((_, f)) = finally_block {
                        self.scan_block_for_calls(caller, f, inventory)?;
                    }
                }
            },
            _ => {}
        }
        Ok(())
    }

    #[allow(clippy::self_only_used_in_recursion)]
    fn scan_expr_for_calls(
        &self,
        caller: &TargetId,
        expr: &Expr,
        inventory: &mut JavaSyntaxInventory,
    ) -> Result<(), argus_core::ArgusError> {
        match expr {
            Expr::MethodCall(call) => {
                let receiver = call.receiver.as_ref().map(|r| expr_to_string(r));
                let span = ByteSpan::new(call.span().start() as u64, call.span().end() as u64)?;
                inventory.calls.push(DiscoveredJavaCallSite {
                    caller_target_id: caller.clone(),
                    callee_name: call.method.name.clone(),
                    receiver,
                    span,
                });
                if let Some(ref r) = call.receiver {
                    self.scan_expr_for_calls(caller, r, inventory)?;
                }
                for a in &call.args {
                    self.scan_expr_for_calls(caller, a, inventory)?;
                }
            }
            Expr::Binary(b) => {
                self.scan_expr_for_calls(caller, &b.left, inventory)?;
                self.scan_expr_for_calls(caller, &b.right, inventory)?;
            }
            Expr::Unary(u) => {
                self.scan_expr_for_calls(caller, &u.expr, inventory)?;
            }
            Expr::Assign(a) => {
                if let java_lang::ast::expr::AssignTarget::FieldAccess(f) = &a.target {
                    self.scan_expr_for_calls(caller, &f.target, inventory)?;
                }
                self.scan_expr_for_calls(caller, &a.value, inventory)?;
            }
            Expr::Paren { expr, .. } => {
                self.scan_expr_for_calls(caller, expr, inventory)?;
            }
            Expr::Cast(c) => {
                self.scan_expr_for_calls(caller, &c.expr, inventory)?;
            }
            Expr::NewClass(n) => {
                for a in &n.args {
                    self.scan_expr_for_calls(caller, a, inventory)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn visibility_from_modifiers(modifiers: &[Modifier]) -> TargetVisibility {
    for m in modifiers {
        match m {
            Modifier::Public(_) => return TargetVisibility::Public,
            Modifier::Protected(_) => return TargetVisibility::Restricted,
            Modifier::Private(_) => return TargetVisibility::Private,
            _ => {}
        }
    }
    TargetVisibility::Restricted
}

fn is_static(modifiers: &[Modifier]) -> bool {
    modifiers.iter().any(|m| matches!(m, Modifier::Static(_)))
}

fn is_final(modifiers: &[Modifier]) -> bool {
    modifiers.iter().any(|m| matches!(m, Modifier::Final(_)))
}

fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Primitive(p) => format!("{p:?}").to_lowercase(),
        Type::Reference(r) => match r {
            java_lang::ast::ty::ReferenceType::ClassOrInterfaceType(c) => {
                c.path.segments.iter().map(|s| s.ident.name.as_str()).collect::<Vec<_>>().join(".")
            }
            java_lang::ast::ty::ReferenceType::TypeVar(i) => i.name.clone(),
            java_lang::ast::ty::ReferenceType::Array(a) => format!("{}[]", type_to_string(&a.elem_type)),
        },
        Type::Void(_) => "void".to_string(),
    }
}

fn expr_to_string(expr: &Expr) -> String {
    match expr {
        Expr::Ident(i) => i.name.clone(),
        Expr::FieldAccess(f) => format!("{}.{}", expr_to_string(&f.target), f.field.name),
        Expr::This(_) => "this".to_string(),
        Expr::Super(_) => "super".to_string(),
        _ => String::new(),
    }
}
