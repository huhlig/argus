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

/// Returns the language specification for Zig.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "zig",
        display_name: "Zig",
        provider_name: "treesitter-zig",
        grammar: || tree_sitter_zig::LANGUAGE.into(),
        file_extensions: &["zig", "zon"],
        manifest_patterns: &["build.zig", "build.zig.zon"],
        classify: classify_zig,
        extract_name: default_extract_name,
        extract_visibility: extract_zig_visibility,
        extract_call_target: extract_zig_call_target,
        extract_import_path: extract_zig_import_path,
    }
}

fn classify_zig(kind: &str) -> Option<NodeClassification> {
    match kind {
        "ContainerDecl" | "container_declaration" | "struct_declaration" | "enum_declaration"
        | "union_declaration" => Some(NodeClassification::Type),
        "FnProto" | "fn_proto" | "fn_decl" => Some(NodeClassification::Callable),
        "CallExpr" | "call_expr" => Some(NodeClassification::Call),
        "comment" | "line_comment" | "doc_comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_zig_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    let text = node_text(node, source);
    if text.starts_with("pub ") || text.contains(" pub ") {
        TargetVisibility::Public
    } else {
        TargetVisibility::Private
    }
}

fn extract_zig_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("function") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_zig_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    if let Some(start) = text.find("@import(\"") {
        let rest = &text[start + 9..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_owned());
        }
    }
    None
}
