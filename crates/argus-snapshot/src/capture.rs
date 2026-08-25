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

use crate::{
    AnalysisConfiguration, CaptureIssue, CaptureIssueKind, FileClass, FileRecord,
    SNAPSHOT_SCHEMA_VERSION, SnapshotManifest, SnapshotRepository, VcsState,
};
use argus_core::{ContentHash, SnapshotId, SourcePath, SourceTreeId};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

#[derive(Clone, Debug)]
pub struct CaptureOptions {
    pub configuration: AnalysisConfiguration,
    pub include_generated: bool,
    pub maximum_file_bytes: u64,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            configuration: AnalysisConfiguration::default_host(),
            include_generated: false,
            maximum_file_bytes: 16 * 1024 * 1024,
        }
    }
}

pub fn capture_snapshot(
    repository_root: &Path,
    state_root: &Path,
    options: &CaptureOptions,
) -> Result<SnapshotManifest, argus_core::ArgusError> {
    let root = repository_root.canonicalize().map_err(|error| {
        argus_core::ArgusError::new(argus_core::ErrorCode::Io, "cannot resolve repository root")
            .with_source(error)
    })?;
    if !root.is_dir() {
        return Err(argus_core::ArgusError::invalid_input(
            "repository root is not a directory",
        ));
    }

    let repository = SnapshotRepository::open(state_root)?;
    let mut files = BTreeMap::new();
    let mut issues = BTreeMap::new();
    walk(&root, &root, options, &repository, &mut files, &mut issues)?;
    let vcs = vcs_state(&root);
    let mut manifest = SnapshotManifest {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        id: SnapshotId::derive([b"pending".as_slice()]),
        source_tree: SourceTreeId::derive([b"pending".as_slice()]),
        configuration: options.configuration.clone(),
        vcs,
        files,
        issues,
    };
    manifest.source_tree = manifest.derive_source_tree_id()?;
    manifest.id = manifest.derive_id()?;
    repository.write_manifest(&manifest)?;
    Ok(manifest)
}

fn walk(
    root: &Path,
    directory: &Path,
    options: &CaptureOptions,
    repository: &SnapshotRepository,
    records: &mut BTreeMap<SourcePath, FileRecord>,
    issues: &mut BTreeMap<SourcePath, CaptureIssue>,
) -> Result<(), argus_core::ArgusError> {
    let mut entries = fs::read_dir(directory)
        .map_err(io_error("cannot read repository directory"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error("cannot read repository entry"))?;
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|_| {
            argus_core::ArgusError::invariant("captured path escaped repository root")
        })?;
        if ignored_directory(relative) {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(io_error("cannot inspect repository entry"))?;
        let source_path = SourcePath::new(path_text(relative))?;
        if file_type.is_symlink() {
            records.insert(
                source_path.clone(),
                FileRecord {
                    path: source_path.clone(),
                    content: None,
                    size: 0,
                    class: FileClass::Unsupported,
                },
            );
            issues.insert(
                source_path.clone(),
                CaptureIssue {
                    path: source_path,
                    kind: CaptureIssueKind::Symlink,
                    detail: "symlink content is not followed during capture".to_owned(),
                },
            );
            continue;
        }
        if file_type.is_dir() {
            if path.join(".git").is_file() {
                record_issue(
                    source_path,
                    0,
                    FileClass::Unsupported,
                    CaptureIssueKind::Submodule,
                    "Git submodule content is not captured as repository-owned source".to_owned(),
                    records,
                    issues,
                );
                continue;
            }
            walk(root, &path, options, repository, records, issues)?;
        } else if file_type.is_file() {
            capture_file(
                &entry,
                relative,
                source_path,
                options,
                repository,
                records,
                issues,
            )?;
        }
    }
    Ok(())
}

fn capture_file(
    entry: &fs::DirEntry,
    relative: &Path,
    source_path: SourcePath,
    options: &CaptureOptions,
    repository: &SnapshotRepository,
    records: &mut BTreeMap<SourcePath, FileRecord>,
    issues: &mut BTreeMap<SourcePath, CaptureIssue>,
) -> Result<(), argus_core::ArgusError> {
    let class = classify(relative);
    if class == FileClass::GeneratedInput && !options.include_generated {
        return Ok(());
    }
    let size = entry
        .metadata()
        .map_err(io_error("cannot inspect repository file"))?
        .len();
    if size > options.maximum_file_bytes {
        record_issue(
            source_path,
            size,
            class,
            CaptureIssueKind::Oversized,
            format!(
                "file exceeds configured {} byte limit",
                options.maximum_file_bytes
            ),
            records,
            issues,
        );
        return Ok(());
    }
    let bytes = match fs::read(entry.path()) {
        Ok(bytes) => bytes,
        Err(error) => {
            record_issue(
                source_path,
                size,
                class,
                CaptureIssueKind::Unreadable,
                error.to_string(),
                records,
                issues,
            );
            return Ok(());
        }
    };
    let content = ContentHash::digest(&bytes);
    repository.write_blob(&content, &bytes)?;
    if class != FileClass::Binary && std::str::from_utf8(&bytes).is_err() {
        issues.insert(
            source_path.clone(),
            CaptureIssue {
                path: source_path.clone(),
                kind: CaptureIssueKind::UnsupportedEncoding,
                detail: "text-classified file is not valid UTF-8".to_owned(),
            },
        );
    }
    records.insert(
        source_path.clone(),
        FileRecord {
            path: source_path,
            content: Some(content),
            size,
            class,
        },
    );
    Ok(())
}

fn record_issue(
    path: SourcePath,
    size: u64,
    class: FileClass,
    kind: CaptureIssueKind,
    detail: String,
    records: &mut BTreeMap<SourcePath, FileRecord>,
    issues: &mut BTreeMap<SourcePath, CaptureIssue>,
) {
    records.insert(
        path.clone(),
        FileRecord {
            path: path.clone(),
            content: None,
            size,
            class,
        },
    );
    issues.insert(path.clone(), CaptureIssue { path, kind, detail });
}

fn ignored_directory(path: &Path) -> bool {
    matches!(
        path.components()
            .next()
            .and_then(|part| part.as_os_str().to_str()),
        Some(".git" | ".argus")
    )
}

fn classify(path: &Path) -> FileClass {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if path
        .components()
        .next()
        .is_some_and(|part| part.as_os_str() == "target")
    {
        FileClass::GeneratedInput
    } else if matches!(
        extension,
        "rs" | "py" | "js" | "ts" | "tsx" | "go" | "java" | "c" | "h" | "cpp"
    ) {
        FileClass::Source
    } else if matches!(name, "Cargo.lock" | "package-lock.json" | "pnpm-lock.yaml") {
        FileClass::Lockfile
    } else if matches!(extension, "toml" | "yaml" | "yml" | "json") {
        FileClass::Configuration
    } else if matches!(extension, "md" | "mdx" | "rst") {
        FileClass::DesignDocument
    } else if path.components().any(|part| part.as_os_str() == "vendor") {
        FileClass::Vendor
    } else if matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "pdf" | "zip" | "exe"
    ) {
        FileClass::Binary
    } else {
        FileClass::Unsupported
    }
}

fn vcs_state(root: &Path) -> VcsState {
    let revision = git(root, &["rev-parse", "HEAD"]).filter(|value| !value.is_empty());
    let dirty = git(root, &["status", "--porcelain", "--untracked-files=all"])
        .is_none_or(|value| !value.is_empty());
    VcsState { revision, dirty }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}
