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
use argus_policies::DocumentationDimension;
use argus_report::{
    DOCUMENTATION_CORPUS_SCHEMA_VERSION, DocumentationEvaluationCorpus, ExpectedDocumentationIssue,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededDocumentationSource {
    pub target: TargetId,
    pub logical_name: &'static str,
    pub source: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededDocumentationFixture {
    pub corpus: DocumentationEvaluationCorpus,
    pub sources: Vec<SeededDocumentationSource>,
}

/// Versioned, deterministic documentation corpus containing defects and known-clean controls.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn seeded_documentation_fixture() -> SeededDocumentationFixture {
    let missing_presence = seeded_target("missing-presence");
    let unclear_purpose = seeded_target("unclear-purpose");
    let missing_behavior = seeded_target("missing-behavior");
    let missing_inputs = seeded_target("missing-inputs");
    let missing_outputs = seeded_target("missing-outputs");
    let missing_errors = seeded_target("missing-errors");
    let missing_panics = seeded_target("missing-panics");
    let missing_safety = seeded_target("missing-safety");
    let undocumented_side_effects = seeded_target("undocumented-side-effects");
    let undocumented_invariants = seeded_target("undocumented-invariants");
    let misleading_examples = seeded_target("misleading-examples");
    let inaccurate_behavior = seeded_target("inaccurate-behavior");
    let obsolete_currency = seeded_target("obsolete-currency");
    let vacuous_tautology = seeded_target("vacuous-tautology");

    let known_clean = seeded_target("known-clean");
    let known_clean_unsafe = seeded_target("known-clean-unsafe");
    let known_clean_error = seeded_target("known-clean-error");

    SeededDocumentationFixture {
        corpus: DocumentationEvaluationCorpus {
            schema_version: DOCUMENTATION_CORPUS_SCHEMA_VERSION,
            name: "argus-seeded-documentation".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "documentation-public-api@1".to_owned(),
            expected_issues: vec![
                ExpectedDocumentationIssue {
                    id: "missing-presence".to_owned(),
                    target: missing_presence.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Presence]),
                },
                ExpectedDocumentationIssue {
                    id: "unclear-purpose".to_owned(),
                    target: unclear_purpose.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Purpose]),
                },
                ExpectedDocumentationIssue {
                    id: "missing-behavior".to_owned(),
                    target: missing_behavior.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Behavior]),
                },
                ExpectedDocumentationIssue {
                    id: "missing-inputs".to_owned(),
                    target: missing_inputs.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Inputs]),
                },
                ExpectedDocumentationIssue {
                    id: "missing-outputs".to_owned(),
                    target: missing_outputs.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Outputs]),
                },
                ExpectedDocumentationIssue {
                    id: "missing-errors".to_owned(),
                    target: missing_errors.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Errors]),
                },
                ExpectedDocumentationIssue {
                    id: "missing-panics".to_owned(),
                    target: missing_panics.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Panics]),
                },
                ExpectedDocumentationIssue {
                    id: "missing-safety".to_owned(),
                    target: missing_safety.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Safety]),
                },
                ExpectedDocumentationIssue {
                    id: "undocumented-side-effects".to_owned(),
                    target: undocumented_side_effects.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::SideEffects]),
                },
                ExpectedDocumentationIssue {
                    id: "undocumented-invariants".to_owned(),
                    target: undocumented_invariants.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Invariants]),
                },
                ExpectedDocumentationIssue {
                    id: "misleading-examples".to_owned(),
                    target: misleading_examples.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Examples]),
                },
                ExpectedDocumentationIssue {
                    id: "inaccurate-behavior".to_owned(),
                    target: inaccurate_behavior.clone(),
                    dimensions: BTreeSet::from([
                        DocumentationDimension::Behavior,
                        DocumentationDimension::Accuracy,
                    ]),
                },
                ExpectedDocumentationIssue {
                    id: "obsolete-currency".to_owned(),
                    target: obsolete_currency.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Currency]),
                },
                ExpectedDocumentationIssue {
                    id: "vacuous-tautology".to_owned(),
                    target: vacuous_tautology.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Value]),
                },
            ],
            known_clean_targets: vec![
                known_clean.clone(),
                known_clean_unsafe.clone(),
                known_clean_error.clone(),
            ],
        },
        sources: vec![
            SeededDocumentationSource {
                target: missing_presence,
                logical_name: "missing_presence",
                source: r"#[must_use]
pub fn missing_presence(x: u32) -> u32 {
    x.saturating_add(1)
}
",
            },
            SeededDocumentationSource {
                target: unclear_purpose,
                logical_name: "unclear_purpose",
                source: r"/// Helper function.
#[must_use]
pub fn unclear_purpose(items: &[u8]) -> usize {
    items.len()
}
",
            },
            SeededDocumentationSource {
                target: missing_behavior,
                logical_name: "missing_behavior",
                source: r"/// Processes the bytes.
#[must_use]
pub fn missing_behavior(input: &[u8]) -> Vec<u8> {
    input.iter().copied().filter(|b| b % 2 == 0).collect()
}
",
            },
            SeededDocumentationSource {
                target: missing_inputs,
                logical_name: "missing_inputs",
                source: r"/// Computes an offset within the buffer.
#[must_use]
pub fn missing_inputs(buffer_len: usize, index: usize, stride: usize) -> usize {
    (index * stride).min(buffer_len)
}
",
            },
            SeededDocumentationSource {
                target: missing_outputs,
                logical_name: "missing_outputs",
                source: r"/// Evaluates telemetry and outputs status.
#[must_use]
pub fn missing_outputs(rate: f64) -> (bool, u32) {
    (rate > 0.5, (rate * 100.0) as u32)
}
",
            },
            SeededDocumentationSource {
                target: missing_errors,
                logical_name: "missing_errors",
                source: r#"/// Sends the request and returns its response.
pub fn missing_errors() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("seeded failure"))
}
"#,
            },
            SeededDocumentationSource {
                target: missing_panics,
                logical_name: "missing_panics",
                source: r#"/// Divides `numerator` by `denominator`.
#[must_use]
pub fn missing_panics(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        panic!("denominator must not be zero");
    }
    numerator / denominator
}
"#,
            },
            SeededDocumentationSource {
                target: missing_safety,
                logical_name: "missing_safety",
                source: r"/// Dereferences the raw pointer.
///
/// # Returns
///
/// The byte value.
#[must_use]
pub unsafe fn missing_safety(ptr: *const u8) -> u8 {
    *ptr
}
",
            },
            SeededDocumentationSource {
                target: undocumented_side_effects,
                logical_name: "undocumented_side_effects",
                source: r"static mut COUNTER: u64 = 0;

/// Returns the current generation number.
#[must_use]
pub fn undocumented_side_effects() -> u64 {
    unsafe {
        COUNTER += 1;
        COUNTER
    }
}
",
            },
            SeededDocumentationSource {
                target: undocumented_invariants,
                logical_name: "UndocumentedInvariants",
                source: r"/// A bounded non-empty byte slice wrapper.
pub struct UndocumentedInvariants {
    /// Inner payload.
    pub data: Vec<u8>,
}
",
            },
            SeededDocumentationSource {
                target: misleading_examples,
                logical_name: "misleading_examples",
                source: r"/// Doubles the input number.
///
/// ```
/// let res = 100;
/// assert_eq!(res, 100);
/// ```
#[must_use]
pub fn misleading_examples(x: i32) -> i32 {
    x * 2
}
",
            },
            SeededDocumentationSource {
                target: inaccurate_behavior,
                logical_name: "inaccurate_behavior",
                source: r"/// Returns the number of bytes without modifying the input.
pub fn inaccurate_behavior(input: &mut Vec<u8>) -> usize {
    input.clear();
    input.len()
}
",
            },
            SeededDocumentationSource {
                target: obsolete_currency,
                logical_name: "obsolete_currency",
                source: r"/// Parses the packet given `payload_length` and timeout.
#[must_use]
pub fn obsolete_currency(bytes: &[u8]) -> usize {
    bytes.len()
}
",
            },
            SeededDocumentationSource {
                target: vacuous_tautology,
                logical_name: "vacuous_tautology",
                source: r"/// Performs computation.
#[must_use]
pub fn vacuous_tautology(val: u32) -> u32 {
    val.rotate_left(3)
}
",
            },
            SeededDocumentationSource {
                target: known_clean,
                logical_name: "known_clean",
                source: r"/// Returns `true` when `input` contains no bytes.
#[must_use]
pub fn known_clean(input: &[u8]) -> bool {
    input.is_empty()
}
",
            },
            SeededDocumentationSource {
                target: known_clean_unsafe,
                logical_name: "known_clean_unsafe",
                source: r"/// Reads the byte from `ptr`.
///
/// # Safety
///
/// `ptr` must be non-null and valid for reads of 1 byte.
#[must_use]
pub unsafe fn known_clean_unsafe(ptr: *const u8) -> u8 {
    *ptr
}
",
            },
            SeededDocumentationSource {
                target: known_clean_error,
                logical_name: "known_clean_error",
                source: r#"/// Opens and validates the configuration file.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if the path is empty.
pub fn known_clean_error(path: &str) -> Result<String, std::io::Error> {
    if path.is_empty() {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty path"))
    } else {
        Ok(path.to_owned())
    }
}
"#,
            },
        ],
    }
}

fn seeded_target(name: &str) -> TargetId {
    let (kind, logical_name) = match name {
        "missing-presence" => ("callable", "missing_presence"),
        "unclear-purpose" => ("callable", "unclear_purpose"),
        "missing-behavior" => ("callable", "missing_behavior"),
        "missing-inputs" => ("callable", "missing_inputs"),
        "missing-outputs" => ("callable", "missing_outputs"),
        "missing-errors" => ("callable", "missing_errors"),
        "missing-panics" => ("callable", "missing_panics"),
        "missing-safety" => ("callable", "missing_safety"),
        "undocumented-side-effects" => ("callable", "undocumented_side_effects"),
        "undocumented-invariants" => ("type", "UndocumentedInvariants"),
        "misleading-examples" => ("callable", "misleading_examples"),
        "inaccurate-behavior" => ("callable", "inaccurate_behavior"),
        "obsolete-currency" => ("callable", "obsolete_currency"),
        "vacuous-tautology" => ("callable", "vacuous_tautology"),
        "known-clean" => ("callable", "known_clean"),
        "known-clean-unsafe" => ("callable", "known_clean_unsafe"),
        "known-clean-error" => ("callable", "known_clean_error"),
        _ => unreachable!("unknown seeded documentation target"),
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
    fn corpus_is_valid_and_every_ground_truth_target_has_source() {
        let fixture = seeded_documentation_fixture();
        fixture.corpus.validate().unwrap();
        let checked_in: DocumentationEvaluationCorpus = serde_json::from_str(include_str!(
            "../../../docs/evaluation/documentation-corpus-v1.json"
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
