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

use crate::spec::{LanguageSpec, NodeClassification, default_extract_name, node_text};
use argus_core::TargetVisibility;

/// Returns the language specification for C++.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "cpp",
        display_name: "C++",
        provider_name: "treesitter-cpp",
        grammar: || tree_sitter_cpp::LANGUAGE.into(),
        file_extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "h"],
        manifest_patterns: &["CMakeLists.txt", "Makefile", "meson.build"],
        classify: classify_cpp,
        extract_name: extract_cpp_name,
        extract_visibility: extract_cpp_visibility,
        extract_call_target: extract_cpp_call_target,
        extract_import_path: extract_cpp_import_path,
    }
}

fn classify_cpp(kind: &str) -> Option<NodeClassification> {
    match kind {
        "namespace_definition" => Some(NodeClassification::Module),
        "class_specifier" | "struct_specifier" | "union_specifier" | "enum_specifier"
        | "type_definition" | "alias_declaration" => Some(NodeClassification::Type),
        "function_definition" => Some(NodeClassification::Callable),
        "preproc_include" | "using_declaration" => Some(NodeClassification::Import),
        "call_expression" => Some(NodeClassification::Call),
        "comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_cpp_name(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if node.kind() == "function_definition" {
        if let Some(decl) = node.child_by_field_name("declarator") {
            return extract_declarator_name(&decl, source);
        }
    }
    default_extract_name(node, source)
}

fn extract_declarator_name(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(inner) = node.child_by_field_name("declarator") {
        return extract_declarator_name(&inner, source);
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            let kind = child.kind();
            if kind == "identifier"
                || kind == "field_identifier"
                || kind == "qualified_identifier"
                || kind == "destructor_name"
            {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    let text = node_text(node, source).trim();
    let name = text.split('(').next().unwrap_or(text).trim();
    let name = name.rsplit(['*', '&', ' ']).next().unwrap_or(name).trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

fn extract_cpp_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    let text = node_text(node, source);
    if text.starts_with("private:") || text.contains(" private ") {
        TargetVisibility::Private
    } else if text.starts_with("protected:") || text.contains(" protected ") {
        TargetVisibility::Restricted
    } else if text.starts_with("static ") || text.contains(" static ") {
        TargetVisibility::Private
    } else {
        TargetVisibility::Public
    }
}

fn extract_cpp_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("function") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_cpp_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(path_node) = node.child_by_field_name("path") {
        let text = node_text(&path_node, source).trim();
        let unquoted = text.trim_matches(['"', '<', '>']);
        if !unquoted.is_empty() {
            return Some(unquoted.to_owned());
        }
    }
    None
}
