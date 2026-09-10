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
use argus_policies::ArchitectureDimension;
use argus_report::{
    ARCHITECTURE_CORPUS_SCHEMA_VERSION, ArchitectureEvaluationCorpus, ExpectedArchitectureIssue,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededArchitectureSource {
    pub target: TargetId,
    pub logical_name: &'static str,
    pub source: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededArchitectureFixture {
    pub corpus: ArchitectureEvaluationCorpus,
    pub sources: Vec<SeededArchitectureSource>,
}

/// Versioned, deterministic architecture corpus containing structural defects and clean controls.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn seeded_architecture_fixture() -> SeededArchitectureFixture {
    let cyclic_dependency = seeded_target("cyclic_a");
    let layering_violation = seeded_target("layering_violation");
    let leaky_abstraction = seeded_target("leaky_abstraction");
    let low_cohesion = seeded_target("low_cohesion");
    let bypassed_boundary = seeded_target("bypassed_boundary");
    let pattern_dissonance = seeded_target("pattern_dissonance");

    let clean_layered_subsystem = seeded_target("clean_layered_subsystem");
    let clean_cohesive_module = seeded_target("clean_cohesive_module");

    SeededArchitectureFixture {
        corpus: ArchitectureEvaluationCorpus {
            schema_version: ARCHITECTURE_CORPUS_SCHEMA_VERSION,
            name: "argus-seeded-architecture".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "architecture-code-derived@1".to_owned(),
            expected_issues: vec![
                ExpectedArchitectureIssue {
                    id: "cyclic-dependency".to_owned(),
                    target: cyclic_dependency.clone(),
                    dimensions: BTreeSet::from([
                        ArchitectureDimension::Cycles,
                        ArchitectureDimension::DependencyStructure,
                    ]),
                },
                ExpectedArchitectureIssue {
                    id: "layering-violation".to_owned(),
                    target: layering_violation.clone(),
                    dimensions: BTreeSet::from([
                        ArchitectureDimension::DependencyStructure,
                        ArchitectureDimension::BoundaryAnalysis,
                    ]),
                },
                ExpectedArchitectureIssue {
                    id: "leaky-abstraction".to_owned(),
                    target: leaky_abstraction.clone(),
                    dimensions: BTreeSet::from([ArchitectureDimension::PublicSurface]),
                },
                ExpectedArchitectureIssue {
                    id: "low-cohesion".to_owned(),
                    target: low_cohesion.clone(),
                    dimensions: BTreeSet::from([ArchitectureDimension::OwnershipAndCohesion]),
                },
                ExpectedArchitectureIssue {
                    id: "bypassed-boundary".to_owned(),
                    target: bypassed_boundary.clone(),
                    dimensions: BTreeSet::from([ArchitectureDimension::BoundaryAnalysis]),
                },
                ExpectedArchitectureIssue {
                    id: "pattern-dissonance".to_owned(),
                    target: pattern_dissonance.clone(),
                    dimensions: BTreeSet::from([ArchitectureDimension::PatternConsistency]),
                },
            ],
            known_clean_targets: vec![
                clean_layered_subsystem.clone(),
                clean_cohesive_module.clone(),
            ],
        },
        sources: vec![
            SeededArchitectureSource {
                target: cyclic_dependency,
                logical_name: "cyclic_a",
                source: r"pub mod cyclic_a {
    pub fn step_a(count: u32) -> u32 {
        if count == 0 {
            0
        } else {
            crate::cyclic_b::step_b(count - 1)
        }
    }
}
",
            },
            SeededArchitectureSource {
                target: layering_violation,
                logical_name: "layering_violation",
                source: r#"pub mod layering_violation {
    pub struct LowLevelStorage {
        pub data: Vec<u8>,
    }

    impl LowLevelStorage {
        pub fn persist(&mut self, item: u8) {
            self.data.push(item);
            crate::presentation::render_alert("persisted byte");
        }
    }
}
"#,
            },
            SeededArchitectureSource {
                target: leaky_abstraction,
                logical_name: "leaky_abstraction",
                source: r"pub mod leaky_abstraction {
    pub struct NetworkSocketPool {
        pub raw_file_descriptors: Vec<i32>,
        pub internal_kernel_pointer: *mut u8,
    }

    impl NetworkSocketPool {
        #[must_use]
        pub fn new() -> Self {
            Self {
                raw_file_descriptors: Vec::new(),
                internal_kernel_pointer: std::ptr::null_mut(),
            }
        }
    }

    impl Default for NetworkSocketPool {
        fn default() -> Self {
            Self::new()
        }
    }
}
",
            },
            SeededArchitectureSource {
                target: low_cohesion,
                logical_name: "low_cohesion",
                source: r"pub mod low_cohesion {
    pub struct GodComponent {
        pub session_token: String,
        pub image_buffer: Vec<u8>,
        pub tax_rate_basis_points: u16,
    }

    impl GodComponent {
        pub fn authenticate(&mut self, token: &str) -> bool {
            self.session_token = token.to_owned();
            true
        }

        pub fn resize_image(&mut self, factor: usize) {
            self.image_buffer.truncate(self.image_buffer.len() / factor.max(1));
        }

        #[must_use]
        pub fn compute_sales_tax(&self, cents: u64) -> u64 {
            (cents * u64::from(self.tax_rate_basis_points)) / 10_000
        }
    }
}
",
            },
            SeededArchitectureSource {
                target: bypassed_boundary,
                logical_name: "bypassed_boundary",
                source: r#"pub mod bypassed_boundary {
    pub fn direct_disk_mutation() {
        let _ = std::fs::write("raw_table.bin", b"uncoordinated_write");
    }
}
"#,
            },
            SeededArchitectureSource {
                target: pattern_dissonance,
                logical_name: "pattern_dissonance",
                source: r#"pub mod pattern_dissonance {
    pub fn fetch_user_record(id: u64) -> Option<String> {
        if id == 0 {
            panic!("fatal pattern violation: panicking instead of returning Option or Result");
        }
        Some(format!("user_{id}"))
    }
}
"#,
            },
            SeededArchitectureSource {
                target: clean_layered_subsystem,
                logical_name: "clean_layered_subsystem",
                source: r"pub mod clean_layered_subsystem {
    pub struct DataStore {
        items: Vec<String>,
    }

    impl DataStore {
        #[must_use]
        pub fn new() -> Self {
            Self { items: Vec::new() }
        }

        pub fn insert(&mut self, item: String) {
            self.items.push(item);
        }
    }

    impl Default for DataStore {
        fn default() -> Self {
            Self::new()
        }
    }

    pub struct BusinessLogic {
        store: DataStore,
    }

    impl BusinessLogic {
        #[must_use]
        pub fn new(store: DataStore) -> Self {
            Self { store }
        }

        pub fn add_entry(&mut self, item: String) {
            self.store.insert(item);
        }
    }
}
",
            },
            SeededArchitectureSource {
                target: clean_cohesive_module,
                logical_name: "clean_cohesive_module",
                source: r"pub mod clean_cohesive_module {
    pub struct TokenBucketRateLimiter {
        capacity: u32,
        tokens: u32,
    }

    impl TokenBucketRateLimiter {
        #[must_use]
        pub fn new(capacity: u32) -> Self {
            Self { capacity, tokens: capacity }
        }

        pub fn try_acquire(&mut self) -> bool {
            if self.tokens > 0 {
                self.tokens -= 1;
                true
            } else {
                false
            }
        }
    }
}
",
            },
        ],
    }
}

fn seeded_target(name: &str) -> TargetId {
    TargetId::derive([b"argus:seeded-architecture:v1", name.as_bytes()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architecture_corpus_is_valid_and_every_ground_truth_target_has_source() {
        let fixture = seeded_architecture_fixture();
        fixture.corpus.validate().unwrap();

        let source_targets = fixture
            .sources
            .iter()
            .map(|s| s.target.clone())
            .collect::<BTreeSet<_>>();

        for issue in &fixture.corpus.expected_issues {
            assert!(
                source_targets.contains(&issue.target),
                "expected architecture issue {} has no fixture source",
                issue.id
            );
        }

        for target in &fixture.corpus.known_clean_targets {
            assert!(
                source_targets.contains(target),
                "known-clean target {target} has no fixture source"
            );
        }

        let repo_corpus_bytes =
            include_bytes!("../../../docs/evaluation/architecture-corpus-v1.json");
        let repo_corpus: ArchitectureEvaluationCorpus =
            serde_json::from_slice(repo_corpus_bytes).unwrap();
        assert_eq!(
            fixture.corpus, repo_corpus,
            "in-memory architecture fixture drifted from checked-in architecture-corpus-v1.json"
        );
    }
}
