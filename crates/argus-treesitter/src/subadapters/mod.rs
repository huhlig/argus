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

pub mod c;
pub mod c_sharp;
pub mod cpp;
pub mod go;
pub mod haskell;
pub mod java;
pub mod javascript;
pub mod kotlin;
pub mod python;
pub mod rust;
pub mod swift;
pub mod typescript;
pub mod zig;

use crate::spec::LanguageSpec;

/// Returns specifications for all 13 supported Tree-Sitter languages.
#[must_use]
pub fn all_specs() -> Vec<LanguageSpec> {
    vec![
        go::spec(),
        c::spec(),
        cpp::spec(),
        rust::spec(),
        python::spec(),
        java::spec(),
        typescript::spec(),
        javascript::spec(),
        c_sharp::spec(),
        haskell::spec(),
        zig::spec(),
        swift::spec(),
        kotlin::spec(),
    ]
}

/// Finds a language specification by language identifier or alias.
#[must_use]
pub fn spec_for_language(name: &str) -> Option<LanguageSpec> {
    match name.to_ascii_lowercase().as_str() {
        "go" | "golang" => Some(go::spec()),
        "c" => Some(c::spec()),
        "cpp" | "c++" | "cxx" => Some(cpp::spec()),
        "rust" | "rs" => Some(rust::spec()),
        "python" | "py" => Some(python::spec()),
        "java" => Some(java::spec()),
        "typescript" | "ts" => Some(typescript::spec()),
        "javascript" | "js" => Some(javascript::spec()),
        "c_sharp" | "c#" | "csharp" | "cs" => Some(c_sharp::spec()),
        "haskell" | "hs" => Some(haskell::spec()),
        "zig" => Some(zig::spec()),
        "swift" => Some(swift::spec()),
        "kotlin" | "kt" => Some(kotlin::spec()),
        _ => None,
    }
}

/// Finds a language specification by file extension (without leading dot).
#[must_use]
pub fn spec_for_extension(ext: &str) -> Option<LanguageSpec> {
    all_specs().into_iter().find(|s| s.matches_extension(ext))
}

/// Finds a language specification matching a manifest or source filename.
#[must_use]
pub fn spec_for_manifest(filename: &str) -> Option<LanguageSpec> {
    all_specs().into_iter().find(|s| s.matches_manifest(filename))
}
