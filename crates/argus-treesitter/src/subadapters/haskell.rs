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

/// Returns the language specification for Haskell.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "haskell",
        display_name: "Haskell",
        provider_name: "treesitter-haskell",
        grammar: || tree_sitter_haskell::LANGUAGE.into(),
        file_extensions: &["hs", "lhs"],
        manifest_patterns: &["package.yaml", "stack.yaml", ".cabal"],
        classify: classify_haskell,
        extract_name: default_extract_name,
        extract_visibility: |_node, _source| TargetVisibility::Public,
        extract_call_target: extract_haskell_call_target,
        extract_import_path: extract_haskell_import_path,
    }
}

fn classify_haskell(kind: &str) -> Option<NodeClassification> {
    match kind {
        "module" => Some(NodeClassification::Module),
        "data_type" | "newtype" | "type_alias" | "class" | "instance" => {
            Some(NodeClassification::Type)
        }
        "function" | "bind" => Some(NodeClassification::Callable),
        "import" => Some(NodeClassification::Import),
        "apply" => Some(NodeClassification::Call),
        "comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_haskell_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "variable" || child.kind() == "identifier" {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}

fn extract_haskell_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "module_id" || child.kind() == "conid" {
                let text = node_text(&child, source).trim();
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}
