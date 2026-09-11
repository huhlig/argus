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

/// Returns the language specification for Swift.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "swift",
        display_name: "Swift",
        provider_name: "treesitter-swift",
        grammar: || tree_sitter_swift::LANGUAGE.into(),
        file_extensions: &["swift"],
        manifest_patterns: &["Package.swift"],
        classify: classify_swift,
        extract_name: default_extract_name,
        extract_visibility: extract_swift_visibility,
        extract_call_target: extract_swift_call_target,
        extract_import_path: extract_swift_import_path,
    }
}

fn classify_swift(kind: &str) -> Option<NodeClassification> {
    match kind {
        "class_declaration"
        | "struct_declaration"
        | "enum_declaration"
        | "protocol_declaration"
        | "actor_declaration" => Some(NodeClassification::Type),
        "function_declaration" | "init_declaration" | "deinit_declaration" => {
            Some(NodeClassification::Callable)
        }
        "import_declaration" => Some(NodeClassification::Import),
        "call_expression" => Some(NodeClassification::Call),
        "comment" | "multiline_comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_swift_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    let text = node_text(node, source);
    if text.contains("public ") || text.contains("open ") {
        TargetVisibility::Public
    } else if text.contains("private ") || text.contains("fileprivate ") {
        TargetVisibility::Private
    } else {
        TargetVisibility::Restricted // Default internal in Swift
    }
}

fn extract_swift_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
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

fn extract_swift_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "identifier" || child.kind() == "simple_identifier" {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}
