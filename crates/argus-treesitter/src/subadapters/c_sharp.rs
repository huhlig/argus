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

/// Returns the language specification for C#.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "c_sharp",
        display_name: "C#",
        provider_name: "treesitter-c_sharp",
        grammar: || tree_sitter_c_sharp::LANGUAGE.into(),
        file_extensions: &["cs"],
        manifest_patterns: &[".csproj", ".sln"],
        classify: classify_c_sharp,
        extract_name: default_extract_name,
        extract_visibility: extract_c_sharp_visibility,
        extract_call_target: extract_c_sharp_call_target,
        extract_import_path: extract_c_sharp_import_path,
    }
}

fn classify_c_sharp(kind: &str) -> Option<NodeClassification> {
    match kind {
        "namespace_declaration" | "file_scoped_namespace_declaration" => {
            Some(NodeClassification::Module)
        }
        "class_declaration"
        | "struct_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration" => Some(NodeClassification::Type),
        "method_declaration" | "constructor_declaration" | "local_function_statement" => {
            Some(NodeClassification::Callable)
        }
        "using_directive" => Some(NodeClassification::Import),
        "invocation_expression" => Some(NodeClassification::Call),
        "comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_c_sharp_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "modifier" {
                let text = node_text(&child, source).trim();
                if text == "public" {
                    return TargetVisibility::Public;
                } else if text == "private" {
                    return TargetVisibility::Private;
                } else if text == "protected" || text == "internal" {
                    return TargetVisibility::Restricted;
                }
            }
        }
    }
    TargetVisibility::Restricted // Default internal in C#
}

fn extract_c_sharp_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("expression") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_c_sharp_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(name_node) = node.child_by_field_name("name") {
        let text = node_text(&name_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}
