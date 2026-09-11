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

/// Returns the language specification for Kotlin.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "kotlin",
        display_name: "Kotlin",
        provider_name: "treesitter-kotlin",
        grammar: || tree_sitter_kotlin_ng::LANGUAGE.into(),
        file_extensions: &["kt", "kts"],
        manifest_patterns: &["build.gradle.kts", "build.gradle", "settings.gradle.kts"],
        classify: classify_kotlin,
        extract_name: default_extract_name,
        extract_visibility: extract_kotlin_visibility,
        extract_call_target: extract_kotlin_call_target,
        extract_import_path: extract_kotlin_import_path,
    }
}

fn classify_kotlin(kind: &str) -> Option<NodeClassification> {
    match kind {
        "class_declaration" | "object_declaration" => Some(NodeClassification::Type),
        "function_declaration" | "secondary_constructor" => Some(NodeClassification::Callable),
        "import_header" => Some(NodeClassification::Import),
        "call_expression" => Some(NodeClassification::Call),
        "comment" | "multiline_comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_kotlin_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    let text = node_text(node, source);
    if text.contains("private ") {
        TargetVisibility::Private
    } else if text.contains("protected ") || text.contains("internal ") {
        TargetVisibility::Restricted
    } else {
        TargetVisibility::Public // Default public in Kotlin
    }
}

fn extract_kotlin_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "simple_identifier" || child.kind() == "identifier" {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}

fn extract_kotlin_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(path_node) = node.child_by_field_name("path") {
        let text = node_text(&path_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "identifier" || child.kind() == "scoped_identifier" {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}
