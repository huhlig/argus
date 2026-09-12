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

use argus_core::SourcePath;
use argus_snapshot::{
    CaptureOptions, FileChangeKind, FileDelta, capture_snapshot, git_diff_delta,
};
use std::{fs, process::Command};

#[test]
fn snapshot_diff_identifies_added_modified_and_removed_files() {
    let temporary = tempfile::tempdir().unwrap();
    let repository_root = temporary.path().join("repo");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(repository_root.join("src")).unwrap();

    let lib_rs = SourcePath::new("src/lib.rs").unwrap();
    let cargo_toml = SourcePath::new("Cargo.toml").unwrap();
    let util_rs = SourcePath::new("src/util.rs").unwrap();

    fs::write(
        repository_root.join("src/lib.rs"),
        b"pub fn original() {}\n",
    )
    .unwrap();
    fs::write(
        repository_root.join("Cargo.toml"),
        b"[package]\nname='fixture'\n",
    )
    .unwrap();

    let snapshot_a =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();

    // Now edit lib.rs, delete Cargo.toml, and add util.rs
    fs::write(repository_root.join("src/lib.rs"), b"pub fn modified() {}\n").unwrap();
    fs::remove_file(repository_root.join("Cargo.toml")).unwrap();
    fs::write(repository_root.join("src/util.rs"), b"pub fn helper() {}\n").unwrap();

    let snapshot_b =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();

    let delta = snapshot_b.diff(&snapshot_a);
    assert!(!delta.is_clean());
    assert_eq!(delta.total_changed(), 3);

    assert!(delta.added.contains_key(&util_rs));
    assert!(delta.modified.contains_key(&lib_rs));
    assert!(delta.removed.contains_key(&cargo_toml));

    let changed = delta.changed_paths();
    assert_eq!(changed.len(), 3);
    assert!(changed.contains(&util_rs));
    assert!(changed.contains(&lib_rs));
    assert!(changed.contains(&cargo_toml));

    // Self-diff should be clean
    let self_diff = snapshot_a.diff(&snapshot_a);
    assert!(self_diff.is_clean());
    assert_eq!(self_diff.total_changed(), 0);
    assert_eq!(self_diff.unchanged.len(), 2);
}

#[test]
fn file_delta_operations() {
    let mut delta = FileDelta::default();
    assert!(delta.is_clean());
    assert_eq!(delta.total_changed(), 0);

    let p1 = SourcePath::new("a.rs").unwrap();
    let p2 = SourcePath::new("b.rs").unwrap();
    let p3 = SourcePath::new("c.rs").unwrap();

    delta.added.insert(p1.clone());
    delta.modified.insert(p2.clone());
    delta.removed.insert(p3.clone());

    assert!(!delta.is_clean());
    assert_eq!(delta.total_changed(), 3);

    let paths = delta.changed_paths();
    assert_eq!(paths.len(), 3);
    assert!(paths.contains(&p1));
    assert!(paths.contains(&p2));
    assert!(paths.contains(&p3));

    assert_eq!(FileChangeKind::Added, FileChangeKind::Added);
}

#[test]
fn git_diff_delta_tracks_working_tree_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let repo = temporary.path();

    // Initialize git repo
    let init = Command::new("git")
        .args(["init"])
        .current_dir(repo)
        .output();
    if init.is_err() || !init.unwrap().status.success() {
        // Git not available on system, skip
        return;
    }

    let _ = Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo)
        .output();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo)
        .output();

    fs::write(repo.join("file1.txt"), b"initial 1\n").unwrap();
    fs::write(repo.join("file2.txt"), b"initial 2\n").unwrap();

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .output();
    let _ = Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(repo)
        .output();

    // Modify file1, remove file2, add file3
    fs::write(repo.join("file1.txt"), b"modified 1\n").unwrap();
    fs::remove_file(repo.join("file2.txt")).unwrap();
    fs::write(repo.join("file3.txt"), b"new file 3\n").unwrap();

    let delta = git_diff_delta(repo, None).unwrap();
    assert!(!delta.is_clean());

    let p1 = SourcePath::new("file1.txt").unwrap();
    let p2 = SourcePath::new("file2.txt").unwrap();
    let p3 = SourcePath::new("file3.txt").unwrap();

    assert!(delta.modified.contains(&p1));
    assert!(delta.removed.contains(&p2));
    assert!(delta.added.contains(&p3));
}
