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

/// Returns the language specification for JavaScript.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "javascript",
        display_name: "JavaScript",
        provider_name: "treesitter-javascript",
        grammar: || tree_sitter_javascript::LANGUAGE.into(),
        file_extensions: &["js", "jsx", "mjs", "cjs"],
        manifest_patterns: &["package.json", "jsconfig.json"],
        classify: classify_javascript,
        extract_name: default_extract_name,
        extract_visibility: |_node, _source| TargetVisibility::Public,
        extract_call_target: extract_js_call_target,
        extract_import_path: extract_js_import_path,
    }
}

fn classify_javascript(kind: &str) -> Option<NodeClassification> {
    match kind {
        "class_declaration" => Some(NodeClassification::Type),
        "function_declaration" | "method_definition" => Some(NodeClassification::Callable),
        "import_statement" => Some(NodeClassification::Import),
        "call_expression" => Some(NodeClassification::Call),
        "comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_js_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("function") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_js_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(source_node) = node.child_by_field_name("source") {
        let text = node_text(&source_node, source).trim();
        let unquoted = text.trim_matches(['"', '\'']);
        if !unquoted.is_empty() {
            return Some(unquoted.to_owned());
        }
    }
    None
}
