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

use argus_core::TargetId;
use argus_policies::CorrectnessDimension;
use argus_report::{
    CORRECTNESS_CORPUS_SCHEMA_VERSION, CorrectnessEvaluationCorpus, ExpectedCorrectnessIssue,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededCorrectnessSource {
    pub target: TargetId,
    pub logical_name: &'static str,
    pub source: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededCorrectnessFixture {
    pub corpus: CorrectnessEvaluationCorpus,
    pub sources: Vec<SeededCorrectnessSource>,
}

/// Versioned, deterministic correctness corpus containing defects and known-clean controls.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn seeded_correctness_fixture() -> SeededCorrectnessFixture {
    let unhandled_failure_path = seeded_target("unhandled-failure-path");
    let broken_invariant = seeded_target("broken-invariant");
    let invalid_state_transition = seeded_target("invalid-state-transition");
    let swallowed_error = seeded_target("swallowed-error");
    let resource_leak = seeded_target("resource-leak");
    let race_condition = seeded_target("race-condition");
    let torn_persistence = seeded_target("torn-persistence");
    let unsound_pointer_assumption = seeded_target("unsound-pointer-assumption");
    let integer_overflow_boundary = seeded_target("integer-overflow-boundary");

    let known_clean_checked_math = seeded_target("known-clean-checked-math");
    let known_clean_error_handling = seeded_target("known-clean-error-handling");
    let known_clean_boundary = seeded_target("known-clean-boundary");

    SeededCorrectnessFixture {
        corpus: CorrectnessEvaluationCorpus {
            schema_version: CORRECTNESS_CORPUS_SCHEMA_VERSION,
            name: "argus-seeded-correctness".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "correctness-conservative@1".to_owned(),
            expected_issues: vec![
                ExpectedCorrectnessIssue {
                    id: "unhandled-failure-path".to_owned(),
                    target: unhandled_failure_path.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::FailurePaths]),
                },
                ExpectedCorrectnessIssue {
                    id: "broken-invariant".to_owned(),
                    target: broken_invariant.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::Invariants]),
                },
                ExpectedCorrectnessIssue {
                    id: "invalid-state-transition".to_owned(),
                    target: invalid_state_transition.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::StateTransitions]),
                },
                ExpectedCorrectnessIssue {
                    id: "swallowed-error".to_owned(),
                    target: swallowed_error.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::ErrorHandling]),
                },
                ExpectedCorrectnessIssue {
                    id: "resource-leak".to_owned(),
                    target: resource_leak.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::ResourceLifecycle]),
                },
                ExpectedCorrectnessIssue {
                    id: "race-condition".to_owned(),
                    target: race_condition.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::Concurrency]),
                },
                ExpectedCorrectnessIssue {
                    id: "torn-persistence".to_owned(),
                    target: torn_persistence.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::Persistence]),
                },
                ExpectedCorrectnessIssue {
                    id: "unsound-pointer-assumption".to_owned(),
                    target: unsound_pointer_assumption.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::UnsafeAssumptions]),
                },
                ExpectedCorrectnessIssue {
                    id: "integer-overflow-boundary".to_owned(),
                    target: integer_overflow_boundary.clone(),
                    dimensions: BTreeSet::from([CorrectnessDimension::BoundaryConditions]),
                },
            ],
            known_clean_targets: vec![
                known_clean_checked_math.clone(),
                known_clean_error_handling.clone(),
                known_clean_boundary.clone(),
            ],
        },
        sources: vec![
            SeededCorrectnessSource {
                target: unhandled_failure_path,
                logical_name: "unhandled_failure_path",
                source: r"pub fn unhandled_failure_path(file: &str, data: &[u8]) -> usize {
    if file.is_empty() {
        return data.len();
    }
    data.len()
}
",
            },
            SeededCorrectnessSource {
                target: broken_invariant,
                logical_name: "NonEmptySortedList",
                source: r"pub struct NonEmptySortedList {
    items: Vec<i32>,
}
",
            },
            SeededCorrectnessSource {
                target: invalid_state_transition,
                logical_name: "Session",
                source: r"pub struct Session {
    pub state: SessionState,
}
",
            },
            SeededCorrectnessSource {
                target: swallowed_error,
                logical_name: "swallowed_error",
                source: r#"pub fn swallowed_error(path: &str) -> bool {
    let result: Result<String, std::io::Error> = if path.is_empty() {
        Err(std::io::Error::other("invalid path"))
    } else {
        Ok(path.to_owned())
    };
    let _ = result;
    true
}
"#,
            },
            SeededCorrectnessSource {
                target: resource_leak,
                logical_name: "ResourceHandle",
                source: r"pub struct ResourceHandle {
    pub id: u64,
    pub is_open: bool,
}
",
            },
            SeededCorrectnessSource {
                target: race_condition,
                logical_name: "race_condition_increment",
                source: r"static mut GLOBAL_SHARED_COUNTER: u64 = 0;

pub fn race_condition_increment() -> u64 {
    unsafe {
        let current = GLOBAL_SHARED_COUNTER;
        GLOBAL_SHARED_COUNTER = current + 1;
        GLOBAL_SHARED_COUNTER
    }
}
",
            },
            SeededCorrectnessSource {
                target: torn_persistence,
                logical_name: "torn_persistence",
                source: r#"pub fn torn_persistence(target_path: &str, content: &[u8]) -> Result<(), std::io::Error> {
    if target_path.is_empty() {
        return Err(std::io::Error::other("empty path"));
    }
    let _ = content;
    Ok(())
}
"#,
            },
            SeededCorrectnessSource {
                target: unsound_pointer_assumption,
                logical_name: "unsound_pointer_assumption",
                source: r"pub unsafe fn unsound_pointer_assumption(bytes: &[u8]) -> u32 {
    let ptr = bytes.as_ptr().cast::<u32>();
    *ptr
}
",
            },
            SeededCorrectnessSource {
                target: integer_overflow_boundary,
                logical_name: "integer_overflow_boundary",
                source: r"#[must_use]
pub fn integer_overflow_boundary(a: u32, b: u32) -> u32 {
    a * b + 100
}
",
            },
            SeededCorrectnessSource {
                target: known_clean_checked_math,
                logical_name: "known_clean_checked_math",
                source: r"#[must_use]
pub fn known_clean_checked_math(a: u32, b: u32) -> Option<u32> {
    a.checked_mul(b)?.checked_add(100)
}
",
            },
            SeededCorrectnessSource {
                target: known_clean_error_handling,
                logical_name: "known_clean_error_handling",
                source: r#"pub fn known_clean_error_handling(path: &str) -> Result<String, std::io::Error> {
    if path.is_empty() {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "path cannot be empty"))
    } else {
        Ok(path.to_owned())
    }
}
"#,
            },
            SeededCorrectnessSource {
                target: known_clean_boundary,
                logical_name: "known_clean_boundary",
                source: r"#[must_use]
pub fn known_clean_boundary(items: &[u8], index: usize) -> Option<u8> {
    items.get(index).copied()
}
",
            },
        ],
    }
}

fn seeded_target(name: &str) -> TargetId {
    let (kind, logical_name) = match name {
        "unhandled-failure-path" => ("callable", "unhandled_failure_path"),
        "broken-invariant" => ("type", "NonEmptySortedList"),
        "invalid-state-transition" => ("type", "Session"),
        "swallowed-error" => ("callable", "swallowed_error"),
        "resource-leak" => ("type", "ResourceHandle"),
        "race-condition" => ("callable", "race_condition_increment"),
        "torn-persistence" => ("callable", "torn_persistence"),
        "unsound-pointer-assumption" => ("callable", "unsound_pointer_assumption"),
        "integer-overflow-boundary" => ("callable", "integer_overflow_boundary"),
        "known-clean-checked-math" => ("callable", "known_clean_checked_math"),
        "known-clean-error-handling" => ("callable", "known_clean_error_handling"),
        "known-clean-boundary" => ("callable", "known_clean_boundary"),
        _ => unreachable!("unknown seeded correctness target"),
    };
    TargetId::derive([
        b"rust-syntax".as_slice(),
        b"src/lib.rs".as_slice(),
        kind.as_bytes(),
        logical_name.as_bytes(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correctness_corpus_is_valid_and_every_ground_truth_target_has_source() {
        let fixture = seeded_correctness_fixture();
        fixture.corpus.validate().unwrap();
        let checked_in: CorrectnessEvaluationCorpus = serde_json::from_str(include_str!(
            "../../../docs/evaluation/correctness-corpus-v1.json"
        ))
        .unwrap();
        assert_eq!(checked_in, fixture.corpus);
        let targets = fixture
            .sources
            .iter()
            .map(|source| &source.target)
            .collect::<BTreeSet<_>>();
        assert!(
            fixture
                .corpus
                .expected_issues
                .iter()
                .all(|issue| targets.contains(&issue.target))
        );
        assert!(
            fixture
                .corpus
                .known_clean_targets
                .iter()
                .all(|target| targets.contains(target))
        );
    }
}
