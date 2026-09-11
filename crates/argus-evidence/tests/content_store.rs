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

use argus_core::{
    ConfigurationId, ContentHash, EvidenceId, EvidenceKind, EvidenceOrigin, EvidenceProvenance,
    EvidenceRecord, ResolutionQuality, SnapshotId,
};
use argus_evidence::{DataClassification, EvidenceEnvelope, EvidenceStore};
use std::fs;

fn envelope() -> EvidenceEnvelope {
    EvidenceEnvelope::current(
        SnapshotId::derive([b"snapshot".as_slice()]),
        DataClassification::Sensitive,
        EvidenceRecord {
            id: EvidenceId::derive([b"diagnostic".as_slice()]),
            kind: EvidenceKind::StaticAnalysis,
            origin: EvidenceOrigin::Direct,
            target: None,
            location: None,
            summary: "compiler warning".to_owned(),
            detail: Some("unused result".to_owned()),
            provenance: EvidenceProvenance {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                configuration: ConfigurationId::derive([b"default".as_slice()]),
                ingest_only: true,
                resolution: ResolutionQuality::Unmapped,
            },
        },
    )
}

#[test]
fn identical_envelopes_are_deduplicated_and_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let envelope = envelope();
    let first = store.put(&envelope).unwrap();
    let repeat = store.put(&envelope).unwrap();

    assert_eq!(first, repeat);
    assert_eq!(store.get(&first).unwrap().envelope, envelope);
}

#[test]
fn classification_and_provenance_change_content_identity() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let first = store.put(&envelope()).unwrap();
    let mut changed = envelope();
    changed.classification = DataClassification::Restricted;
    let second = store.put(&changed).unwrap();
    assert_ne!(first, second);
}

#[test]
fn corrupted_objects_fail_hash_verification() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let hash = store.put(&envelope()).unwrap();
    let path = directory
        .path()
        .join("objects")
        .join(&hash.as_str()[..2])
        .join(hash.as_str());
    fs::write(path, b"corrupt").unwrap();
    assert!(
        store
            .get(&hash)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch")
    );
}

#[test]
fn content_hash_parsing_rejects_unsafe_object_names() {
    assert!(ContentHash::parse("../escape").is_err());
    let digest = ContentHash::digest(b"safe");
    assert_eq!(ContentHash::parse(digest.as_str()).unwrap(), digest);
}

#[test]
fn list_and_prune_objects_removes_files_and_cleans_shards() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let first = store.put(&envelope()).unwrap();

    let mut env2 = envelope();
    env2.classification = DataClassification::Restricted;
    let second = store.put(&env2).unwrap();
    assert_ne!(first, second);

    let objects = store.list_objects().unwrap();
    assert_eq!(objects.len(), 2);

    // Prune first object
    let (pruned_count, reclaimed_bytes) = store.prune_objects(&[first.clone()]).unwrap();
    assert_eq!(pruned_count, 1);
    assert!(reclaimed_bytes > 0);

    let remaining = store.list_objects().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].0, second);

    // Clear all remaining objects
    let (cleared_count, _) = store.clear_objects().unwrap();
    assert_eq!(cleared_count, 1);
    assert!(store.list_objects().unwrap().is_empty());
}
