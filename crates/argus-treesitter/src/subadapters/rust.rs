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

/// Returns the language specification for Rust.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "rust",
        display_name: "Rust",
        provider_name: "treesitter-rust",
        grammar: || tree_sitter_rust::LANGUAGE.into(),
        file_extensions: &["rs"],
        manifest_patterns: &["Cargo.toml", "Cargo.lock"],
        classify: classify_rust,
        extract_name: default_extract_name,
        extract_visibility: extract_rust_visibility,
        extract_call_target: extract_rust_call_target,
        extract_import_path: extract_rust_import_path,
    }
}

fn classify_rust(kind: &str) -> Option<NodeClassification> {
    match kind {
        "mod_item" => Some(NodeClassification::Module),
        "struct_item" | "enum_item" | "union_item" | "trait_item" | "type_item" => {
            Some(NodeClassification::Type)
        }
        "function_item" => Some(NodeClassification::Callable),
        "const_item" | "static_item" => Some(NodeClassification::Constant),
        "use_declaration" => Some(NodeClassification::Import),
        "call_expression" => Some(NodeClassification::Call),
        "line_comment" | "block_comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_rust_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "visibility_modifier" {
                let text = node_text(&child, source).trim();
                if text == "pub" {
                    return TargetVisibility::Public;
                }
                return TargetVisibility::Restricted;
            }
        }
    }
    TargetVisibility::Private
}

fn extract_rust_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("function") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_rust_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(arg) = node.child_by_field_name("argument") {
        let text = node_text(&arg, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}
