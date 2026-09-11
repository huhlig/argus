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

use argus_core::{PortableTargetKind, TargetVisibility};

/// High-level semantic classification of a Tree-Sitter AST node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeClassification {
    /// Module, namespace, or package declaration.
    Module,
    /// Type declaration (class, struct, interface, enum, union, typedef, trait).
    Type,
    /// Callable declaration (function, method, constructor, lambda).
    Callable,
    /// Constant, static, or global variable declaration.
    Constant,
    /// Import, use, or include statement.
    Import,
    /// Call expression or invocation site.
    Call,
    /// Comment or doc comment.
    Comment,
}

impl NodeClassification {
    /// Maps this classification to an Argus [`PortableTargetKind`], if applicable.
    #[must_use]
    pub const fn to_portable_kind(self) -> Option<PortableTargetKind> {
        match self {
            Self::Module => Some(PortableTargetKind::Module),
            Self::Type => Some(PortableTargetKind::Type),
            Self::Callable => Some(PortableTargetKind::Callable),
            Self::Constant => Some(PortableTargetKind::Constant),
            Self::Import | Self::Call | Self::Comment => None,
        }
    }
}

/// Language specification defining Tree-Sitter grammar mapping for an Argus subadapter.
pub struct LanguageSpec {
    /// Unique identifier for this language (e.g. "go", "c", "cpp", "rust").
    pub language_id: &'static str,
    /// Human-friendly display name (e.g. "Go", "C++", "Rust").
    pub display_name: &'static str,
    /// Provider name reported in target capabilities (e.g. "treesitter-go").
    pub provider_name: &'static str,
    /// Constructor for the Tree-Sitter [`tree_sitter::Language`] grammar.
    pub grammar: fn() -> tree_sitter::Language,
    /// File extensions associated with this language (lowercase without leading dot).
    pub file_extensions: &'static [&'static str],
    /// Manifest or project file names associated with this language (e.g. "go.mod", "CMakeLists.txt").
    pub manifest_patterns: &'static [&'static str],
    /// Classifies an AST node kind into a high-level [`NodeClassification`].
    pub classify: fn(kind: &str) -> Option<NodeClassification>,
    /// Extracts the symbol name for a definition node.
    pub extract_name: fn(node: &tree_sitter::Node, source: &[u8]) -> Option<String>,
    /// Determines the visibility of a definition node.
    pub extract_visibility: fn(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility,
    /// Extracts the target/callee name from a call expression node.
    pub extract_call_target: fn(node: &tree_sitter::Node, source: &[u8]) -> Option<String>,
    /// Extracts the imported module/path string from an import node.
    pub extract_import_path: fn(node: &tree_sitter::Node, source: &[u8]) -> Option<String>,
}

impl LanguageSpec {
    /// Checks if a file extension belongs to this language.
    #[must_use]
    pub fn matches_extension(&self, ext: &str) -> bool {
        self.file_extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext))
    }

    /// Checks if a filename matches any manifest patterns for this language.
    #[must_use]
    pub fn matches_manifest(&self, filename: &str) -> bool {
        self.manifest_patterns
            .iter()
            .any(|p| p.eq_ignore_ascii_case(filename))
    }
}

/// Helper to extract utf-8 text from a node's byte range.
#[must_use]
pub fn node_text<'a>(node: &tree_sitter::Node, source: &'a [u8]) -> &'a str {
    let start = node.start_byte();
    let end = node.end_byte().min(source.len());
    if start <= end {
        std::str::from_utf8(&source[start..end]).unwrap_or("")
    } else {
        ""
    }
}

/// Default helper to find a name from a "name" child field, or the first identifier child.
#[must_use]
pub fn default_extract_name(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(name_node) = node.child_by_field_name("name") {
        let text = node_text(&name_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    if let Some(decl_node) = node.child_by_field_name("declarator") {
        if let Some(inner) = decl_node.child_by_field_name("declarator") {
            let text = node_text(&inner, source).trim();
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
        let text = node_text(&decl_node, source).trim();
        if !text.is_empty() {
            let ident = text.split('(').next().unwrap_or(text).trim();
            let ident = ident.rsplit(['*', '&', ' ']).next().unwrap_or(ident).trim();
            if !ident.is_empty() {
                return Some(ident.to_owned());
            }
        }
    }
    // Search direct children for identifier
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            let kind = child.kind();
            if kind == "identifier"
                || kind == "type_identifier"
                || kind == "field_identifier"
                || kind == "property_identifier"
            {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}
