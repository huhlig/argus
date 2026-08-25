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
pub fn seeded_documentation_fixture() -> SeededDocumentationFixture {
    let missing_errors = seeded_target("missing-errors");
    let inaccurate_behavior = seeded_target("inaccurate-behavior");
    let known_clean = seeded_target("known-clean");
    SeededDocumentationFixture {
        corpus: DocumentationEvaluationCorpus {
            schema_version: DOCUMENTATION_CORPUS_SCHEMA_VERSION,
            name: "argus-seeded-documentation".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "documentation-public-api@1".to_owned(),
            expected_issues: vec![
                ExpectedDocumentationIssue {
                    id: "missing-errors".to_owned(),
                    target: missing_errors.clone(),
                    dimensions: BTreeSet::from([DocumentationDimension::Errors]),
                },
                ExpectedDocumentationIssue {
                    id: "inaccurate-behavior".to_owned(),
                    target: inaccurate_behavior.clone(),
                    dimensions: BTreeSet::from([
                        DocumentationDimension::Behavior,
                        DocumentationDimension::Accuracy,
                    ]),
                },
            ],
            known_clean_targets: vec![known_clean.clone()],
        },
        sources: vec![
            SeededDocumentationSource {
                target: missing_errors,
                logical_name: "missing_errors",
                source: r#"/// Sends the request and returns its response.
pub fn send() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("seeded failure"))
}
"#,
            },
            SeededDocumentationSource {
                target: inaccurate_behavior,
                logical_name: "inaccurate_behavior",
                source: r"/// Returns the number of bytes without modifying the input.
pub fn normalize(input: &mut Vec<u8>) -> usize {
    input.clear();
    input.len()
}
",
            },
            SeededDocumentationSource {
                target: known_clean,
                logical_name: "known_clean",
                source: r"/// Returns `true` when `input` contains no bytes.
#[must_use]
pub fn is_empty(input: &[u8]) -> bool {
    input.is_empty()
}
",
            },
        ],
    }
}

fn seeded_target(name: &str) -> TargetId {
    TargetId::derive([b"argus-seeded-documentation-v1".as_slice(), name.as_bytes()])
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
