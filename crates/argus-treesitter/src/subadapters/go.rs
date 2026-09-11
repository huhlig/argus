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

/// Returns the language specification for Go.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "go",
        display_name: "Go",
        provider_name: "treesitter-go",
        grammar: || tree_sitter_go::LANGUAGE.into(),
        file_extensions: &["go"],
        manifest_patterns: &["go.mod", "go.sum", "go.work"],
        classify: classify_go,
        extract_name: extract_go_name,
        extract_visibility: extract_go_visibility,
        extract_call_target: extract_go_call_target,
        extract_import_path: extract_go_import_path,
    }
}

fn classify_go(kind: &str) -> Option<NodeClassification> {
    match kind {
        "package_clause" => Some(NodeClassification::Module),
        "type_spec" | "struct_type" | "interface_type" => Some(NodeClassification::Type),
        "function_declaration" | "method_declaration" => Some(NodeClassification::Callable),
        "const_spec" | "var_spec" => Some(NodeClassification::Constant),
        "import_spec" => Some(NodeClassification::Import),
        "call_expression" => Some(NodeClassification::Call),
        "comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_go_name(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if node.kind() == "package_clause" {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == "package_identifier" {
                    let text = node_text(&child, source).trim();
                    if !text.is_empty() {
                        return Some(text.to_owned());
                    }
                }
            }
        }
    }
    default_extract_name(node, source)
}

fn extract_go_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    if let Some(name) = extract_go_name(node, source) {
        if name.chars().next().is_some_and(char::is_uppercase) {
            TargetVisibility::Public
        } else {
            TargetVisibility::Private
        }
    } else {
        TargetVisibility::NotApplicable
    }
}

fn extract_go_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("function") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_go_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(path_node) = node.child_by_field_name("path") {
        let text = node_text(&path_node, source).trim();
        let unquoted = text.trim_matches('"');
        if !unquoted.is_empty() {
            return Some(unquoted.to_owned());
        }
    }
    None
}
