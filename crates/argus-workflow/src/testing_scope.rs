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

//! Language-neutral test recognition, module scopes, and test linking.
//!
//! Adapters classify tests inconsistently (only Rust reports `Test` targets), so test code
//! is recognised from adapter classification, path conventions, and containment. Tests are
//! linked to production code through `*:calls`, `*:references`, and file-level `*:imports`
//! relations. Links are a signal, not proof: dynamic dispatch, macros, and tests that drive
//! behaviour through a CLI or files leave no relation.

use argus_core::{PortableTargetKind, Relation, Target, TargetId, TargetKind};
use argus_policies::{TestingTargetClass, TestingTargetProfile};
use std::collections::{BTreeMap, BTreeSet};

/// Directory names whose contents are test code.
const TEST_DIRECTORIES: [&str; 5] = ["tests", "test", "__tests__", "spec", "testing"];
/// Directory names whose contents are benchmark code.
const BENCHMARK_DIRECTORIES: [&str; 4] = ["benches", "bench", "benchmark", "benchmarks"];

/// How a test reaches a production member.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TestLink {
    pub test: TargetId,
    /// Production caller the test reaches the member through, when indirect.
    pub via: Option<TargetId>,
}

/// Test code, module scopes, and test links derived from one inventory.
#[derive(Clone, Debug)]
pub struct TestingScope<'a> {
    targets: BTreeMap<TargetId, &'a Target>,
    test_code: BTreeSet<TargetId>,
    benchmark_code: BTreeSet<TargetId>,
    modules: BTreeMap<TargetId, Vec<TargetId>>,
    links: BTreeMap<TargetId, BTreeSet<TestLink>>,
    importers: BTreeMap<TargetId, BTreeSet<TargetId>>,
}

impl<'a> TestingScope<'a> {
    #[must_use]
    pub fn analyze(targets: &'a [Target], relations: &[Relation]) -> Self {
        let targets = targets
            .iter()
            .map(|target| (target.id.clone(), target))
            .collect::<BTreeMap<_, _>>();
        let test_code = recognise_test_code(&targets);
        let benchmark_code = test_code
            .iter()
            .filter(|id| is_benchmark(targets[*id], &targets))
            .cloned()
            .collect();
        let mut scope = Self {
            modules: BTreeMap::new(),
            links: BTreeMap::new(),
            importers: BTreeMap::new(),
            targets,
            test_code,
            benchmark_code,
        };
        scope.assign_members();
        scope.link_tests(relations);
        scope
    }

    #[must_use]
    pub fn target(&self, id: &TargetId) -> Option<&'a Target> {
        self.targets.get(id).copied()
    }

    #[must_use]
    pub fn is_test_code(&self, id: &TargetId) -> bool {
        self.test_code.contains(id)
    }

    #[must_use]
    pub fn is_benchmark(&self, id: &TargetId) -> bool {
        self.benchmark_code.contains(id)
    }

    /// Module scopes with the production members assigned to them.
    pub fn modules(&self) -> impl Iterator<Item = (&TargetId, &[TargetId])> {
        self.modules
            .iter()
            .map(|(module, members)| (module, members.as_slice()))
    }

    /// Production members assigned to one module scope.
    #[must_use]
    pub fn members(&self, module: &TargetId) -> &[TargetId] {
        self.modules.get(module).map_or(&[], Vec::as_slice)
    }

    /// Tests linked to a production member through call or reference relations.
    pub fn tests_for(&self, member: &TargetId) -> impl Iterator<Item = &TestLink> {
        self.links.get(member).into_iter().flatten()
    }

    /// Test files importing a module scope's file.
    pub fn importing_tests(&self, module: &TargetId) -> impl Iterator<Item = &TargetId> {
        self.importers.get(module).into_iter().flatten()
    }

    /// Nearest package ancestor of a target.
    #[must_use]
    pub fn package_of(&self, id: &TargetId) -> Option<&'a Target> {
        self.ancestors(id).find(|target| {
            target.kind
                == TargetKind::Portable {
                    kind: PortableTargetKind::Package,
                }
        })
    }

    /// Test-code files and test build targets, for path-based test inventories.
    pub fn test_files(&self) -> impl Iterator<Item = &'a Target> + '_ {
        self.test_code
            .iter()
            .map(|id| self.targets[id])
            .filter(|target| {
                matches!(
                    target.kind,
                    TargetKind::Portable {
                        kind: PortableTargetKind::File
                    }
                ) || matches!(&target.kind, TargetKind::LanguageSpecific { kind, .. } if kind.starts_with("cargo_target"))
            })
    }

    /// Ancestors along the `parent` chain, nearest first, stopping on cycles.
    fn ancestors(&self, id: &TargetId) -> impl Iterator<Item = &'a Target> + '_ {
        let mut seen = BTreeSet::from([id.clone()]);
        let mut next = self
            .targets
            .get(id)
            .and_then(|target| target.parent.clone());
        std::iter::from_fn(move || {
            let current = next.take()?;
            if !seen.insert(current.clone()) {
                return None;
            }
            let target = self.targets.get(&current).copied()?;
            next.clone_from(&target.parent);
            Some(target)
        })
    }

    fn assign_members(&mut self) {
        let mut modules: BTreeMap<TargetId, Vec<TargetId>> = BTreeMap::new();
        for (id, target) in &self.targets {
            if self.test_code.contains(id) || class(target) != TestingTargetClass::Member {
                continue;
            }
            let module = self
                .ancestors(id)
                .find(|ancestor| class(ancestor) == TestingTargetClass::Module)
                .filter(|module| !self.test_code.contains(&module.id));
            if let Some(module) = module {
                modules
                    .entry(module.id.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        self.modules = modules;
    }

    fn link_tests(&mut self, relations: &[Relation]) {
        let call_like = |relation: &&Relation| {
            relation.kind.ends_with(":calls") || relation.kind.ends_with(":references")
        };
        let mut direct: BTreeMap<TargetId, BTreeSet<TargetId>> = BTreeMap::new();
        for relation in relations.iter().filter(call_like) {
            if self.test_code.contains(&relation.source)
                && !self.test_code.contains(&relation.target)
            {
                direct
                    .entry(relation.target.clone())
                    .or_default()
                    .insert(relation.source.clone());
            }
        }
        let mut links: BTreeMap<TargetId, BTreeSet<TestLink>> = BTreeMap::new();
        for (member, tests) in &direct {
            links
                .entry(member.clone())
                .or_default()
                .extend(tests.iter().map(|test| TestLink {
                    test: test.clone(),
                    via: None,
                }));
        }
        for relation in relations.iter().filter(call_like) {
            let helper = &relation.source;
            if relation.source == relation.target
                || self.test_code.contains(helper)
                || self.test_code.contains(&relation.target)
            {
                continue;
            }
            if let Some(tests) = direct.get(helper) {
                links
                    .entry(relation.target.clone())
                    .or_default()
                    .extend(tests.iter().map(|test| TestLink {
                        test: test.clone(),
                        via: Some(helper.clone()),
                    }));
            }
        }
        // A direct link supersedes indirect links from the same test.
        for set in links.values_mut() {
            let direct_tests = set
                .iter()
                .filter(|link| link.via.is_none())
                .map(|link| link.test.clone())
                .collect::<BTreeSet<_>>();
            set.retain(|link| link.via.is_none() || !direct_tests.contains(&link.test));
        }
        self.links = links;

        for relation in relations
            .iter()
            .filter(|relation| relation.kind.ends_with(":imports"))
        {
            if self.test_code.contains(&relation.source)
                && self.modules.contains_key(&relation.target)
            {
                self.importers
                    .entry(relation.target.clone())
                    .or_default()
                    .insert(relation.source.clone());
            }
        }
    }
}

fn class(target: &Target) -> TestingTargetClass {
    TestingTargetProfile::from_target(target, false).class
}

fn adapter_classified_test(target: &Target) -> bool {
    match &target.kind {
        TargetKind::Portable { kind } => *kind == PortableTargetKind::Test,
        TargetKind::LanguageSpecific { kind, .. } => {
            let lower = kind.to_ascii_lowercase();
            lower.contains("test") || lower.contains("bench")
        }
    }
}

fn recognise_test_code(targets: &BTreeMap<TargetId, &Target>) -> BTreeSet<TargetId> {
    let mut test_code = targets
        .values()
        .filter(|target| {
            adapter_classified_test(target)
                || target
                    .location
                    .as_ref()
                    .is_some_and(|location| is_test_path(location.path.as_str()))
        })
        .map(|target| target.id.clone())
        .collect::<BTreeSet<_>>();
    // A module (not a file or package) holding adapter-classified tests is a test module,
    // which catches helpers and attribute-macro tests beside `#[test]` functions.
    for target in targets.values() {
        if let Some(parent) = target.parent.as_ref().and_then(|id| targets.get(id)) {
            let is_inner_module = matches!(
                parent.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Module
                }
            ) || matches!(&parent.kind, TargetKind::LanguageSpecific { kind, .. } if kind.to_ascii_lowercase().contains("module"));
            if is_inner_module && adapter_classified_test(target) {
                test_code.insert(parent.id.clone());
            }
        }
    }
    // Anything whose parent chain passes through test code is test code.
    let descendants = targets
        .values()
        .filter(|target| {
            let mut seen = BTreeSet::new();
            let mut next = target.parent.clone();
            while let Some(current) = next {
                if !seen.insert(current.clone()) {
                    return false;
                }
                if test_code.contains(&current) {
                    return true;
                }
                next = targets
                    .get(&current)
                    .and_then(|parent| parent.parent.clone());
            }
            false
        })
        .map(|target| target.id.clone())
        .collect::<Vec<_>>();
    test_code.extend(descendants);
    test_code
}

fn is_benchmark(target: &Target, targets: &BTreeMap<TargetId, &Target>) -> bool {
    let kind_says_bench = |target: &Target| matches!(&target.kind, TargetKind::LanguageSpecific { kind, .. } if kind.to_ascii_lowercase().contains("bench"));
    kind_says_bench(target)
        || target
            .location
            .as_ref()
            .is_some_and(|location| is_benchmark_path(location.path.as_str()))
        || target
            .parent
            .as_ref()
            .and_then(|parent| targets.get(parent))
            .is_some_and(|parent| kind_says_bench(parent))
}

/// Whether a workspace-relative path follows a test or benchmark naming convention.
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    let (directories, file) = split_path(path);
    if directories.iter().any(|directory| {
        TEST_DIRECTORIES.contains(directory) || BENCHMARK_DIRECTORIES.contains(directory)
    }) {
        return true;
    }
    let (stem, extension) = file.rsplit_once('.').unwrap_or((file, ""));
    let jvm = matches!(extension, "java" | "kt" | "scala" | "groovy");
    // `app.test.ts` and `app.spec.tsx` carry the marker as an inner extension.
    let inner = stem.rsplit_once('.').map_or("", |(_, inner)| inner);
    stem == "test"
        || stem == "tests"
        || stem == "conftest"
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with("_bench")
        || (extension == "py" && stem.starts_with("test_"))
        || matches!(inner, "test" | "spec" | "bench")
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
        || stem.ends_with("Benchmark")
        || (jvm && stem.ends_with("IT"))
}

/// Whether a workspace-relative path follows a benchmark naming convention.
#[must_use]
pub fn is_benchmark_path(path: &str) -> bool {
    let (directories, file) = split_path(path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let inner = stem.rsplit_once('.').map_or("", |(_, inner)| inner);
    directories
        .iter()
        .any(|directory| BENCHMARK_DIRECTORIES.contains(directory))
        || stem.ends_with("_bench")
        || inner == "bench"
        || stem.ends_with("Benchmark")
}

fn split_path(path: &str) -> (Vec<&str>, &str) {
    let mut segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let file = segments.pop().unwrap_or("");
    (segments, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{
        ByteSpan, ConfigurationId, InventoryState, RelationId, RelationProvenance,
        ResolutionQuality, SourceLocation, SourcePath, TargetVisibility,
    };

    struct Inventory {
        targets: Vec<Target>,
        relations: Vec<Relation>,
    }

    impl Inventory {
        fn new() -> Self {
            Self {
                targets: Vec::new(),
                relations: Vec::new(),
            }
        }

        fn add(
            &mut self,
            kind: TargetKind,
            name: &str,
            path: &str,
            parent: Option<&str>,
        ) -> TargetId {
            let id = TargetId::derive([name.as_bytes()]);
            self.targets.push(Target {
                id: id.clone(),
                kind,
                visibility: TargetVisibility::Public,
                name: name.to_owned(),
                parent: parent.map(|parent| TargetId::derive([parent.as_bytes()])),
                location: Some(SourceLocation {
                    path: SourcePath::new(path).unwrap(),
                    bytes: ByteSpan::new(0, 0).unwrap(),
                    start: None,
                    end: None,
                }),
                inventory: InventoryState::Represented,
                capabilities: Vec::new(),
                diagnostic: None,
            });
            id
        }

        fn portable(
            &mut self,
            kind: PortableTargetKind,
            name: &str,
            path: &str,
            parent: Option<&str>,
        ) -> TargetId {
            self.add(TargetKind::Portable { kind }, name, path, parent)
        }

        fn language(
            &mut self,
            kind: &str,
            name: &str,
            path: &str,
            parent: Option<&str>,
        ) -> TargetId {
            self.add(
                TargetKind::LanguageSpecific {
                    language: "rust".to_owned(),
                    kind: kind.to_owned(),
                },
                name,
                path,
                parent,
            )
        }

        fn relate(&mut self, source: &str, target: &str, kind: &str) {
            let source = TargetId::derive([source.as_bytes()]);
            let target = TargetId::derive([target.as_bytes()]);
            self.relations.push(Relation {
                id: RelationId::derive([
                    source.as_str().as_bytes(),
                    target.as_str().as_bytes(),
                    kind.as_bytes(),
                ]),
                source,
                target,
                kind: kind.to_owned(),
                provenance: RelationProvenance {
                    provider: "fixture".to_owned(),
                    provider_version: "1".to_owned(),
                    configuration: Some(ConfigurationId::derive([b"cfg".as_slice()])),
                    ingest_only: true,
                    resolution: ResolutionQuality::Inferred,
                    detail: None,
                },
            });
        }
    }

    fn id(name: &str) -> TargetId {
        TargetId::derive([name.as_bytes()])
    }

    /// Mirrors what the Rust, Python, TypeScript, and Java adapters emit for a small sample.
    fn sample() -> Inventory {
        use PortableTargetKind::{Callable, File, Module, Package, Test, Type};
        let mut inv = Inventory::new();
        inv.portable(Package, "sample", "Cargo.toml", None);
        inv.language(
            "cargo_target:lib",
            "sample-lib",
            "src/lib.rs",
            Some("sample"),
        );
        inv.portable(File, "src/lib.rs", "src/lib.rs", Some("sample-lib"));
        inv.portable(Callable, "add", "src/lib.rs", Some("src/lib.rs"));
        inv.portable(Module, "inner", "src/lib.rs", Some("src/lib.rs"));
        inv.portable(Callable, "helper", "src/lib.rs", Some("inner"));
        inv.portable(Module, "parser", "src/lib.rs", Some("src/lib.rs"));
        inv.portable(File, "src/parser.rs", "src/parser.rs", Some("parser"));
        inv.portable(Callable, "parse", "src/parser.rs", Some("src/parser.rs"));
        inv.language("impl", "impl@1", "src/parser.rs", Some("src/parser.rs"));
        inv.language("method", "run", "src/parser.rs", Some("impl@1"));
        inv.portable(Module, "tests", "src/lib.rs", Some("src/lib.rs"));
        inv.portable(Test, "adds", "src/lib.rs", Some("tests"));
        inv.portable(Callable, "adds_async", "src/lib.rs", Some("tests"));
        inv.language("cargo_target:test", "api", "tests/api.rs", Some("sample"));
        inv.portable(File, "tests/api.rs", "tests/api.rs", Some("api"));
        inv.portable(Test, "parses_numbers", "tests/api.rs", Some("tests/api.rs"));
        inv.language(
            "cargo_target:bench",
            "speed",
            "benches/speed.rs",
            Some("sample"),
        );
        inv.portable(File, "benches/speed.rs", "benches/speed.rs", Some("speed"));
        inv.portable(
            Callable,
            "bench_add",
            "benches/speed.rs",
            Some("benches/speed.rs"),
        );

        inv.portable(Package, "pysample", "py/pyproject.toml", None);
        inv.portable(File, "py/pkg/calc.py", "py/pkg/calc.py", Some("pysample"));
        inv.portable(Callable, "mul", "py/pkg/calc.py", Some("py/pkg/calc.py"));
        inv.portable(Type, "Calc", "py/pkg/calc.py", Some("py/pkg/calc.py"));
        inv.portable(Callable, "div", "py/pkg/calc.py", Some("Calc"));
        inv.portable(
            File,
            "py/tests/test_calc.py",
            "py/tests/test_calc.py",
            Some("pysample"),
        );
        inv.portable(
            Callable,
            "test_mul",
            "py/tests/test_calc.py",
            Some("py/tests/test_calc.py"),
        );

        inv.portable(Package, "tssample", "ts/package.json", None);
        inv.portable(File, "ts/src/math.ts", "ts/src/math.ts", Some("tssample"));
        inv.portable(Callable, "sub", "ts/src/math.ts", Some("ts/src/math.ts"));
        inv.portable(
            File,
            "ts/test/math.spec.ts",
            "ts/test/math.spec.ts",
            Some("tssample"),
        );

        inv.portable(Package, "jsample", "java/pom.xml", None);
        let main = "java/src/main/java/com/ex/Greeter.java";
        let test = "java/src/test/java/com/ex/GreeterTest.java";
        inv.portable(File, main, main, Some("jsample"));
        inv.portable(Type, "Greeter", main, Some(main));
        inv.portable(Callable, "greet", main, Some("Greeter"));
        inv.portable(File, test, test, Some("jsample"));
        inv.portable(Type, "GreeterTest", test, Some(test));
        inv.portable(Callable, "greets", test, Some("GreeterTest"));

        inv.relate("adds", "add", "rust:calls");
        inv.relate("adds_async", "add", "rust:calls");
        inv.relate("parses_numbers", "parse", "rust:calls");
        inv.relate("parses_numbers", "parser", "rust:references");
        inv.relate("run", "parse", "rust:calls");
        inv.relate("bench_add", "add", "rust:calls");
        inv.relate("greets", "greet", "java:calls");
        inv.relate("ts/test/math.spec.ts", "ts/src/math.ts", "core:imports");
        inv
    }

    #[test]
    fn recognises_test_code_across_languages() {
        let inv = sample();
        let scope = TestingScope::analyze(&inv.targets, &inv.relations);
        for name in [
            "adds",
            "adds_async",
            "tests",
            "api",
            "tests/api.rs",
            "parses_numbers",
            "speed",
            "benches/speed.rs",
            "bench_add",
            "py/tests/test_calc.py",
            "test_mul",
            "ts/test/math.spec.ts",
            "GreeterTest",
            "greets",
        ] {
            assert!(scope.is_test_code(&id(name)), "{name} should be test code");
        }
        for name in [
            "add",
            "helper",
            "parse",
            "run",
            "mul",
            "div",
            "sub",
            "greet",
            "src/lib.rs",
            "inner",
            "sample",
        ] {
            assert!(
                !scope.is_test_code(&id(name)),
                "{name} should be production code"
            );
        }
        assert!(scope.is_benchmark(&id("bench_add")));
        assert!(scope.is_benchmark(&id("speed")));
        assert!(!scope.is_benchmark(&id("adds")));
    }

    #[test]
    fn assigns_members_to_their_nearest_module_scope() {
        let inv = sample();
        let scope = TestingScope::analyze(&inv.targets, &inv.relations);
        let members = |module: &str| {
            scope
                .members(&id(module))
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(members("src/lib.rs"), BTreeSet::from([id("add")]));
        assert_eq!(members("inner"), BTreeSet::from([id("helper")]));
        assert_eq!(
            members("src/parser.rs"),
            BTreeSet::from([id("parse"), id("run")])
        );
        assert_eq!(
            members("py/pkg/calc.py"),
            BTreeSet::from([id("mul"), id("Calc"), id("div")])
        );
        assert_eq!(
            members("java/src/main/java/com/ex/Greeter.java"),
            BTreeSet::from([id("Greeter"), id("greet")])
        );
        // A `mod parser;` declaration has no members of its own, and test modules are not scopes.
        assert!(members("parser").is_empty());
        assert!(members("tests").is_empty());
        assert!(members("py/tests/test_calc.py").is_empty());
    }

    #[test]
    fn links_tests_directly_indirectly_and_by_import() {
        let inv = sample();
        let scope = TestingScope::analyze(&inv.targets, &inv.relations);
        let links = |member: &str| {
            scope
                .tests_for(&id(member))
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        let direct = |test: &str| TestLink {
            test: id(test),
            via: None,
        };
        assert_eq!(
            links("add"),
            BTreeSet::from([direct("adds"), direct("adds_async"), direct("bench_add")])
        );
        // `parses_numbers` calls `parse` directly, so its indirect path through `run` is dropped.
        assert_eq!(links("parse"), BTreeSet::from([direct("parses_numbers")]));
        assert_eq!(links("greet"), BTreeSet::from([direct("greets")]));
        assert!(
            links("mul").is_empty(),
            "the Python adapter emits no test call edges"
        );
        assert_eq!(
            scope
                .importing_tests(&id("ts/src/math.ts"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![id("ts/test/math.spec.ts")]
        );
    }

    #[test]
    fn indirect_links_record_the_production_caller() {
        let mut inv = sample();
        inv.relate("adds", "run", "rust:calls");
        let scope = TestingScope::analyze(&inv.targets, &inv.relations);
        let links = scope
            .tests_for(&id("parse"))
            .cloned()
            .collect::<BTreeSet<_>>();
        assert!(links.contains(&TestLink {
            test: id("adds"),
            via: Some(id("run"))
        }));
    }

    #[test]
    fn resolves_packages_and_test_file_inventory() {
        let inv = sample();
        let scope = TestingScope::analyze(&inv.targets, &inv.relations);
        assert_eq!(
            scope.package_of(&id("parse")).map(|p| p.name.as_str()),
            Some("sample")
        );
        assert_eq!(
            scope.package_of(&id("div")).map(|p| p.name.as_str()),
            Some("pysample")
        );
        let files = scope
            .test_files()
            .map(|t| t.name.clone())
            .collect::<BTreeSet<_>>();
        for name in [
            "api",
            "tests/api.rs",
            "speed",
            "benches/speed.rs",
            "py/tests/test_calc.py",
            "ts/test/math.spec.ts",
            "java/src/test/java/com/ex/GreeterTest.java",
        ] {
            assert!(files.contains(name), "{name} missing from test inventory");
        }
    }

    #[test]
    fn path_conventions() {
        for path in [
            "tests/api.rs",
            "src/test/java/a/B.java",
            "pkg/__tests__/x.js",
            "spec/models/user_spec.rb",
            "benches/speed.rs",
            "src/foo_test.go",
            "tests.rs",
            "src/tests.rs",
            "py/test_calc.py",
            "conftest.py",
            "web/app.test.ts",
            "web/app.spec.tsx",
            "src/FooTest.java",
            "src/FooTests.cs",
            "src/FooIT.java",
            "src/parse_bench.rs",
            "src/ParserBenchmark.java",
        ] {
            assert!(is_test_path(path), "{path} should be a test path");
        }
        for path in [
            "src/lib.rs",
            "src/contest.rs",
            "src/Contest.java",
            "src/latest.py",
            "testdata.rs",
            "src/attestation/mod.rs",
            "src/FooIT.py",
            "py/pkg/testing_utils.py",
        ] {
            assert!(!is_test_path(path), "{path} should not be a test path");
        }
        assert!(is_benchmark_path("benches/speed.rs"));
        assert!(!is_benchmark_path("tests/api.rs"));
    }
}
