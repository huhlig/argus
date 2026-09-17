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

use argus_core::ArgusError;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// Supported language or tooling ecosystems for project root discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Rust,
    TypeScript,
    Python,
    Java,
    C,
    Go,
}

impl Ecosystem {
    /// Returns default ignored patterns relative to this ecosystem's project root.
    #[must_use]
    pub fn default_ignore_patterns(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["target/"],
            Self::TypeScript => &["node_modules/", "dist/", ".next/", "build/", "coverage/"],
            Self::Python => &["__pycache__/", ".venv/", "venv/", ".pytest_cache/"],
            Self::Java => &["target/", "build/", ".gradle/"],
            Self::C => &["build/", "cmake-build-*/"],
            Self::Go => &[],
        }
    }
}

/// A discovered or configured project root within a repository.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct DiscoveredRoot {
    /// Relative path from repository root (empty path `""` represents the repository root itself).
    pub relative_path: PathBuf,
    /// Ecosystem identified for this root.
    pub ecosystem: Ecosystem,
}

/// Options configuring ignore evaluation and root discovery.
#[derive(Clone, Debug)]
pub struct IgnoreOptions {
    /// Whether to discover project roots and apply their ecosystem-specific default ignores.
    pub use_ecosystem_defaults: bool,
    /// Whether to honor `.gitignore` files found in the repository.
    pub use_gitignore: bool,
    /// Whether to honor `.argusignore` files found in the repository.
    pub use_argusignore: bool,
    /// Additional explicit ignore patterns (standard gitignore format).
    pub additional_patterns: Vec<String>,
    /// Explicit project roots overriding or supplementing auto-discovery.
    pub explicit_roots: Vec<DiscoveredRoot>,
}

impl Default for IgnoreOptions {
    fn default() -> Self {
        Self {
            use_ecosystem_defaults: true,
            use_gitignore: true,
            use_argusignore: true,
            additional_patterns: Vec::new(),
            explicit_roots: Vec::new(),
        }
    }
}

/// Ignore evaluator that combines base invariants, ecosystem defaults, gitignore, and argusignore.
#[derive(Debug)]
pub struct SnapshotIgnore {
    matcher: Gitignore,
    discovered_roots: Vec<DiscoveredRoot>,
}

impl SnapshotIgnore {
    /// Initializes and builds a [`SnapshotIgnore`] for the given repository root.
    pub fn new(repository_root: &Path, options: &IgnoreOptions) -> Result<Self, ArgusError> {
        let mut builder = GitignoreBuilder::new(repository_root);

        // 1. Base invariants: always ignore version control and argus state directories
        builder
            .add_line(None, ".git/")
            .map_err(|e| ArgusError::invariant(format!("invalid base ignore: {e}")))?;
        builder
            .add_line(None, ".argus/")
            .map_err(|e| ArgusError::invariant(format!("invalid base ignore: {e}")))?;

        // 2. Discover project roots (or use explicit roots)
        let mut roots = if options.explicit_roots.is_empty() {
            discover_roots(repository_root)
        } else {
            options.explicit_roots.clone()
        };
        roots.sort();
        roots.dedup();

        // 3. Apply scoped ecosystem defaults if enabled
        if options.use_ecosystem_defaults {
            for root in &roots {
                let base = if root.relative_path.as_os_str().is_empty() {
                    None
                } else {
                    Some(root.relative_path.clone())
                };
                for pattern in root.ecosystem.default_ignore_patterns() {
                    let _ = builder.add_line(base.clone(), pattern);
                }
            }
        }

        // 4. Honor .gitignore files hierarchically if enabled
        if options.use_gitignore {
            add_ignore_files(repository_root, ".gitignore", &mut builder);
        }

        // 5. Additional explicit patterns
        for pattern in &options.additional_patterns {
            let _ = builder.add_line(None, pattern);
        }

        // 6. Honor .argusignore files hierarchically if enabled (highest user priority)
        if options.use_argusignore {
            add_ignore_files(repository_root, ".argusignore", &mut builder);
        }

        let matcher = builder
            .build()
            .map_err(|e| ArgusError::invalid_input(format!("failed to build ignore matcher: {e}")))?;

        Ok(Self {
            matcher,
            discovered_roots: roots,
        })
    }

    /// Discovered or configured project roots.
    #[must_use]
    pub fn roots(&self) -> &[DiscoveredRoot] {
        &self.discovered_roots
    }

    /// Checks if a path relative to repository root is ignored.
    #[must_use]
    pub fn is_ignored(&self, relative_path: &Path, is_dir: bool) -> bool {
        // First check directory components for .git or .argus
        for component in relative_path.components() {
            let s = component.as_os_str().to_string_lossy();
            if s == ".git" || s == ".argus" {
                return true;
            }
        }

        // Use gitignore matcher (checks the path and any parent directory matches)
        match self.matcher.matched_path_or_any_parents(relative_path, is_dir) {
            ignore::Match::Ignore(_) => true,
            ignore::Match::Whitelist(_) | ignore::Match::None => false,
        }
    }
}

/// Recursively discovers project roots by looking for ecosystem marker files.
/// Avoids descending into known build or VCS directories.
#[must_use]
pub fn discover_roots(repository_root: &Path) -> Vec<DiscoveredRoot> {
    let mut roots = Vec::new();
    scan_dir(repository_root, Path::new(""), &mut roots, 0);
    roots
}

fn scan_dir(
    repository_root: &Path,
    relative_dir: &Path,
    roots: &mut Vec<DiscoveredRoot>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let current_dir = if relative_dir.as_os_str().is_empty() {
        repository_root.to_path_buf()
    } else {
        repository_root.join(relative_dir)
    };

    let entries = match fs::read_dir(&current_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let mut filenames = BTreeSet::new();
    let mut subdirs = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" || name_str == "build" {
            continue;
        }
        if let Ok(file_type) = entry.file_type() {
            if file_type.is_dir() {
                subdirs.push(name);
            } else if file_type.is_file() {
                filenames.insert(name_str.to_string());
            }
        }
    }

    // Check for ecosystem markers in current directory
    if filenames.contains("Cargo.toml") {
        roots.push(DiscoveredRoot {
            relative_path: relative_dir.to_path_buf(),
            ecosystem: Ecosystem::Rust,
        });
    }
    if filenames.contains("package.json") || filenames.contains("tsconfig.json") {
        roots.push(DiscoveredRoot {
            relative_path: relative_dir.to_path_buf(),
            ecosystem: Ecosystem::TypeScript,
        });
    }
    if filenames.contains("pyproject.toml")
        || filenames.contains("setup.py")
        || filenames.contains("setup.cfg")
        || filenames.contains("requirements.txt")
    {
        roots.push(DiscoveredRoot {
            relative_path: relative_dir.to_path_buf(),
            ecosystem: Ecosystem::Python,
        });
    }
    if filenames.contains("pom.xml")
        || filenames.contains("build.gradle")
        || filenames.contains("build.gradle.kts")
    {
        roots.push(DiscoveredRoot {
            relative_path: relative_dir.to_path_buf(),
            ecosystem: Ecosystem::Java,
        });
    }
    if filenames.contains("CMakeLists.txt") || filenames.contains("meson.build") {
        roots.push(DiscoveredRoot {
            relative_path: relative_dir.to_path_buf(),
            ecosystem: Ecosystem::C,
        });
    }
    if filenames.contains("go.mod") {
        roots.push(DiscoveredRoot {
            relative_path: relative_dir.to_path_buf(),
            ecosystem: Ecosystem::Go,
        });
    }

    // Recurse into subdirectories
    for subdir in subdirs {
        let next_rel = if relative_dir.as_os_str().is_empty() {
            PathBuf::from(&subdir)
        } else {
            relative_dir.join(&subdir)
        };
        scan_dir(repository_root, &next_rel, roots, depth + 1);
    }
}

/// Recursively searches for ignore files with the given filename and adds them to builder.
fn add_ignore_files(repository_root: &Path, filename: &str, builder: &mut GitignoreBuilder) {
    let mut dirs_to_visit = vec![PathBuf::new()];
    while let Some(rel_dir) = dirs_to_visit.pop() {
        let abs_dir = if rel_dir.as_os_str().is_empty() {
            repository_root.to_path_buf()
        } else {
            repository_root.join(&rel_dir)
        };
        let candidate = abs_dir.join(filename);
        if candidate.is_file() {
            let _ = builder.add(&candidate);
        }
        if let Ok(entries) = fs::read_dir(&abs_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s == ".git" || s == ".argus" || s == "node_modules" || s == "target" {
                    continue;
                }
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        dirs_to_visit.push(if rel_dir.as_os_str().is_empty() {
                            PathBuf::from(name)
                        } else {
                            rel_dir.join(name)
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discovers_roots_in_hybrid_monorepo() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        // Rust at root
        fs::write(root.join("Cargo.toml"), "[package]\nname=\"test\"\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();

        // TypeScript in console/
        fs::create_dir_all(root.join("console/src")).unwrap();
        fs::write(
            root.join("console/package.json"),
            "{\"name\":\"console\"}\n",
        )
        .unwrap();

        let roots = discover_roots(root);
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().any(|r| r.relative_path == Path::new("") && r.ecosystem == Ecosystem::Rust));
        assert!(roots.iter().any(|r| r.relative_path == Path::new("console") && r.ecosystem == Ecosystem::TypeScript));
    }

    #[test]
    fn scopes_ecosystem_defaults_correctly() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        // Rust at root, TS in console/
        fs::write(root.join("Cargo.toml"), "[package]\nname=\"test\"\n").unwrap();
        fs::create_dir_all(root.join("console")).unwrap();
        fs::write(
            root.join("console/package.json"),
            "{\"name\":\"console\"}\n",
        )
        .unwrap();

        let ignore = SnapshotIgnore::new(root, &IgnoreOptions::default()).unwrap();

        // target/ at root should be ignored (Rust default)
        assert!(ignore.is_ignored(Path::new("target"), true));
        assert!(ignore.is_ignored(Path::new("target/debug/test"), false));

        // console/node_modules should be ignored (TypeScript default)
        assert!(ignore.is_ignored(Path::new("console/node_modules"), true));
        assert!(ignore.is_ignored(Path::new("console/node_modules/pkg/index.js"), false));

        // console/src should NOT be ignored
        assert!(!ignore.is_ignored(Path::new("console/src"), true));
        assert!(!ignore.is_ignored(Path::new("console/src/index.ts"), false));

        // src at root should NOT be ignored
        assert!(!ignore.is_ignored(Path::new("src/main.rs"), false));
    }

    #[test]
    fn argusignore_overrides_defaults_with_negation() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        fs::write(root.join("Cargo.toml"), "[package]\nname=\"test\"\n").unwrap();
        fs::create_dir_all(root.join("console/node_modules/special")).unwrap();
        fs::write(
            root.join("console/package.json"),
            "{\"name\":\"console\"}\n",
        )
        .unwrap();

        // .argusignore un-ignores a specific package
        fs::write(
            root.join(".argusignore"),
            "!console/node_modules/special/**\n",
        )
        .unwrap();

        let ignore = SnapshotIgnore::new(root, &IgnoreOptions::default()).unwrap();

        // Regular node_modules is ignored
        assert!(ignore.is_ignored(Path::new("console/node_modules/other/index.js"), false));
        // Whitelisted package is NOT ignored
        assert!(!ignore.is_ignored(Path::new("console/node_modules/special/index.js"), false));
    }
}
