//! Snapshot-scoped inventory publication and immutable run selections.
use super::{InventoryMetrics, JsonLinesInventorySink, SnapshotSource, io_error};
use argus_core::{ArgusError, ConfigurationId, ContentHash, RunId, SnapshotId};
use argus_language::{AdapterIdentity, AdapterInventory, InventorySink, SourceAccess};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Entry {
    pub adapter: AdapterIdentity,
    pub input: ContentHash,
    pub stream: String,
    pub digest: ContentHash,
    pub metrics: InventoryMetrics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Manifest {
    schema_version: u32,
    pub snapshot: SnapshotId,
    configuration: ConfigurationId,
    pub adapters: BTreeMap<String, Entry>,
}

pub(super) struct WorkspaceInventory {
    pub snapshot: SnapshotId,
    pub manifest: Manifest,
    pub targets: Vec<argus_core::Target>,
    pub evidence: Vec<argus_core::EvidenceRecord>,
    pub relations: Vec<argus_core::Relation>,
    pub partitions: Vec<(String, argus_language::DiscoveryPartition)>,
    pub owners: BTreeMap<argus_core::TargetId, Vec<String>>,
    pub conflicts: Vec<(String, argus_language::ConflictRecord)>,
    pub missing_adapters: Vec<String>,
    pub capture_issues: Vec<argus_snapshot::CaptureIssue>,
    pub excluded_typescript: Vec<argus_core::SourcePath>,
}

fn directory(root: &Path, snapshot: &SnapshotId) -> PathBuf {
    root.join(".argus/state/inventory").join(snapshot.as_str())
}

// An OS lock is released on process termination, including crashes. The lock file stays put.
pub(super) fn lock(root: &Path) -> Result<File, ArgusError> {
    let dir = root.join(".argus/state/inventory");
    std::fs::create_dir_all(&dir).map_err(io_error("cannot create inventory directory"))?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("publication.lock"))
        .map_err(io_error("cannot open inventory lock"))?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(io_error(
        "another inventory operation is active; retry when it finishes",
    ))?;
    Ok(file)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ArgusError> {
    let parent = path.parent().expect("inventory file has a parent");
    std::fs::create_dir_all(parent).map_err(io_error("cannot create inventory parent"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(io_error("cannot stage inventory metadata"))?;
    temp.write_all(bytes)
        .map_err(io_error("cannot write inventory metadata"))?;
    temp.as_file()
        .sync_all()
        .map_err(io_error("cannot sync inventory metadata"))?;
    temp.persist(path).map_err(|error| {
        ArgusError::invariant("cannot publish inventory metadata").with_source(error)
    })?;
    Ok(())
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ArgusError> {
    serde_json::to_vec_pretty(value)
        .map_err(|e| ArgusError::invariant("cannot serialize inventory metadata").with_source(e))
}

fn read_manifest(path: &Path) -> Result<Manifest, ArgusError> {
    let bytes = std::fs::read(path).map_err(io_error(
        "cannot read inventory manifest; prime the selected adapters first",
    ))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| ArgusError::invalid_input("invalid inventory manifest").with_source(e))?;
    manifest.snapshot.as_str().parse::<SnapshotId>()?;
    manifest.configuration.as_str().parse::<ConfigurationId>()?;
    if manifest.schema_version != 1 {
        return Err(ArgusError::unsupported(
            "unsupported inventory manifest schema",
        ));
    }
    Ok(manifest)
}

fn hash_file(path: &Path) -> Result<ContentHash, ArgusError> {
    let mut file = File::open(path).map_err(io_error("cannot hash inventory stream"))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let size = file
            .read(&mut buffer)
            .map_err(io_error("cannot hash inventory stream"))?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
    }
    ContentHash::parse(hasher.finalize().to_hex().to_string())
}

fn key(name: &str) -> Result<String, ArgusError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(ArgusError::invalid_input(
            "adapter identity must be a safe lowercase filesystem key",
        ));
    }
    Ok(name.to_owned())
}

pub(super) struct Publication<'a> {
    root: &'a Path,
    source: &'a dyn SourceAccess,
    manifest: Manifest,
    staged: tempfile::TempDir,
    keys: Vec<String>,
}

impl<'a> Publication<'a> {
    pub fn new(
        root: &'a Path,
        source: &'a dyn SourceAccess,
        configuration: ConfigurationId,
    ) -> Result<Self, ArgusError> {
        let dir = directory(root, source.snapshot_id());
        std::fs::create_dir_all(&dir)
            .map_err(io_error("cannot create snapshot inventory directory"))?;
        let manifest = if dir.join("manifest.json").exists() {
            read_manifest(&dir.join("manifest.json"))?
        } else {
            Manifest {
                schema_version: 1,
                snapshot: source.snapshot_id().clone(),
                configuration: configuration.clone(),
                adapters: BTreeMap::new(),
            }
        };
        if manifest.snapshot != *source.snapshot_id() || manifest.configuration != configuration {
            return Err(ArgusError::invariant(
                "inventory manifest snapshot/configuration mismatch",
            ));
        }
        let staged =
            tempfile::tempdir_in(&dir).map_err(io_error("cannot stage adapter inventories"))?;
        Ok(Self {
            root,
            source,
            manifest,
            staged,
            keys: Vec::new(),
        })
    }

    pub fn add(
        &mut self,
        inventory: AdapterInventory,
        semantic: &[u8],
        selection: &str,
    ) -> Result<(), ArgusError> {
        let inventory = argus_language::normalize_inventory(self.source, inventory)?;
        let name = key(&inventory.adapter.name)?;
        let input = ContentHash::digest(&encode(&(
            &inventory.adapter,
            &self.manifest.configuration,
            ContentHash::digest(semantic),
            selection,
        ))?);
        if let Some(old) = self.manifest.adapters.get(&name) {
            if old.input != input {
                return Err(ArgusError::invalid_input(format!(
                    "adapter {name} version/options/semantic inputs changed for this snapshot; rebuild its inventory with fresh state"
                )));
            }
        }
        let mut sink = JsonLinesInventorySink::new(self.staged.path(), self.source, &name)?;
        sink.begin(inventory.adapter.clone(), inventory.snapshot.clone())?;
        let identity = inventory.adapter.clone();
        super::append_inventory_items(&mut sink, inventory)?;
        sink.finish()?;
        let dir = directory(self.staged.path(), self.source.snapshot_id());
        let stream = format!("{name}.jsonl");
        let digest = hash_file(&dir.join(&stream))?;
        if self
            .manifest
            .adapters
            .get(&name)
            .is_some_and(|old| old.digest != digest)
        {
            return Err(ArgusError::invariant(format!(
                "snapshot inventory is not deterministic for adapter {name}"
            )));
        }
        let metrics = serde_json::from_slice(
            &std::fs::read(dir.join(format!("{name}-metrics.json")))
                .map_err(io_error("cannot read staged metrics"))?,
        )
        .map_err(|e| ArgusError::invariant("invalid staged metrics").with_source(e))?;
        self.manifest.adapters.insert(
            name.clone(),
            Entry {
                adapter: identity,
                input,
                stream,
                digest,
                metrics,
            },
        );
        self.keys.push(name);
        Ok(())
    }

    pub fn finish(self) -> Result<usize, ArgusError> {
        let dir = directory(self.root, self.source.snapshot_id());
        let staged = directory(self.staged.path(), self.source.snapshot_id());
        // Streams are immutable. Orphaned complete files after a crash are safe to compare/reuse.
        for name in &self.keys {
            let entry = &self.manifest.adapters[name];
            let destination = dir.join(&entry.stream);
            if destination.exists() {
                if hash_file(&destination)? != entry.digest {
                    return Err(ArgusError::invariant(format!(
                        "existing inventory differs for adapter {name}"
                    )));
                }
            } else {
                std::fs::rename(staged.join(&entry.stream), &destination)
                    .map_err(io_error("cannot publish adapter stream"))?;
            }
            atomic_write(
                &dir.join(format!("{name}-metrics.json")),
                &encode(&entry.metrics)?,
            )?;
        }
        atomic_write(&dir.join("manifest.json"), &encode(&self.manifest)?)?;
        for name in &self.keys {
            atomic_write(
                &self
                    .root
                    .join(".argus/state/inventory")
                    .join(format!("current-{name}")),
                self.manifest.snapshot.as_str().as_bytes(),
            )?;
        }
        Ok(self
            .keys
            .iter()
            .map(|name| self.manifest.adapters[name].metrics.targets)
            .sum())
    }
}

fn run_path(root: &Path, run: &RunId, selected: bool) -> PathBuf {
    root.join(".argus/state/inventory/runs").join(format!(
        "{run}{}.json",
        if selected { "-selected" } else { "" }
    ))
}

pub(super) fn has_run(root: &Path, run: &RunId) -> bool {
    run_path(root, run, false).is_file()
}

pub(super) fn pin_prime(root: &Path, run: &argus_storage::RunRecord) -> Result<(), ArgusError> {
    let path = directory(root, &run.snapshot).join("manifest.json");
    if path.exists() {
        atomic_write(
            &run_path(root, &run.id, false),
            &std::fs::read(path).map_err(io_error("cannot pin run inventory"))?,
        )?;
    }
    Ok(())
}

pub(super) fn canonical_filter(value: &str) -> String {
    match value {
        "ts" | "js" | "javascript" => "typescript".to_owned(),
        "py" => "python".to_owned(),
        "go" | "golang" => "treesitter-go".to_owned(),
        "c++" | "cxx" | "cpp" => "treesitter-cpp".to_owned(),
        "cs" | "csharp" | "c#" | "c_sharp" => "treesitter-c_sharp".to_owned(),
        "hs" | "haskell" => "treesitter-haskell".to_owned(),
        "kt" | "kotlin" => "treesitter-kotlin".to_owned(),
        "c" | "zig" | "swift" => format!("treesitter-{value}"),
        other => other.replace("treesitter:", "treesitter-"),
    }
}

pub(super) fn for_run(
    root: &Path,
    run: &RunId,
    filter: Option<&str>,
) -> Result<WorkspaceInventory, ArgusError> {
    let selected = run_path(root, run, true);
    let mut manifest = read_manifest(&if selected.exists() {
        selected
    } else {
        run_path(root, run, false)
    })?;
    if let Some(filter) = filter.filter(|f| *f != "all") {
        let filter = canonical_filter(filter);
        if !manifest.adapters.contains_key(&filter) {
            return Err(ArgusError::invalid_input(format!(
                "adapter {filter} is not in this run's inventory; prime it and start a new run"
            )));
        }
        manifest.adapters.retain(|name, _| name == &filter);
    }
    assemble(root, manifest)
}

pub(super) fn load(root: &Path, filter: Option<&str>) -> Result<WorkspaceInventory, ArgusError> {
    for_run(root, &super::current_run(root)?, filter)
}

pub(super) fn pin_selection(root: &Path, inventory: &WorkspaceInventory) -> Result<(), ArgusError> {
    let path = run_path(root, &super::current_run(root)?, true);
    let bytes = encode(&inventory.manifest)?;
    if path.exists()
        && std::fs::read(&path).map_err(io_error("cannot read pinned selection"))? != bytes
    {
        return Err(ArgusError::invalid_input(
            "run adapter selection is already pinned; prime a new run to change scope",
        ));
    }
    atomic_write(&path, &bytes)
}

fn merge<K: Ord, T: PartialEq>(
    map: &mut BTreeMap<K, T>,
    id: K,
    value: T,
    name: &str,
) -> Result<(), ArgusError> {
    if let Some(old) = map.get(&id) {
        if old != &value {
            return Err(ArgusError::invariant(format!(
                "conflicting inventory identity from adapter {name}"
            )));
        }
    } else {
        map.insert(id, value);
    }
    Ok(())
}

fn assemble(root: &Path, manifest: Manifest) -> Result<WorkspaceInventory, ArgusError> {
    let repository = argus_snapshot::SnapshotRepository::open(root.join(".argus/state/sources"))?;
    let snapshot = repository.load_manifest(&manifest.snapshot)?;
    if snapshot.configuration.id != manifest.configuration {
        return Err(ArgusError::invariant(
            "inventory configuration does not match source snapshot",
        ));
    }
    let mut expected = std::collections::BTreeSet::new();
    for path in snapshot.files.keys() {
        let adapter = match Path::new(path.as_str())
            .extension()
            .and_then(|e| e.to_str())
        {
            Some("rs") => Some("rust"),
            Some("ts" | "tsx" | "js" | "jsx") => Some("typescript"),
            Some("py") => Some("python"),
            Some("java") => Some("java"),
            _ => None,
        };
        if let Some(adapter) = adapter {
            expected.insert(adapter.to_owned());
        }
    }
    let missing_adapters = expected
        .into_iter()
        .filter(|a| !manifest.adapters.contains_key(a))
        .collect();
    let capture_issues = snapshot.issues.values().cloned().collect();
    let excluded_typescript = snapshot
        .files
        .keys()
        .filter(|path| {
            matches!(
                Path::new(path.as_str())
                    .extension()
                    .and_then(|e| e.to_str()),
                Some("ts" | "tsx" | "js" | "jsx")
            ) && !argus_typescript::TypeScriptSyntaxProvider::is_supported_source(path)
        })
        .cloned()
        .collect();
    let source = SnapshotSource(repository.reader(snapshot));
    let (mut targets, mut evidence, mut relations, mut owners) = (
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::<_, Vec<String>>::new(),
    );
    let mut partitions = Vec::new();
    let mut conflicts = Vec::new();
    for (name, entry) in &manifest.adapters {
        if key(name)? != entry.adapter.name || entry.stream != format!("{name}.jsonl") {
            return Err(ArgusError::invariant(
                "invalid adapter manifest path or identity",
            ));
        }
        let path = directory(root, &manifest.snapshot).join(&entry.stream);
        if hash_file(&path)? != entry.digest {
            return Err(ArgusError::invariant(format!(
                "inventory digest mismatch for {name}"
            )));
        }
        let inventory = super::read_inventory_stream(&path)?;
        if inventory.adapter != entry.adapter {
            return Err(ArgusError::invariant(format!(
                "inventory header mismatch for {name}"
            )));
        }
        let inventory = argus_language::normalize_inventory(&source, inventory)?;
        if (
            inventory.targets.len(),
            inventory.evidence.len(),
            inventory.relations.len(),
            inventory.partitions.len(),
            inventory.conflicts.len(),
        ) != (
            entry.metrics.targets,
            entry.metrics.evidence,
            entry.metrics.relations,
            entry.metrics.partitions,
            entry.metrics.conflicts,
        ) {
            return Err(ArgusError::invariant(format!(
                "inventory record counts mismatch for {name}"
            )));
        }
        for target in inventory.targets {
            owners
                .entry(target.id.clone())
                .or_default()
                .push(name.clone());
            merge(&mut targets, target.id.clone(), target, name)?;
        }
        for item in inventory.evidence {
            merge(&mut evidence, item.id.clone(), item, name)?;
        }
        let mut ids = std::collections::BTreeSet::new();
        for relation in inventory.relations {
            if !ids.insert(relation.id.clone()) {
                return Err(ArgusError::invariant(format!(
                    "duplicate relation identity in {name}"
                )));
            }
            merge(&mut relations, relation.id.clone(), relation, name)?;
        }
        partitions.extend(inventory.partitions.into_iter().map(|p| (name.clone(), p)));
        conflicts.extend(inventory.conflicts.into_iter().map(|c| (name.clone(), c)));
    }
    Ok(WorkspaceInventory {
        snapshot: manifest.snapshot.clone(),
        manifest,
        targets: targets.into_values().collect(),
        evidence: evidence.into_values().collect(),
        relations: relations.into_values().collect(),
        partitions,
        owners,
        conflicts,
        missing_adapters,
        capture_issues,
        excluded_typescript,
    })
}

pub(super) fn describe(inventory: &WorkspaceInventory) -> String {
    let mut output = format!(
        "Snapshot: {}\nInventory scope: committed adapters in this run (not whole-workspace completeness)\nManifest digest: {}\nTargets: {}\nEvidence: {}\nRelations: {}\n",
        inventory.snapshot,
        ContentHash::digest(&encode(&inventory.manifest).expect("serializable manifest")).as_str(),
        inventory.targets.len(),
        inventory.evidence.len(),
        inventory.relations.len()
    );
    for (name, entry) in &inventory.manifest.adapters {
        output.push_str(&format!("Adapter: {name}@{} targets={} evidence={} relations={} conflicts={} stream_bytes={} elapsed_millis={}\n", entry.adapter.version, entry.metrics.targets, entry.metrics.evidence, entry.metrics.relations, entry.metrics.conflicts, entry.metrics.stream_bytes, entry.metrics.elapsed_millis));
    }
    let mut counts = BTreeMap::<String, usize>::new();
    for target in &inventory.targets {
        *counts
            .entry(format!(
                "{} {:?}",
                super::target_kind_label(&target.kind),
                target.visibility
            ))
            .or_default() += 1;
    }
    for (class, count) in counts {
        output.push_str(&format!("{class}: {count}\n"));
    }
    for (name, partition) in &inventory.partitions {
        output.push_str(&format!(
            "{name}/{}\t{:?}\t{}\n",
            partition.name,
            partition.status,
            partition.diagnostic.as_deref().unwrap_or("")
        ));
    }
    for (name, conflict) in &inventory.conflicts {
        output.push_str(&format!(
            "Conflict {name}/{}: {}\n",
            conflict.subject, conflict.detail
        ));
    }
    if !inventory.missing_adapters.is_empty() {
        output.push_str(&format!(
            "Source languages outside selected inventory (detected by extension): {}\n",
            inventory.missing_adapters.join(", ")
        ));
    }
    output.push_str("Discovery scope: captured snapshot files; adapter dependency/build exclusions and capability partitions apply. Counts are declarations, not source lines.\n");
    for issue in &inventory.capture_issues {
        output.push_str(&format!(
            "Capture limitation {}: {:?}: {}\n",
            issue.path.as_str(),
            issue.kind,
            issue.detail
        ));
    }
    for path in &inventory.excluded_typescript {
        output.push_str(&format!(
            "TypeScript discovery exclusion: {} (adapter path/support rules)\n",
            path.as_str()
        ));
    }
    output.push_str(&format!(
        "Retained identifiers: {}\n",
        inventory
            .manifest
            .adapters
            .values()
            .map(|e| e.metrics.retained_identifiers)
            .sum::<usize>()
    ));
    output.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"inventory-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[lib]\npath=\"lib.rs\"\n").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "/// Public entry.\npub fn entry() { hidden(); }\nfn hidden() { panic!(\"non-obvious\"); }\n").unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            "{\"name\":\"console\",\"version\":\"1.0.0\"}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("index.ts"),
            "export function consoleEntry() { return 1; }\n",
        )
        .unwrap();
        super::super::cargo_metadata(dir.path()).unwrap();
        dir
    }

    fn prime(root: &Path, adapter: &str) {
        super::super::prime_command(root, ["--adapter", adapter].map(str::to_owned).into_iter())
            .unwrap();
    }

    #[test]
    fn independent_adapters_and_all_have_identical_streams_and_pin_runs() {
        for order in [["rust", "typescript"], ["typescript", "rust"]] {
            let dir = workspace();
            prime(dir.path(), order[0]);
            let first_run = super::super::current_run(dir.path()).unwrap();
            let first = load(dir.path(), None).unwrap();
            assert_eq!(first.manifest.adapters.len(), 1);
            prime(dir.path(), order[1]);
            let combined = load(dir.path(), None).unwrap();
            assert_eq!(combined.snapshot, first.snapshot);
            assert_eq!(combined.manifest.adapters.len(), 2);
            assert!(combined.targets.iter().any(|t| t.name == "hidden"));
            assert!(combined.targets.iter().any(|t| t.name == "consoleEntry"));
            let pinned = for_run(dir.path(), &first_run, None).unwrap();
            assert_eq!(pinned.targets, first.targets);
            prime(dir.path(), "all");
            let all = load(dir.path(), None).unwrap();
            assert_eq!(all.targets, combined.targets);
            assert_eq!(all.evidence, combined.evidence);
            for (name, entry) in &combined.manifest.adapters {
                assert_eq!(all.manifest.adapters[name].digest, entry.digest);
            }
            prime(dir.path(), "all");
            assert_eq!(load(dir.path(), None).unwrap().targets, all.targets);
        }
    }

    #[test]
    fn filters_aliases_and_snapshot_changes_do_not_merge_latest_pointers() {
        let dir = workspace();
        prime(dir.path(), "all");
        let first = super::super::current_run(dir.path()).unwrap();
        let selected = load(dir.path(), Some("js")).unwrap();
        assert_eq!(selected.manifest.adapters.len(), 1);
        pin_selection(dir.path(), &selected).unwrap();
        assert!(load(dir.path(), Some("rust")).is_err());
        std::fs::write(
            dir.path().join("index.ts"),
            "export function changed() {}\n",
        )
        .unwrap();
        prime(dir.path(), "typescript");
        let newer = load(dir.path(), None).unwrap();
        assert_ne!(selected.snapshot, newer.snapshot);
        assert_eq!(newer.manifest.adapters.len(), 1);
        assert!(newer.missing_adapters.contains(&"rust".to_owned()));
        assert_eq!(
            for_run(dir.path(), &first, None).unwrap().targets,
            selected.targets
        );
    }

    #[test]
    fn failed_rust_discovery_does_not_publish_partial_all_or_replace_run() {
        let dir = workspace();
        prime(dir.path(), "typescript");
        let run = super::super::current_run(dir.path()).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "this is not TOML").unwrap();
        let error = super::super::prime_command(
            dir.path(),
            ["--adapter", "all"].map(str::to_owned).into_iter(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cargo metadata failed"));
        assert_eq!(super::super::current_run(dir.path()).unwrap(), run);
        assert_eq!(load(dir.path(), None).unwrap().manifest.adapters.len(), 1);
    }

    #[test]
    fn locks_release_and_unpublished_staging_is_invisible() {
        let dir = workspace();
        prime(dir.path(), "rust");
        let guard = lock(dir.path()).unwrap();
        assert!(lock(dir.path()).is_err());
        let loaded = load(dir.path(), None).unwrap();
        let repository =
            argus_snapshot::SnapshotRepository::open(dir.path().join(".argus/state/sources"))
                .unwrap();
        let snapshot = repository.load_manifest(&loaded.snapshot).unwrap();
        let source = SnapshotSource(repository.reader(snapshot.clone()));
        let mut publication =
            Publication::new(dir.path(), &source, snapshot.configuration.id.clone()).unwrap();
        let adapter = argus_typescript::TypeScriptWorkspaceAdapter::new(
            snapshot.configuration.id,
            snapshot.files.keys().cloned().collect(),
        );
        use argus_language::LanguageAdapter;
        publication
            .add(adapter.inventory(&source).unwrap(), &[], "default")
            .unwrap();
        drop(publication);
        assert_eq!(load(dir.path(), None).unwrap().manifest.adapters.len(), 1);
        drop(guard);
        assert!(lock(dir.path()).is_ok());
    }

    #[test]
    fn rejects_tampered_streams_paths_headers_and_input_changes() {
        let dir = workspace();
        prime(dir.path(), "rust");
        let loaded = load(dir.path(), None).unwrap();
        let mut manifest = loaded.manifest.clone();
        manifest.adapters.get_mut("rust").unwrap().stream = "../outside.jsonl".to_owned();
        assert!(assemble(dir.path(), manifest).is_err());
        let path = directory(dir.path(), &loaded.snapshot).join("rust.jsonl");
        let original = std::fs::read(&path).unwrap();
        let mut modified = original.clone();
        modified.extend_from_slice(b"{}\n");
        std::fs::write(&path, &modified).unwrap();
        assert!(load(dir.path(), None).is_err());
        std::fs::write(&path, &original).unwrap();
        let repository =
            argus_snapshot::SnapshotRepository::open(dir.path().join(".argus/state/sources"))
                .unwrap();
        let snapshot = repository.load_manifest(&loaded.snapshot).unwrap();
        let source = SnapshotSource(repository.reader(snapshot.clone()));
        let mut publication =
            Publication::new(dir.path(), &source, snapshot.configuration.id).unwrap();
        let inventory = super::super::read_inventory_stream(&path).unwrap();
        assert!(
            publication
                .add(inventory.clone(), b"changed semantics", "default")
                .unwrap_err()
                .to_string()
                .contains("inputs changed")
        );
        let mut changed = inventory;
        changed.targets[0].name.push_str(" changed");
        assert!(
            publication
                .add(changed, &[], "default")
                .unwrap_err()
                .to_string()
                .contains("not deterministic")
        );
    }

    #[test]
    fn internal_documentation_is_separately_admitted_and_reported() {
        let dir = workspace();
        prime(dir.path(), "all");
        let output = super::super::audit_command(
            dir.path(),
            ["--pipeline", "full"].map(str::to_owned).into_iter(),
        )
        .unwrap();
        assert!(output.contains("Internal documentation plan"));
        let run = super::super::current_run(dir.path()).unwrap();
        let queue = super::super::working_queue(dir.path()).unwrap();
        let records = queue.run_records(&run).unwrap();
        let hidden = load(dir.path(), None)
            .unwrap()
            .targets
            .into_iter()
            .find(|t| t.name == "hidden")
            .unwrap();
        let internal = records
            .work
            .iter()
            .filter(|w| w.coverage.policy == "documentation-internal@1")
            .collect::<Vec<_>>();
        assert!(!internal.is_empty());
        assert!(internal.iter().all(|w| w.coverage.adapter == "workspace"));
        assert!(internal.iter().any(|w| {
            serde_json::from_slice::<argus_workflow::DocumentationReviewAdmission>(&w.payload)
                .unwrap()
                .unit
                .target
                .target
                == hidden.id
        }));
        drop(queue);
        let output = super::super::report_command(
            dir.path(),
            ["--format", "json"].map(str::to_owned).into_iter(),
        )
        .unwrap();
        let report: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            report["internal_documentation"]["policy_version"],
            "documentation-internal@1"
        );
        assert_eq!(
            report["documentation"]["policy_version"],
            "documentation-public-api@1"
        );
    }

    #[test]
    fn dependency_paths_are_not_mistaken_for_first_party_typescript() {
        let dir = workspace();
        for folder in [
            "console/src",
            "console/node_modules/example",
            "node_modules/example",
            "console/dist",
        ] {
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
            std::fs::write(
                dir.path().join(folder).join("index.ts"),
                if folder == "console/src" {
                    "export function firstParty() {}\n"
                } else {
                    "export function dependencyOnly() {}\n"
                },
            )
            .unwrap();
        }
        prime(dir.path(), "typescript");
        let inventory = load(dir.path(), None).unwrap();
        assert!(inventory.targets.iter().any(|t| t.name == "firstParty"));
        assert!(!inventory.targets.iter().any(|t| t.name == "dependencyOnly"));
        assert_eq!(
            inventory.targets.len(),
            inventory.manifest.adapters["typescript"].metrics.targets
        );
    }

    #[test]
    fn loading_validates_headers_and_references_even_with_matching_hashes() {
        let dir = workspace();
        prime(dir.path(), "rust");
        let original = load(dir.path(), None).unwrap().manifest;
        let path = directory(dir.path(), &original.snapshot).join("rust.jsonl");
        let records: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for header in [true, false] {
            let mut changed = records.clone();
            if header {
                changed[0]["adapter"]["name"] = "typescript".into();
            } else {
                let relation = changed
                    .iter_mut()
                    .find(|r| r["record"] == "relation")
                    .unwrap();
                relation["value"]["source"] = argus_core::TargetId::derive([b"missing".as_slice()])
                    .to_string()
                    .into();
            }
            let bytes = changed
                .iter()
                .map(|r| serde_json::to_string(r).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            std::fs::write(&path, bytes).unwrap();
            let mut manifest = original.clone();
            manifest.adapters.get_mut("rust").unwrap().digest = hash_file(&path).unwrap();
            assert!(assemble(dir.path(), manifest).is_err());
        }
    }

    #[cfg(feature = "treesitter")]
    #[test]
    fn treesitter_aliases_use_safe_canonical_streams() {
        let dir = workspace();
        std::fs::write(dir.path().join("main.go"), "package main\nfunc main() {}\n").unwrap();
        prime(dir.path(), "go");
        let first = load(dir.path(), Some("treesitter:go")).unwrap();
        assert!(first.manifest.adapters.contains_key("treesitter-go"));
        prime(dir.path(), "golang");
        assert_eq!(first.targets, load(dir.path(), Some("go")).unwrap().targets);
    }
}
