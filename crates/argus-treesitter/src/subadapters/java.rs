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

/// Returns the language specification for Java.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "java",
        display_name: "Java",
        provider_name: "treesitter-java",
        grammar: || tree_sitter_java::LANGUAGE.into(),
        file_extensions: &["java"],
        manifest_patterns: &[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        classify: classify_java,
        extract_name: default_extract_name,
        extract_visibility: extract_java_visibility,
        extract_call_target: extract_java_call_target,
        extract_import_path: extract_java_import_path,
    }
}

fn classify_java(kind: &str) -> Option<NodeClassification> {
    match kind {
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration"
        | "annotation_type_declaration" => Some(NodeClassification::Type),
        "method_declaration" | "constructor_declaration" => Some(NodeClassification::Callable),
        "constant_declaration" => Some(NodeClassification::Constant),
        "import_declaration" => Some(NodeClassification::Import),
        "method_invocation" => Some(NodeClassification::Call),
        "line_comment" | "block_comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_java_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    if let Some(mods) = node.child_by_field_name("modifiers") {
        let text = node_text(&mods, source);
        if text.contains("public") {
            return TargetVisibility::Public;
        } else if text.contains("private") {
            return TargetVisibility::Private;
        } else if text.contains("protected") {
            return TargetVisibility::Restricted;
        }
    }
    TargetVisibility::Restricted // Package-private
}

fn extract_java_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(name_node) = node.child_by_field_name("name") {
        let text = node_text(&name_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_java_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}
