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

/// Returns the language specification for TypeScript.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "typescript",
        display_name: "TypeScript",
        provider_name: "treesitter-typescript",
        grammar: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        file_extensions: &["ts", "tsx", "mts", "cts"],
        manifest_patterns: &["package.json", "tsconfig.json"],
        classify: classify_typescript,
        extract_name: default_extract_name,
        extract_visibility: extract_typescript_visibility,
        extract_call_target: extract_typescript_call_target,
        extract_import_path: extract_typescript_import_path,
    }
}

fn classify_typescript(kind: &str) -> Option<NodeClassification> {
    match kind {
        "class_declaration"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration" => Some(NodeClassification::Type),
        "function_declaration" | "method_definition" => Some(NodeClassification::Callable),
        "import_statement" => Some(NodeClassification::Import),
        "call_expression" => Some(NodeClassification::Call),
        "comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_typescript_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "accessibility_modifier" {
                let text = node_text(&child, source).trim();
                if text == "private" {
                    return TargetVisibility::Private;
                } else if text == "protected" {
                    return TargetVisibility::Restricted;
                }
                return TargetVisibility::Public;
            }
        }
    }
    TargetVisibility::Public
}

fn extract_typescript_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("function") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_typescript_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(source_node) = node.child_by_field_name("source") {
        let text = node_text(&source_node, source).trim();
        let unquoted = text.trim_matches(['"', '\'']);
        if !unquoted.is_empty() {
            return Some(unquoted.to_owned());
        }
    }
    None
}
