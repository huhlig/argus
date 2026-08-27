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
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    SourceLocation, SourcePath, Target, TargetId, TargetKind, TargetVisibility,
};
use argus_language::SourceAccess;
use ra_ap_syntax::{
    AstNode, Edition, SourceFile,
    ast::{self, HasAttrs, HasDocComments, HasModuleItem, HasName, HasVisibility},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustEdition {
    Edition2015,
    Edition2018,
    Edition2021,
    Edition2024,
}

impl From<RustEdition> for Edition {
    fn from(value: RustEdition) -> Self {
        match value {
            RustEdition::Edition2015 => Self::Edition2015,
            RustEdition::Edition2018 => Self::Edition2018,
            RustEdition::Edition2021 => Self::Edition2021,
            RustEdition::Edition2024 => Self::Edition2024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustSyntaxInventory {
    pub targets: Vec<Target>,
    pub diagnostics: Vec<String>,
    pub documentation: BTreeMap<TargetId, String>,
    pub conditions: BTreeMap<TargetId, Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct RustSyntaxProvider {
    edition: RustEdition,
}

impl RustSyntaxProvider {
    #[must_use]
    pub fn new(_configuration: ConfigurationId, edition: RustEdition) -> Self {
        Self { edition }
    }

    pub fn inventory_file(
        &self,
        source: &dyn SourceAccess,
        path: &SourcePath,
        parent: Option<TargetId>,
    ) -> Result<RustSyntaxInventory, argus_core::ArgusError> {
        let bytes = source.read(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            argus_core::ArgusError::invalid_input("Rust syntax provider requires UTF-8 source")
                .with_source(error)
        })?;
        let parse = SourceFile::parse(text, self.edition.into());
        let diagnostics = parse
            .errors()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let file = parse.tree();
        let file_id = Self::target_id(path, "file", path.as_str());
        let mut targets = vec![Target {
            id: file_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::File,
            },
            visibility: TargetVisibility::NotApplicable,
            name: path.as_str().to_owned(),
            parent,
            location: Some(location(path, file.syntax())?),
            inventory: InventoryState::Represented,
            capabilities: vec![syntax_capability(!diagnostics.is_empty())],
            diagnostic: None,
        }];
        let mut documentation = BTreeMap::new();
        let mut conditions = BTreeMap::new();
        Self::collect_items(
            path,
            file.items(),
            &file_id,
            "",
            &mut targets,
            &mut documentation,
            &mut conditions,
        )?;
        Ok(RustSyntaxInventory {
            targets,
            diagnostics,
            documentation,
            conditions,
        })
    }

    pub fn inventory_crate(
        &self,
        source: &dyn SourceAccess,
        entry: &SourcePath,
        parent: Option<TargetId>,
    ) -> Result<RustSyntaxInventory, argus_core::ArgusError> {
        let mut combined = RustSyntaxInventory {
            targets: Vec::new(),
            diagnostics: Vec::new(),
            documentation: BTreeMap::new(),
            conditions: BTreeMap::new(),
        };
        let mut pending = vec![(entry.clone(), parent)];
        let mut visited = BTreeSet::new();
        while let Some((path, file_parent)) = pending.pop() {
            if !visited.insert(path.clone()) {
                combined.diagnostics.push(format!(
                    "module source referenced more than once: {}",
                    path.as_str()
                ));
                continue;
            }
            let mut inventory = self.inventory_file(source, &path, file_parent)?;
            let modules = self.external_modules(source, &path)?;
            for module in modules {
                match resolve_module_path(source, &path, &module) {
                    Ok(Some(module_path)) => {
                        pending.push((module_path, Some(module.target.clone())));
                    }
                    Ok(None) => mark_unresolved_module(
                        &mut inventory.targets,
                        &module.target,
                        "module source is absent from the immutable snapshot",
                    ),
                    Err(error) => mark_unresolved_module(
                        &mut inventory.targets,
                        &module.target,
                        &format!("invalid module path: {error}"),
                    ),
                }
            }
            combined.targets.append(&mut inventory.targets);
            combined.diagnostics.append(&mut inventory.diagnostics);
            combined.documentation.append(&mut inventory.documentation);
            combined.conditions.append(&mut inventory.conditions);
        }
        combined
            .targets
            .sort_by(|left, right| left.id.cmp(&right.id));
        combined.diagnostics.sort();
        Ok(combined)
    }

    fn external_modules(
        &self,
        source: &dyn SourceAccess,
        path: &SourcePath,
    ) -> Result<Vec<ExternalModule>, argus_core::ArgusError> {
        let bytes = source.read(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            argus_core::ArgusError::invalid_input("Rust syntax provider requires UTF-8 source")
                .with_source(error)
        })?;
        let file = SourceFile::parse(text, self.edition.into()).tree();
        Ok(file
            .items()
            .filter_map(|item| match item {
                ast::Item::Module(module) if module.item_list().is_none() => {
                    let name = module.name()?.text().to_string();
                    let target = Self::target_id(path, "module", &name);
                    Some(ExternalModule {
                        name,
                        target,
                        path_override: module.attrs().find_map(|attr| path_attribute(&attr)),
                    })
                }
                _ => None,
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_items(
        path: &SourcePath,
        items: impl Iterator<Item = ast::Item>,
        parent: &TargetId,
        prefix: &str,
        targets: &mut Vec<Target>,
        documentation: &mut BTreeMap<TargetId, String>,
        conditions: &mut BTreeMap<TargetId, Vec<String>>,
    ) -> Result<(), argus_core::ArgusError> {
        for item in items {
            let (kind, name) = item_identity(&item);
            let qualified = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}::{name}")
            };
            let id = Self::target_id(path, kind_key(&kind), &qualified);
            targets.push(Target {
                id: id.clone(),
                kind,
                visibility: item_visibility(&item),
                name,
                parent: Some(parent.clone()),
                location: Some(location(path, item.syntax())?),
                inventory: InventoryState::Represented,
                capabilities: item_capabilities(&item),
                diagnostic: None,
            });
            if let Some(docs) = match &item {
                ast::Item::Module(inner) => documentation_text(inner),
                ast::Item::Const(inner) => documentation_text(inner),
                ast::Item::Enum(inner) => documentation_text(inner),
                ast::Item::ExternBlock(inner) => documentation_text(inner),
                ast::Item::ExternCrate(inner) => documentation_text(inner),
                ast::Item::Fn(inner) => documentation_text(inner),
                ast::Item::Impl(inner) => documentation_text(inner),
                ast::Item::MacroCall(inner) => documentation_text(inner),
                ast::Item::MacroDef(inner) => documentation_text(inner),
                ast::Item::MacroRules(inner) => documentation_text(inner),
                ast::Item::Static(inner) => documentation_text(inner),
                ast::Item::Struct(inner) => documentation_text(inner),
                ast::Item::Trait(inner) => documentation_text(inner),
                ast::Item::TypeAlias(inner) => documentation_text(inner),
                ast::Item::Union(inner) => documentation_text(inner),
                ast::Item::Use(inner) => documentation_text(inner),
                ast::Item::AsmExpr(_) => None,
            } {
                documentation.insert(id.clone(), docs);
            }

            let predicates = configuration_predicates(&item);
            if !predicates.is_empty() {
                conditions.insert(id.clone(), predicates);
            }
            if let ast::Item::Module(module) = &item
                && let Some(list) = module.item_list()
            {
                Self::collect_items(
                    path,
                    list.items(),
                    &id,
                    &qualified,
                    targets,
                    documentation,
                    conditions,
                )?;
            } else if let ast::Item::Impl(item) = &item
                && let Some(list) = item.assoc_item_list()
            {
                Self::collect_associated_items(
                    path,
                    list.assoc_items(),
                    &id,
                    &qualified,
                    "method",
                    TargetVisibility::Unknown,
                    targets,
                    documentation,
                    conditions,
                )?;
            } else if let ast::Item::Trait(item) = &item
                && let Some(list) = item.assoc_item_list()
            {
                Self::collect_associated_items(
                    path,
                    list.assoc_items(),
                    &id,
                    &qualified,
                    "trait_method",
                    TargetVisibility::Inherited,
                    targets,
                    documentation,
                    conditions,
                )?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_associated_items(
        path: &SourcePath,
        items: impl Iterator<Item = ast::AssocItem>,
        parent: &TargetId,
        prefix: &str,
        callable_kind: &str,
        absent_visibility: TargetVisibility,
        targets: &mut Vec<Target>,
        documentation: &mut BTreeMap<TargetId, String>,
        conditions: &mut BTreeMap<TargetId, Vec<String>>,
    ) -> Result<(), argus_core::ArgusError> {
        for item in items {
            let (kind, name) = associated_item_identity(&item, callable_kind);
            let qualified = format!("{prefix}::{name}");
            let id = Self::target_id(path, kind_key(&kind), &qualified);
            targets.push(Target {
                id: id.clone(),
                kind,
                visibility: associated_item_visibility(&item, absent_visibility),
                name,
                parent: Some(parent.clone()),
                location: Some(location(path, item.syntax())?),
                inventory: InventoryState::Represented,
                capabilities: associated_item_capabilities(&item),
                diagnostic: None,
            });
            if let Some(docs) = documentation_text(&item) {
                documentation.insert(id.clone(), docs);
            }
            let predicates = configuration_predicates(&item);
            if !predicates.is_empty() {
                conditions.insert(id, predicates);
            }
        }
        Ok(())
    }

    fn target_id(path: &SourcePath, kind: &str, name: &str) -> TargetId {
        TargetId::derive([
            b"rust-syntax".as_slice(),
            path.as_str().as_bytes(),
            kind.as_bytes(),
            name.as_bytes(),
        ])
    }
}

#[derive(Clone, Debug)]
struct ExternalModule {
    name: String,
    target: TargetId,
    path_override: Option<String>,
}

fn path_attribute(attr: &ast::Attr) -> Option<String> {
    if attr.simple_name().as_deref() != Some("path") {
        return None;
    }

    attr.meta()
        .and_then(|meta| match meta {
            // Match the KeyValueMeta variant where the `.expr()` method actually lives
            ast::Meta::KeyValueMeta(kv) => kv.expr(),
            _ => None,
        })
        .map(|expr| {
            expr.syntax()
                .text()
                .to_string()
                .trim_matches('"')
                .to_owned()
        })
}

fn resolve_module_path(
    source: &dyn SourceAccess,
    declaring_file: &SourcePath,
    module: &ExternalModule,
) -> Result<Option<SourcePath>, argus_core::ArgusError> {
    let declaring = Path::new(declaring_file.as_str());
    let parent = declaring.parent().unwrap_or_else(|| Path::new(""));
    if let Some(path_override) = &module.path_override {
        let candidate = normalized_join(parent, Path::new(path_override))?;
        return Ok(source.contains(&candidate).then_some(candidate));
    }
    let file_name = declaring.file_name().and_then(|name| name.to_str());
    let module_dir = if matches!(file_name, Some("lib.rs" | "main.rs" | "mod.rs")) {
        parent.to_path_buf()
    } else {
        let stem = declaring
            .file_stem()
            .ok_or_else(|| argus_core::ArgusError::invalid_input("module file has no stem"))?;
        parent.join(stem)
    };
    for relative in [
        PathBuf::from(format!("{}.rs", module.name)),
        PathBuf::from(&module.name).join("mod.rs"),
    ] {
        let candidate = normalized_join(&module_dir, &relative)?;
        if source.contains(&candidate) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn normalized_join(base: &Path, child: &Path) -> Result<SourcePath, argus_core::ArgusError> {
    SourcePath::new(base.join(child).to_string_lossy().replace('\\', "/"))
}

fn mark_unresolved_module(targets: &mut [Target], id: &TargetId, diagnostic: &str) {
    if let Some(target) = targets.iter_mut().find(|target| target.id == *id) {
        target.inventory = InventoryState::Unsupported;
        target.diagnostic = Some(diagnostic.to_owned());
        target.capabilities.push(Capability {
            name: "rust-module-resolution".to_owned(),
            status: CapabilityStatus::Unavailable,
            detail: Some(diagnostic.to_owned()),
            provider: Some("ra_ap_syntax".to_owned()),
        });
    }
}

fn item_identity(item: &ast::Item) -> (TargetKind, String) {
    match item {
        ast::Item::Fn(item) => callable_identity(item, "function"),
        ast::Item::Struct(item) => portable(PortableTargetKind::Type, item.name()),
        ast::Item::Enum(item) => portable(PortableTargetKind::Type, item.name()),
        ast::Item::Union(item) => portable(PortableTargetKind::Type, item.name()),
        ast::Item::Trait(item) => portable(PortableTargetKind::Type, item.name()),
        ast::Item::Const(item) => portable(PortableTargetKind::Constant, item.name()),
        ast::Item::Static(item) => portable(PortableTargetKind::Constant, item.name()),
        ast::Item::Module(item) => portable(PortableTargetKind::Module, item.name()),
        ast::Item::TypeAlias(item) => rust_kind("type_alias", item.name()),
        ast::Item::MacroDef(item) => rust_kind("macro_definition", item.name()),
        ast::Item::MacroRules(item) => rust_kind("macro_rules", item.name()),
        ast::Item::Impl(item) => anonymous("impl", item.syntax()),
        ast::Item::AsmExpr(item) => anonymous("asm_expr", item.syntax()),
        ast::Item::ExternBlock(item) => anonymous("extern_block", item.syntax()),
        ast::Item::ExternCrate(item) => anonymous("extern_crate", item.syntax()),
        ast::Item::MacroCall(item) => anonymous("macro_call", item.syntax()),
        ast::Item::Use(item) => use_identity(item),
    }
}

fn use_identity(item: &ast::Use) -> (TargetKind, String) {
    let kind = if item.visibility().is_some() {
        "reexport"
    } else {
        "import"
    };
    let name = item.use_tree().map_or_else(
        || format!("{kind}@{}", u32::from(item.syntax().text_range().start())),
        |tree| tree.syntax().text().to_string(),
    );
    rust_kind(kind, None).map_name(name)
}

fn associated_item_identity(item: &ast::AssocItem, callable_kind: &str) -> (TargetKind, String) {
    match item {
        ast::AssocItem::Fn(item) => callable_identity(item, callable_kind),
        ast::AssocItem::Const(item) => portable(PortableTargetKind::Constant, item.name()),
        ast::AssocItem::TypeAlias(item) => rust_kind("associated_type", item.name()),
        ast::AssocItem::MacroCall(item) => anonymous("associated_macro_call", item.syntax()),
    }
}

fn callable_identity(item: &ast::Fn, ordinary_kind: &str) -> (TargetKind, String) {
    let is_test = item
        .attrs()
        .any(|attr| attr.simple_name().as_deref() == Some("test"));
    let is_benchmark = item
        .attrs()
        .any(|attr| attr.simple_name().as_deref() == Some("bench"));
    if is_test {
        portable(PortableTargetKind::Test, item.name())
    } else if is_benchmark {
        rust_kind("benchmark", item.name())
    } else if ordinary_kind == "function" {
        portable(PortableTargetKind::Callable, item.name())
    } else {
        rust_kind(ordinary_kind, item.name())
    }
}

fn documentation_text(item: &(impl HasDocComments + HasAttrs)) -> Option<String> {
    let mut lines = item
        .doc_comments()
        .filter_map(|comment| {
            comment
                .doc_comment()
                .map(|(text, _offset)| text.strip_prefix(' ').unwrap_or(text).to_owned())
        })
        .collect::<Vec<_>>();

    lines.extend(item.attrs().filter_map(|attr| {
        if attr.simple_name().as_deref() == Some("doc") {
            attr.meta()
                .and_then(|meta| match meta {
                    ast::Meta::KeyValueMeta(kv) => kv.expr(),
                    _ => None,
                })
                .map(|expr| expr.syntax().text().to_string())
                .map(|text| text.trim_matches('"').to_owned())
        } else {
            None
        }
    }));

    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn item_visibility(item: &ast::Item) -> TargetVisibility {
    declared_visibility(
        item.syntax().children().find_map(ast::Visibility::cast),
        TargetVisibility::Private,
    )
}

fn associated_item_visibility(
    item: &ast::AssocItem,
    absent_visibility: TargetVisibility,
) -> TargetVisibility {
    declared_visibility(
        item.syntax().children().find_map(ast::Visibility::cast),
        absent_visibility,
    )
}

fn declared_visibility(
    visibility: Option<ast::Visibility>,
    absent_visibility: TargetVisibility,
) -> TargetVisibility {
    visibility.map_or_else(
        || absent_visibility,
        |visibility| {
            if visibility.syntax().text() == "pub" {
                TargetVisibility::Public
            } else {
                TargetVisibility::Restricted
            }
        },
    )
}

fn configuration_predicates(item: &impl HasAttrs) -> Vec<String> {
    item.attrs()
        .filter(|attr| matches!(attr.simple_name().as_deref(), Some("cfg" | "cfg_attr")))
        .map(|attr| attr.syntax().text().to_string())
        .collect()
}

fn item_capabilities(item: &ast::Item) -> Vec<Capability> {
    capabilities_for_node(
        !configuration_predicates(item).is_empty(),
        matches!(item, ast::Item::MacroCall(_)),
    )
}

fn associated_item_capabilities(item: &ast::AssocItem) -> Vec<Capability> {
    capabilities_for_node(
        !configuration_predicates(item).is_empty(),
        matches!(item, ast::AssocItem::MacroCall(_)),
    )
}

fn capabilities_for_node(configuration_specific: bool, macro_invocation: bool) -> Vec<Capability> {
    let mut capabilities = vec![syntax_capability(false)];
    if configuration_specific {
        capabilities.push(Capability {
            name: "rust-configuration-resolution".to_owned(),
            status: CapabilityStatus::Partial,
            detail: Some("configuration predicate retained but not compiler-evaluated".to_owned()),
            provider: Some("ra_ap_syntax".to_owned()),
        });
    }
    if macro_invocation {
        capabilities.push(Capability {
            name: "rust-macro-expansion".to_owned(),
            status: CapabilityStatus::Unavailable,
            detail: Some("macro invocation represented without generated expansion".to_owned()),
            provider: Some("ra_ap_syntax".to_owned()),
        });
    }
    capabilities
}

fn portable(kind: PortableTargetKind, name: Option<ast::Name>) -> (TargetKind, String) {
    (
        TargetKind::Portable { kind },
        name.map_or_else(
            || "<missing-name>".to_owned(),
            |name| name.text().to_string(),
        ),
    )
}

fn rust_kind(kind: &str, name: Option<ast::Name>) -> (TargetKind, String) {
    (
        TargetKind::LanguageSpecific {
            language: "rust".to_owned(),
            kind: kind.to_owned(),
        },
        name.map_or_else(
            || "<missing-name>".to_owned(),
            |name| name.text().to_string(),
        ),
    )
}

fn anonymous(kind: &str, node: &ra_ap_syntax::SyntaxNode) -> (TargetKind, String) {
    let start = u32::from(node.text_range().start());
    rust_kind(kind, None).map_name(format!("{kind}@{start}"))
}

trait MapName {
    fn map_name(self, name: String) -> Self;
}

impl MapName for (TargetKind, String) {
    fn map_name(mut self, name: String) -> Self {
        self.1 = name;
        self
    }
}

fn kind_key(kind: &TargetKind) -> &str {
    match kind {
        TargetKind::Portable { kind } => match kind {
            PortableTargetKind::Workspace => "workspace",
            PortableTargetKind::Package => "package",
            PortableTargetKind::Module => "module",
            PortableTargetKind::Type => "type",
            PortableTargetKind::Callable => "callable",
            PortableTargetKind::Constant => "constant",
            PortableTargetKind::Test => "test",
            PortableTargetKind::File => "file",
            _ => "portable",
        },
        TargetKind::LanguageSpecific { kind, .. } => kind,
    }
}

fn location(
    path: &SourcePath,
    node: &ra_ap_syntax::SyntaxNode,
) -> Result<SourceLocation, argus_core::ArgusError> {
    let range = node.text_range();
    Ok(SourceLocation {
        path: path.clone(),
        bytes: ByteSpan::new(
            u64::from(u32::from(range.start())),
            u64::from(u32::from(range.end())),
        )?,
        start: None,
        end: None,
    })
}

fn syntax_capability(partial: bool) -> Capability {
    Capability {
        name: "rust-syntax".to_owned(),
        status: if partial {
            CapabilityStatus::Partial
        } else {
            CapabilityStatus::Complete
        },
        detail: partial.then(|| "source contains recoverable syntax errors".to_owned()),
        provider: Some("ra_ap_syntax".to_owned()),
    }
}
