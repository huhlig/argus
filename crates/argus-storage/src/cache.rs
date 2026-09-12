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

//! Durable review assessment cache keyed by target, policy, and review fingerprint.
//!
//! Stores reusable assessment outcomes and artifacts in `redb` to accelerate incremental
//! and CI-lite evaluation workflows without re-invoking model providers when target code
//! and environmental dependencies are unchanged.

use argus_core::{ArgusError, ContentHash, PolicyId, ReviewFingerprint, TargetId};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// redb table storing review assessment records.
///
/// Key format: `<target_id>\0<policy_id>\0<fingerprint_composite_hash>`
/// Value format: Serialized JSON bytes of [`CachedAssessmentRecord`].
pub const REVIEW_ASSESSMENT_CACHE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("review_assessment_cache_v1");

/// Constructs the canonical composite cache key for a target review assessment.
///
/// Uses null-byte delimiters (`\0`) to prevent collision or ambiguity between
/// components regardless of colons or special characters in target or policy IDs.
#[must_use]
pub fn review_assessment_cache_key(
    target: &TargetId,
    policy: &PolicyId,
    fingerprint_hash: &ContentHash,
) -> String {
    format!(
        "{}\0{}\0{}",
        target.as_str(),
        policy.as_str(),
        fingerprint_hash.as_str()
    )
}

/// Parses a composite cache key into its constituent `(target, policy, fingerprint_hash)` parts.
#[must_use]
pub fn parse_review_assessment_cache_key(key: &str) -> Option<(&str, &str, &str)> {
    let mut parts = key.split('\0');
    let target = parts.next()?;
    let policy = parts.next()?;
    let hash = parts.next()?;
    if parts.next().is_none() {
        Some((target, policy, hash))
    } else {
        None
    }
}

/// A persistently cached review assessment record.
///
/// Contains the evaluation outcome, serialized payload, referenced artifacts,
/// and provenance metadata necessary to reproduce or synthesize review outcomes
/// directly without invoking LLM providers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CachedAssessmentRecord {
    /// Target evaluated.
    pub target: TargetId,
    /// Policy evaluated against.
    pub policy: PolicyId,
    /// Canonical content-addressed review fingerprint.
    pub fingerprint: ReviewFingerprint,
    /// Deterministic BLAKE3 composite hash of the review fingerprint.
    pub fingerprint_hash: ContentHash,
    /// Serialized assessment payload or outcome record (e.g. JSON bytes of the assessment).
    pub payload: Vec<u8>,
    /// High-level assessment outcome kind (e.g. `"passed"`, `"candidate_findings"`, `"unable_to_verify"`).
    pub outcome_kind: String,
    /// Optional content addresses of supporting artifacts stored in the artifact table.
    pub artifact_references: Vec<String>,
    /// Human-readable diagnostic summary or reasoning snippet.
    pub summary: Option<String>,
    /// Optional model reuse class or provider identifier.
    pub model_reuse_class: Option<String>,
    /// Epoch timestamp in milliseconds when this assessment was recorded into the cache.
    pub cached_at_millis: u64,
    /// Optional epoch timestamp in milliseconds when this cache entry expires.
    /// `None` indicates the entry does not expire by time (only via invalidation).
    pub expires_at_millis: Option<u64>,
    /// Epoch timestamp in milliseconds when this cache entry was last hit.
    pub last_accessed_at_millis: u64,
    /// Cumulative count of cache hits served by this entry.
    pub hit_count: u64,
}

impl CachedAssessmentRecord {
    /// Creates a new cached assessment record with initialized access metadata.
    #[must_use]
    pub fn new(
        target: TargetId,
        policy: PolicyId,
        fingerprint: ReviewFingerprint,
        payload: Vec<u8>,
        outcome_kind: impl Into<String>,
        cached_at_millis: u64,
    ) -> Self {
        let fingerprint_hash = fingerprint.composite_hash();
        Self {
            target,
            policy,
            fingerprint,
            fingerprint_hash,
            payload,
            outcome_kind: outcome_kind.into(),
            artifact_references: Vec::new(),
            summary: None,
            model_reuse_class: None,
            cached_at_millis,
            expires_at_millis: None,
            last_accessed_at_millis: cached_at_millis,
            hit_count: 0,
        }
    }

    /// Sets a relative time-to-live (TTL) duration in milliseconds from creation time.
    #[must_use]
    pub fn with_ttl(mut self, ttl_millis: u64) -> Self {
        self.expires_at_millis = Some(self.cached_at_millis.saturating_add(ttl_millis));
        self
    }

    /// Sets an absolute expiration timestamp in milliseconds.
    #[must_use]
    pub fn with_expires_at(mut self, expires_at_millis: u64) -> Self {
        self.expires_at_millis = Some(expires_at_millis);
        self
    }

    /// Associates supporting artifact content references with this cached assessment.
    #[must_use]
    pub fn with_artifacts(mut self, artifact_references: Vec<String>) -> Self {
        self.artifact_references = artifact_references;
        self
    }

    /// Attaches an optional human-readable summary.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Attaches an optional model reuse class classification.
    #[must_use]
    pub fn with_model_reuse_class(mut self, model_reuse_class: impl Into<String>) -> Self {
        self.model_reuse_class = Some(model_reuse_class.into());
        self
    }

    /// Checks if this cache entry has expired relative to the given epoch timestamp.
    #[must_use]
    pub fn is_expired(&self, now_millis: u64) -> bool {
        self.expires_at_millis.is_some_and(|exp| now_millis >= exp)
    }

    /// Validates structural invariants and fingerprint identity consistency.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if target, policy, or composite hash do not match the fingerprint.
    pub fn validate(&self) -> Result<(), ArgusError> {
        self.fingerprint.validate()?;
        if self.target != self.fingerprint.target {
            return Err(ArgusError::invariant(
                "cached assessment target does not match fingerprint target",
            ));
        }
        if self.policy != self.fingerprint.policy {
            return Err(ArgusError::invariant(
                "cached assessment policy does not match fingerprint policy",
            ));
        }
        if self.fingerprint_hash != self.fingerprint.composite_hash() {
            return Err(ArgusError::invariant(
                "cached assessment fingerprint_hash does not match composite hash",
            ));
        }
        if self.outcome_kind.trim().is_empty() {
            return Err(ArgusError::invalid_input(
                "cached assessment outcome_kind cannot be empty",
            ));
        }
        Ok(())
    }

    /// Generates the canonical cache key for this record.
    #[must_use]
    pub fn cache_key(&self) -> String {
        review_assessment_cache_key(&self.target, &self.policy, &self.fingerprint_hash)
    }
}

/// Operational summary statistics for review assessment cache telemetry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CachedAssessmentStats {
    /// Total number of entries stored in the cache table.
    pub total_entries: usize,
    /// Number of entries currently active (unexpired) at queried timestamp.
    pub active_entries: usize,
    /// Number of expired entries eligible for pruning.
    pub expired_entries: usize,
    /// Cumulative cache hits served across all stored entries.
    pub total_hits: u64,
    /// Oldest creation timestamp among cached entries, if any.
    pub oldest_cached_millis: Option<u64>,
    /// Newest creation timestamp among cached entries, if any.
    pub newest_cached_millis: Option<u64>,
    /// Total bytes of serialized assessment payloads stored.
    pub total_payload_bytes: u64,
}

/// Standalone durable review assessment cache engine backed by `redb`.
#[derive(Debug)]
pub struct ReviewAssessmentCache {
    database: Database,
    path: PathBuf,
}

impl ReviewAssessmentCache {
    /// Opens or creates a standalone review assessment cache database at the specified path.
    ///
    /// Parent directories will be created automatically if they do not exist.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if directory creation, database open, or table initialization fails.
    pub fn open(path: &Path) -> Result<Self, ArgusError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error("cannot create cache directory"))?;
        }
        let database = Database::create(path).map_err(database_error("cannot open cache redb"))?;
        let cache = Self {
            database,
            path: path.to_owned(),
        };
        cache.initialize()?;
        Ok(cache)
    }

    fn initialize(&self) -> Result<(), ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot begin cache schema transaction"))?;
        {
            write
                .open_table(REVIEW_ASSESSMENT_CACHE)
                .map_err(database_error("cannot create review assessment cache table"))?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit cache schema transaction"))
    }

    /// Returns the filesystem path to this cache database.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stores a cached assessment record, replacing any previous record with the same key.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if validation fails or database write transaction fails.
    pub fn insert(&self, record: &CachedAssessmentRecord) -> Result<(), ArgusError> {
        db_insert_cached_assessment(&self.database, record)
    }

    /// Retrieves an active cached assessment matching the given target, policy, and exact review fingerprint.
    ///
    /// Returns `None` if no matching record exists, if the fingerprint differs, or if the record is expired.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if reading or decoding fails.
    pub fn get(
        &self,
        target: &TargetId,
        policy: &PolicyId,
        fingerprint: &ReviewFingerprint,
        now_millis: u64,
    ) -> Result<Option<CachedAssessmentRecord>, ArgusError> {
        db_get_cached_assessment(&self.database, target, policy, fingerprint, now_millis)
    }

    /// Retrieves an active cached assessment matching target, policy, and fingerprint composite hash.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if reading or decoding fails.
    pub fn get_by_hash(
        &self,
        target: &TargetId,
        policy: &PolicyId,
        fingerprint_hash: &ContentHash,
        now_millis: u64,
    ) -> Result<Option<CachedAssessmentRecord>, ArgusError> {
        db_get_cached_assessment_by_hash(
            &self.database,
            target,
            policy,
            fingerprint_hash,
            now_millis,
        )
    }

    /// Updates the access time and hit counter for a cache entry without altering its content.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if reading or updating fails.
    pub fn touch(
        &self,
        target: &TargetId,
        policy: &PolicyId,
        fingerprint_hash: &ContentHash,
        now_millis: u64,
    ) -> Result<bool, ArgusError> {
        db_touch_cached_assessment(
            &self.database,
            target,
            policy,
            fingerprint_hash,
            now_millis,
        )
    }

    /// Prunes all expired cache entries whose `expires_at_millis <= now_millis`.
    ///
    /// Returns the number of expired entries removed.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if scanning or removing fails.
    pub fn prune_expired(&self, now_millis: u64) -> Result<usize, ArgusError> {
        db_prune_expired_assessments(&self.database, now_millis)
    }

    /// Prunes all cached assessments for a single invalidated target.
    ///
    /// Returns the number of entries removed.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if scanning or removing fails.
    pub fn prune_invalidated_target(&self, target: &TargetId) -> Result<usize, ArgusError> {
        db_prune_invalidated_targets(&self.database, std::slice::from_ref(target))
    }

    /// Prunes all cached assessments for a collection of invalidated targets in a single pass.
    ///
    /// Returns the number of entries removed.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if scanning or removing fails.
    pub fn prune_invalidated_targets(&self, targets: &[TargetId]) -> Result<usize, ArgusError> {
        db_prune_invalidated_targets(&self.database, targets)
    }

    /// Prunes all cached assessments for a specific target and policy pair.
    ///
    /// Returns the number of entries removed.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if scanning or removing fails.
    pub fn prune_invalidated_target_policy(
        &self,
        target: &TargetId,
        policy: &PolicyId,
    ) -> Result<usize, ArgusError> {
        db_prune_invalidated_target_policy(&self.database, target, policy)
    }

    /// Prunes cached assessments matching an arbitrary predicate filter.
    ///
    /// Useful for environment invalidation (e.g. prompt template updates, toolchain changes).
    /// Returns the number of entries removed.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if scanning or removing fails.
    pub fn prune_by_filter<F>(&self, should_remove: F) -> Result<usize, ArgusError>
    where
        F: Fn(&CachedAssessmentRecord) -> bool,
    {
        db_prune_by_filter(&self.database, should_remove)
    }

    /// Returns the total count of cached assessment entries.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if scanning fails.
    pub fn count(&self) -> Result<usize, ArgusError> {
        db_cached_assessment_count(&self.database)
    }

    /// Retrieves all cached assessment entries currently stored in the cache.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if reading or decoding fails.
    pub fn all(&self) -> Result<Vec<CachedAssessmentRecord>, ArgusError> {
        db_all_cached_assessments(&self.database)
    }

    /// Computes summary statistics across all cached entries relative to `now_millis`.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if reading fails.
    pub fn stats(&self, now_millis: u64) -> Result<CachedAssessmentStats, ArgusError> {
        db_cached_assessment_stats(&self.database, now_millis)
    }

    /// Clears all entries from the cache.
    ///
    /// Returns the number of entries deleted.
    ///
    /// # Errors
    /// Returns [`ArgusError`] if deleting fails.
    pub fn clear(&self) -> Result<usize, ArgusError> {
        db_clear_cached_assessments(&self.database)
    }
}

// ============================================================================
// Internal database-level execution functions shared across DurableQueue and ReviewAssessmentCache
// ============================================================================

pub(crate) fn db_insert_cached_assessment(
    database: &Database,
    record: &CachedAssessmentRecord,
) -> Result<(), ArgusError> {
    record.validate()?;
    let key = record.cache_key();
    let bytes = encode_record(record)?;
    let write = database
        .begin_write()
        .map_err(database_error("cannot begin write transaction for assessment cache"))?;
    {
        let mut table = write
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        table
            .insert(key.as_str(), bytes.as_slice())
            .map_err(database_error("cannot insert cached assessment"))?;
    }
    write
        .commit()
        .map_err(database_error("cannot commit cached assessment"))
}

pub(crate) fn db_get_cached_assessment(
    database: &Database,
    target: &TargetId,
    policy: &PolicyId,
    fingerprint: &ReviewFingerprint,
    now_millis: u64,
) -> Result<Option<CachedAssessmentRecord>, ArgusError> {
    let fingerprint_hash = fingerprint.composite_hash();
    let key = review_assessment_cache_key(target, policy, &fingerprint_hash);
    let read = database
        .begin_read()
        .map_err(database_error("cannot begin read transaction for assessment cache"))?;
    let table = read
        .open_table(REVIEW_ASSESSMENT_CACHE)
        .map_err(database_error("cannot open review assessment cache table"))?;
    match table
        .get(key.as_str())
        .map_err(database_error("cannot read cached assessment"))?
    {
        Some(value) => {
            let record: CachedAssessmentRecord = decode_record(value.value())?;
            record.validate()?;
            if record.is_expired(now_millis) {
                return Ok(None);
            }
            if record.fingerprint == *fingerprint {
                Ok(Some(record))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

pub(crate) fn db_get_cached_assessment_by_hash(
    database: &Database,
    target: &TargetId,
    policy: &PolicyId,
    fingerprint_hash: &ContentHash,
    now_millis: u64,
) -> Result<Option<CachedAssessmentRecord>, ArgusError> {
    let key = review_assessment_cache_key(target, policy, fingerprint_hash);
    let read = database
        .begin_read()
        .map_err(database_error("cannot begin read transaction for assessment cache"))?;
    let table = read
        .open_table(REVIEW_ASSESSMENT_CACHE)
        .map_err(database_error("cannot open review assessment cache table"))?;
    match table
        .get(key.as_str())
        .map_err(database_error("cannot read cached assessment"))?
    {
        Some(value) => {
            let record: CachedAssessmentRecord = decode_record(value.value())?;
            record.validate()?;
            if record.is_expired(now_millis) {
                return Ok(None);
            }
            Ok(Some(record))
        }
        None => Ok(None),
    }
}

pub(crate) fn db_touch_cached_assessment(
    database: &Database,
    target: &TargetId,
    policy: &PolicyId,
    fingerprint_hash: &ContentHash,
    now_millis: u64,
) -> Result<bool, ArgusError> {
    let key = review_assessment_cache_key(target, policy, fingerprint_hash);
    let write = database
        .begin_write()
        .map_err(database_error("cannot begin write transaction to touch assessment cache"))?;
    let modified = {
        let mut table = write
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        let existing = table
            .get(key.as_str())
            .map_err(database_error("cannot read cached assessment"))?
            .map(|value| decode_record::<CachedAssessmentRecord>(value.value()))
            .transpose()?;
        match existing {
            Some(mut record) => {
                record.last_accessed_at_millis = now_millis;
                record.hit_count = record.hit_count.saturating_add(1);
                let bytes = encode_record(&record)?;
                table
                    .insert(key.as_str(), bytes.as_slice())
                    .map_err(database_error("cannot update cached assessment"))?;
                true
            }
            None => false,
        }
    };
    if modified {
        write
            .commit()
            .map_err(database_error("cannot commit touched cached assessment"))?;
    }
    Ok(modified)
}

pub(crate) fn db_prune_expired_assessments(
    database: &Database,
    now_millis: u64,
) -> Result<usize, ArgusError> {
    let write = database
        .begin_write()
        .map_err(database_error("cannot begin write transaction to prune expired assessments"))?;
    let removed = {
        let mut table = write
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        let mut keys_to_remove = Vec::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan review assessment cache"))?
        {
            let (key, value) = entry.map_err(database_error("cannot read assessment cache entry"))?;
            let record: CachedAssessmentRecord = decode_record(value.value())?;
            if record.is_expired(now_millis) {
                keys_to_remove.push(key.value().to_owned());
            }
        }
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            table
                .remove(key.as_str())
                .map_err(database_error("cannot remove expired cached assessment"))?;
        }
        count
    };
    write
        .commit()
        .map_err(database_error("cannot commit pruned expired assessments"))?;
    Ok(removed)
}

pub(crate) fn db_prune_invalidated_targets(
    database: &Database,
    targets: &[TargetId],
) -> Result<usize, ArgusError> {
    if targets.is_empty() {
        return Ok(0);
    }
    let target_set: HashSet<&str> = targets.iter().map(TargetId::as_str).collect();
    let write = database
        .begin_write()
        .map_err(database_error("cannot begin write transaction to prune invalidated targets"))?;
    let removed = {
        let mut table = write
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        let mut keys_to_remove = Vec::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan review assessment cache"))?
        {
            let (key, _) = entry.map_err(database_error("cannot read assessment cache entry"))?;
            let key_str = key.value();
            if let Some(target_part) = key_str.split('\0').next() {
                if target_set.contains(target_part) {
                    keys_to_remove.push(key_str.to_owned());
                }
            }
        }
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            table
                .remove(key.as_str())
                .map_err(database_error("cannot remove invalidated cached assessment"))?;
        }
        count
    };
    write
        .commit()
        .map_err(database_error("cannot commit pruned invalidated targets"))?;
    Ok(removed)
}

pub(crate) fn db_prune_invalidated_target_policy(
    database: &Database,
    target: &TargetId,
    policy: &PolicyId,
) -> Result<usize, ArgusError> {
    let prefix = format!("{}\0{}\0", target.as_str(), policy.as_str());
    let write = database.begin_write().map_err(database_error(
        "cannot begin write transaction to prune target policy",
    ))?;
    let removed = {
        let mut table = write
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        let mut keys_to_remove = Vec::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan review assessment cache"))?
        {
            let (key, _) = entry.map_err(database_error("cannot read assessment cache entry"))?;
            let key_str = key.value();
            if key_str.starts_with(&prefix) {
                keys_to_remove.push(key_str.to_owned());
            }
        }
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            table
                .remove(key.as_str())
                .map_err(database_error("cannot remove invalidated cached assessment"))?;
        }
        count
    };
    write
        .commit()
        .map_err(database_error("cannot commit pruned target policy"))?;
    Ok(removed)
}

pub(crate) fn db_prune_by_filter<F>(
    database: &Database,
    should_remove: F,
) -> Result<usize, ArgusError>
where
    F: Fn(&CachedAssessmentRecord) -> bool,
{
    let write = database
        .begin_write()
        .map_err(database_error("cannot begin write transaction to prune by filter"))?;
    let removed = {
        let mut table = write
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        let mut keys_to_remove = Vec::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan review assessment cache"))?
        {
            let (key, value) = entry.map_err(database_error("cannot read assessment cache entry"))?;
            let record: CachedAssessmentRecord = decode_record(value.value())?;
            if should_remove(&record) {
                keys_to_remove.push(key.value().to_owned());
            }
        }
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            table
                .remove(key.as_str())
                .map_err(database_error("cannot remove filtered cached assessment"))?;
        }
        count
    };
    write
        .commit()
        .map_err(database_error("cannot commit pruned by filter"))?;
    Ok(removed)
}

pub(crate) fn db_cached_assessment_count(database: &Database) -> Result<usize, ArgusError> {
    let read = database
        .begin_read()
        .map_err(database_error("cannot begin read transaction for count"))?;
    let table = read
        .open_table(REVIEW_ASSESSMENT_CACHE)
        .map_err(database_error("cannot open review assessment cache table"))?;
    let mut count = 0;
    for entry in table
        .iter()
        .map_err(database_error("cannot scan review assessment cache"))?
    {
        let _ = entry.map_err(database_error("cannot read entry"))?;
        count += 1;
    }
    Ok(count)
}

pub(crate) fn db_all_cached_assessments(
    database: &Database,
) -> Result<Vec<CachedAssessmentRecord>, ArgusError> {
    let read = database
        .begin_read()
        .map_err(database_error("cannot begin read transaction for all assessments"))?;
    let table = read
        .open_table(REVIEW_ASSESSMENT_CACHE)
        .map_err(database_error("cannot open review assessment cache table"))?;
    let mut records = Vec::new();
    for entry in table
        .iter()
        .map_err(database_error("cannot scan review assessment cache"))?
    {
        let (_, value) = entry.map_err(database_error("cannot read entry"))?;
        let record: CachedAssessmentRecord = decode_record(value.value())?;
        records.push(record);
    }
    Ok(records)
}

pub(crate) fn db_cached_assessment_stats(
    database: &Database,
    now_millis: u64,
) -> Result<CachedAssessmentStats, ArgusError> {
    let read = database
        .begin_read()
        .map_err(database_error("cannot begin read transaction for stats"))?;
    let table = read
        .open_table(REVIEW_ASSESSMENT_CACHE)
        .map_err(database_error("cannot open review assessment cache table"))?;
    let mut stats = CachedAssessmentStats::default();
    for entry in table
        .iter()
        .map_err(database_error("cannot scan review assessment cache"))?
    {
        let (_, value) = entry.map_err(database_error("cannot read entry"))?;
        let record: CachedAssessmentRecord = decode_record(value.value())?;
        stats.total_entries += 1;
        stats.total_hits = stats.total_hits.saturating_add(record.hit_count);
        stats.total_payload_bytes = stats
            .total_payload_bytes
            .saturating_add(record.payload.len() as u64);

        if record.is_expired(now_millis) {
            stats.expired_entries += 1;
        } else {
            stats.active_entries += 1;
        }

        stats.oldest_cached_millis = match stats.oldest_cached_millis {
            Some(oldest) => Some(oldest.min(record.cached_at_millis)),
            None => Some(record.cached_at_millis),
        };
        stats.newest_cached_millis = match stats.newest_cached_millis {
            Some(newest) => Some(newest.max(record.cached_at_millis)),
            None => Some(record.cached_at_millis),
        };
    }
    Ok(stats)
}

pub(crate) fn db_clear_cached_assessments(database: &Database) -> Result<usize, ArgusError> {
    let write = database
        .begin_write()
        .map_err(database_error("cannot begin write transaction to clear cache"))?;
    let removed = {
        let mut table = write
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        let mut keys_to_remove = Vec::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan review assessment cache"))?
        {
            let (key, _) = entry.map_err(database_error("cannot read entry"))?;
            keys_to_remove.push(key.value().to_owned());
        }
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            table
                .remove(key.as_str())
                .map_err(database_error("cannot remove cached assessment"))?;
        }
        count
    };
    write
        .commit()
        .map_err(database_error("cannot commit clear cache"))?;
    Ok(removed)
}

fn encode_record<T: Serialize>(value: &T) -> Result<Vec<u8>, ArgusError> {
    serde_json::to_vec(value).map_err(|error| {
        ArgusError::invariant("cannot serialize cached assessment record").with_source(error)
    })
}

fn decode_record<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ArgusError> {
    serde_json::from_slice(bytes).map_err(|error| {
        ArgusError::invariant("cannot deserialize cached assessment record").with_source(error)
    })
}

fn database_error<E>(message: &'static str) -> impl FnOnce(E) -> ArgusError
where
    E: std::error::Error + Send + Sync + 'static,
{
    move |error| ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> ArgusError {
    database_error(message)
}
