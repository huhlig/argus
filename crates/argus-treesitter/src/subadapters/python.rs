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

/// Returns the language specification for Python.
#[must_use]
pub fn spec() -> LanguageSpec {
    LanguageSpec {
        language_id: "python",
        display_name: "Python",
        provider_name: "treesitter-python",
        grammar: || tree_sitter_python::LANGUAGE.into(),
        file_extensions: &["py", "pyi"],
        manifest_patterns: &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
            "Pipfile",
        ],
        classify: classify_python,
        extract_name: default_extract_name,
        extract_visibility: extract_python_visibility,
        extract_call_target: extract_python_call_target,
        extract_import_path: extract_python_import_path,
    }
}

fn classify_python(kind: &str) -> Option<NodeClassification> {
    match kind {
        "class_definition" => Some(NodeClassification::Type),
        "function_definition" => Some(NodeClassification::Callable),
        "import_statement" | "import_from_statement" => Some(NodeClassification::Import),
        "call" => Some(NodeClassification::Call),
        "comment" => Some(NodeClassification::Comment),
        _ => None,
    }
}

fn extract_python_visibility(node: &tree_sitter::Node, source: &[u8]) -> TargetVisibility {
    if let Some(name) = default_extract_name(node, source) {
        if name.starts_with("__") && !name.ends_with("__") {
            TargetVisibility::Private
        } else if name.starts_with('_') && !name.starts_with("__") {
            TargetVisibility::Restricted
        } else {
            TargetVisibility::Public
        }
    } else {
        TargetVisibility::NotApplicable
    }
}

fn extract_python_call_target(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(func_node) = node.child_by_field_name("function") {
        let text = node_text(&func_node, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn extract_python_import_path(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    if let Some(mod_name) = node.child_by_field_name("module_name") {
        let text = node_text(&mod_name, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    if let Some(name) = node.child_by_field_name("name") {
        let text = node_text(&name, source).trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}
