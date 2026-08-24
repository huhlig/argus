use argus_core::{ByteSpan, SourcePath};
use argus_snapshot::{
    AnalysisConfiguration, CaptureIssueKind, CaptureOptions, CompilerInput, DriftKind,
    EnvironmentInput, FileClass, SnapshotRepository, capture_snapshot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

#[test]
fn captured_source_survives_edit_delete_and_rename() {
    let temporary = tempfile::tempdir().unwrap();
    let repository_root = temporary.path().join("repo");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(repository_root.join("src")).unwrap();
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

    let manifest =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();
    let original_path = SourcePath::new("src/lib.rs").unwrap();
    assert_eq!(manifest.files[&original_path].class, FileClass::Source);

    fs::write(repository_root.join("src/lib.rs"), b"changed\n").unwrap();
    fs::rename(
        repository_root.join("src/lib.rs"),
        repository_root.join("src/renamed.rs"),
    )
    .unwrap();

    let store = SnapshotRepository::open(&state_root).unwrap();
    let loaded = store.load_manifest(&manifest.id).unwrap();
    let reader = store.reader(loaded);
    assert_eq!(
        reader.read(&original_path).unwrap(),
        b"pub fn original() {}\n"
    );
    assert_eq!(
        reader
            .read_range(&original_path, ByteSpan::new(7, 15).unwrap())
            .unwrap(),
        b"original"
    );
    assert_eq!(
        reader.read_text(&original_path).unwrap(),
        "pub fn original() {}\n"
    );
    let index = reader.line_index(&original_path).unwrap();
    assert_eq!(index.line_count(), 2);
    assert_eq!(index.line_start(1), Some(21));

    let drift = reader.detect_drift(&repository_root);
    assert!(
        drift
            .records
            .iter()
            .any(|record| { record.path == original_path && record.kind == DriftKind::Missing })
    );
}

#[test]
fn identical_declared_inputs_have_deterministic_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let repository_root = temporary.path().join("repo");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(&repository_root).unwrap();
    fs::write(repository_root.join("main.rs"), b"fn main() {}\n").unwrap();

    let first =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();
    let second =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first, second);
}

#[test]
fn paths_cannot_escape_repository_namespace() {
    assert!(SourcePath::new("../secret.rs").is_err());
    assert!(SourcePath::new("src/../../secret.rs").is_err());
    assert!(SourcePath::new("/absolute.rs").is_err());
}

#[test]
fn oversized_files_are_accounted_for_without_being_readable() {
    let temporary = tempfile::tempdir().unwrap();
    let repository_root = temporary.path().join("repo");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(&repository_root).unwrap();
    fs::write(repository_root.join("large.rs"), b"12345").unwrap();
    let options = CaptureOptions {
        maximum_file_bytes: 4,
        ..CaptureOptions::default()
    };

    let manifest = capture_snapshot(&repository_root, &state_root, &options).unwrap();
    let path = SourcePath::new("large.rs").unwrap();
    assert_eq!(manifest.issues[&path].kind, CaptureIssueKind::Oversized);
    assert!(manifest.files[&path].content.is_none());
    let reader = SnapshotRepository::open(&state_root)
        .unwrap()
        .reader(manifest);
    assert!(reader.read(&path).is_err());
}

#[test]
fn non_utf8_text_is_preserved_and_reported() {
    let temporary = tempfile::tempdir().unwrap();
    let repository_root = temporary.path().join("repo");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(&repository_root).unwrap();
    fs::write(repository_root.join("invalid.rs"), [0xff, 0xfe]).unwrap();

    let manifest =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();
    let path = SourcePath::new("invalid.rs").unwrap();
    assert_eq!(
        manifest.issues[&path].kind,
        CaptureIssueKind::UnsupportedEncoding
    );
    let reader = SnapshotRepository::open(&state_root)
        .unwrap()
        .reader(manifest);
    assert_eq!(reader.read(&path).unwrap(), [0xff, 0xfe]);
    assert!(reader.read_text(&path).is_err());
}

#[test]
fn analysis_inputs_change_configuration_and_snapshot_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let repository_root = temporary.path().join("repo");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(&repository_root).unwrap();
    fs::write(repository_root.join("lib.rs"), b"pub fn item() {}\n").unwrap();

    let baseline =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();
    let mut environment = BTreeMap::new();
    environment.insert(
        "RUSTFLAGS".to_owned(),
        EnvironmentInput::new("RUSTFLAGS", b"--cfg special"),
    );
    let configured = CaptureOptions {
        configuration: AnalysisConfiguration::new(
            Some("x86_64-unknown-linux-gnu".to_owned()),
            "release",
            BTreeSet::from(["special".to_owned()]),
            BTreeSet::new(),
            environment,
        ),
        ..CaptureOptions::default()
    };
    let changed = capture_snapshot(&repository_root, &state_root, &configured).unwrap();

    assert_ne!(baseline.configuration.id, changed.configuration.id);
    assert_ne!(baseline.id, changed.id);
    assert!(changed.configuration.has_valid_identity());
    assert!(changed.validate_identity().is_ok());
}

#[test]
fn compiler_identity_is_part_of_analysis_configuration() {
    let baseline = AnalysisConfiguration::default_host();
    let configured = baseline.clone().with_compiler(CompilerInput {
        implementation: "rustc".to_owned(),
        version: "1.85.0".to_owned(),
        commit_hash: Some("fixture-commit".to_owned()),
        host: "x86_64-pc-windows-msvc".to_owned(),
    });

    assert_ne!(baseline.id, configured.id);
    assert!(configured.has_valid_identity());
    let mut tampered = configured;
    tampered.compiler.as_mut().unwrap().version = "1.86.0".to_owned();
    assert!(!tampered.has_valid_identity());
}

#[test]
fn submodule_boundary_is_explicitly_accounted_for() {
    let temporary = tempfile::tempdir().unwrap();
    let repository_root = temporary.path().join("repo");
    let state_root = temporary.path().join("state");
    fs::create_dir_all(repository_root.join("dependency")).unwrap();
    fs::write(
        repository_root.join("dependency/.git"),
        b"gitdir: elsewhere\n",
    )
    .unwrap();
    fs::write(repository_root.join("dependency/lib.rs"), b"external\n").unwrap();

    let manifest =
        capture_snapshot(&repository_root, &state_root, &CaptureOptions::default()).unwrap();
    let path = SourcePath::new("dependency").unwrap();
    assert_eq!(manifest.issues[&path].kind, CaptureIssueKind::Submodule);
    assert_eq!(manifest.files[&path].class, FileClass::Unsupported);
    assert!(
        !manifest
            .files
            .contains_key(&SourcePath::new("dependency/lib.rs").unwrap())
    );
}
