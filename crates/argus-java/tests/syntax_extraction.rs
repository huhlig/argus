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

use argus_core::{ConfigurationId, EvidenceKind, PortableTargetKind, SourcePath, TargetKind};
use argus_java::JavaSyntaxProvider;

const JAVA_SAMPLE: &str = r#"package com.example.model;

import java.util.List;
import java.io.Serializable;

/**
 * Account service managing transactions.
 */
public class AccountService extends BaseService implements Serializable {
    /** Maximum daily limit */
    public static final int MAX_LIMIT = 5000;

    /** Current balance */
    private double balance;

    /**
     * Creates an account with initial balance.
     */
    public AccountService(double initialBalance) {
        this.balance = initialBalance;
    }

    /**
     * Deposits amount into account.
     */
    public void deposit(double amount) {
        this.balance += amount;
    }
}

/**
 * Immutable transaction record.
 */
public record Transaction(String id, double amount) implements Serializable {
    public Transaction {
        if (amount < 0) {
            throw new IllegalArgumentException();
        }
    }
}

/**
 * Account status.
 */
public enum AccountStatus {
    ACTIVE,
    SUSPENDED
}
"#;

#[test]
fn syntax_provider_extracts_all_java_symbols() {
    let provider = JavaSyntaxProvider::new(ConfigurationId::derive([b"cfg".as_slice()]));
    let path = SourcePath::new("src/main/java/com/example/model/AccountService.java").unwrap();

    let inv = provider.parse_file(&path, JAVA_SAMPLE, None).unwrap();
    assert!(inv.diagnostics.is_empty());

    // Check targets
    let target_names: Vec<_> = inv.targets.iter().map(|t| t.name.as_str()).collect();

    // File target
    assert!(target_names.contains(&"src/main/java/com/example/model/AccountService.java"));

    // Types
    assert!(target_names.contains(&"AccountService"));
    assert!(target_names.contains(&"Transaction"));
    assert!(target_names.contains(&"AccountStatus"));

    // Methods & Constructors
    assert!(target_names.contains(&"deposit"));
    assert!(target_names.contains(&"AccountService"));
    assert!(target_names.contains(&"Transaction")); // compact ctor

    // Fields & Constants
    assert!(target_names.contains(&"MAX_LIMIT"));
    assert!(target_names.contains(&"balance"));
    assert!(target_names.contains(&"ACTIVE"));
    assert!(target_names.contains(&"SUSPENDED"));

    // Check TargetKinds
    let account_cls = inv
        .targets
        .iter()
        .find(|t| {
            t.name == "AccountService"
                && matches!(
                    t.kind,
                    TargetKind::Portable {
                        kind: PortableTargetKind::Type
                    }
                )
        })
        .unwrap();
    assert!(account_cls.location.is_some());

    let deposit_m = inv.targets.iter().find(|t| t.name == "deposit").unwrap();
    assert!(matches!(
        deposit_m.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable
        }
    ));

    // Check Documentation Evidence
    let doc_evidence: Vec<_> = inv
        .evidence
        .iter()
        .filter(|e| e.kind == EvidenceKind::Documentation)
        .collect();
    assert!(!doc_evidence.is_empty());

    // AccountService has doc
    assert!(doc_evidence.iter().any(|e| {
        e.detail
            .as_deref()
            .unwrap_or("")
            .contains("Account service managing transactions")
    }));
    // deposit has doc
    assert!(doc_evidence.iter().any(|e| {
        e.detail
            .as_deref()
            .unwrap_or("")
            .contains("Deposits amount into account")
    }));
    // Transaction has doc
    assert!(doc_evidence.iter().any(|e| {
        e.detail
            .as_deref()
            .unwrap_or("")
            .contains("Immutable transaction record")
    }));

    // Check Inheritances
    assert_eq!(inv.inheritances.len(), 3); // AccountService extends BaseService, implements Serializable; Transaction implements Serializable
    assert!(
        inv.inheritances
            .iter()
            .any(|i| i.base_name == "BaseService" && !i.is_interface)
    );
    assert!(
        inv.inheritances
            .iter()
            .any(|i| i.base_name == "Serializable" && i.is_interface)
    );

    // Check Imports
    assert_eq!(inv.imports.len(), 2);
    assert!(inv.imports.iter().any(|i| i.path == "java.util.List"));
    assert!(inv.imports.iter().any(|i| i.path == "java.io.Serializable"));
}

const JAVA_TESTS: &str = r#"package com.example;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;

public class CalculatorTest {
    @Test
    void addsNumbers() {
        new Calculator().add(1, 2);
    }

    @ParameterizedTest
    void addsMany(int value) {
        new Calculator().add(value, value);
    }

    @org.junit.Test
    public void legacyJunitFour() {}

    @org.openjdk.jmh.annotations.Benchmark
    public void measureAdd() {}

    private Calculator fixture() {
        return new Calculator();
    }
}
"#;

#[test]
fn syntax_provider_emits_source_evidence_for_files_types_and_callables() {
    let provider = JavaSyntaxProvider::new(ConfigurationId::derive([b"java".as_slice()]));
    let path = SourcePath::new("src/main/java/com/example/model/AccountService.java").unwrap();
    let inv = provider.parse_file(&path, JAVA_SAMPLE, None).unwrap();
    let source_for = |name: &str| {
        let target = inv
            .targets
            .iter()
            .find(|target| target.name == name)
            .unwrap();
        inv.evidence
            .iter()
            .find(|record| {
                record.kind == EvidenceKind::Source && record.target.as_ref() == Some(&target.id)
            })
            .and_then(|record| record.detail.clone())
            .unwrap_or_else(|| panic!("no source evidence for {name}"))
    };
    assert!(source_for(path.as_str()).starts_with("package com.example.model;"));
    assert!(source_for("AccountService").contains("public void deposit(double amount)"));
    let deposit = source_for("deposit");
    assert!(deposit.contains("this.balance += amount;"));
    assert!(!deposit.contains("class AccountService"));
}

#[test]
fn syntax_provider_classifies_junit_testng_and_jmh_methods() {
    let provider = JavaSyntaxProvider::new(ConfigurationId::derive([b"java".as_slice()]));
    let path = SourcePath::new("src/test/java/com/example/CalculatorTest.java").unwrap();
    let inv = provider.parse_file(&path, JAVA_TESTS, None).unwrap();
    let kind = |name: &str| {
        inv.targets
            .iter()
            .find(|target| target.name == name)
            .map(|target| target.kind.clone())
            .unwrap()
    };
    let test = TargetKind::Portable {
        kind: PortableTargetKind::Test,
    };
    assert_eq!(kind("addsNumbers"), test);
    assert_eq!(kind("addsMany"), test);
    assert_eq!(kind("legacyJunitFour"), test);
    assert_eq!(
        kind("measureAdd"),
        TargetKind::LanguageSpecific {
            language: "java".to_owned(),
            kind: "benchmark".to_owned(),
        }
    );
    assert_eq!(
        kind("fixture"),
        TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        }
    );

    // Classification does not change identity: the ID depends only on the signature.
    let plain = JAVA_TESTS.replace("    @Test\n    void addsNumbers", "    void addsNumbers");
    let plain_inv = provider.parse_file(&path, &plain, None).unwrap();
    let id = |inventory: &argus_java::JavaSyntaxInventory| {
        inventory
            .targets
            .iter()
            .find(|target| target.name == "addsNumbers")
            .unwrap()
            .id
            .clone()
    };
    assert_eq!(id(&inv), id(&plain_inv));
}

const JAVA_SPANS: &str = r#"package com.example;

public interface Shape {
    double area();
}

class Parser {
    String first(String input) {
        String braces = "}{"; // } in a comment
        char close = '}';
        return input + braces + close;
    }

    <T> T second(T value) {
        /* } */
        String block = """
            }
            """;
        return value;
    }
}

record Range(int low, int high) {
    Range {
        if (low > high) { throw new IllegalArgumentException("}"); }
    }
}
"#;

#[test]
fn callable_spans_cover_exactly_their_declaration() {
    let provider = JavaSyntaxProvider::new(ConfigurationId::derive([b"java".as_slice()]));
    let path = SourcePath::new("src/main/java/com/example/Parser.java").unwrap();
    let inv = provider.parse_file(&path, JAVA_SPANS, None).unwrap();
    let text_of = |name: &str| {
        let location = inv
            .targets
            .iter()
            .find(|target| target.name == name)
            .and_then(|target| target.location.clone())
            .unwrap();
        JAVA_SPANS[location.bytes.start as usize..location.bytes.end as usize].to_owned()
    };
    let first = text_of("first");
    assert!(first.starts_with("String first(String input) {"), "{first}");
    assert!(
        first.ends_with("return input + braces + close;\n    }"),
        "{first}"
    );
    let second = text_of("second");
    assert!(second.starts_with("<T> T second(T value) {"), "{second}");
    assert!(second.ends_with("return value;\n    }"), "{second}");
    assert_eq!(text_of("area"), "double area();");
    let compact = text_of("Range");
    assert!(
        compact.contains("record Range"),
        "the record type keeps its own span"
    );
    let constructor = inv
        .targets
        .iter()
        .filter(|target| target.name == "Range")
        .filter_map(|target| target.location.clone())
        .map(|location| &JAVA_SPANS[location.bytes.start as usize..location.bytes.end as usize])
        .find(|text| !text.contains("record"))
        .unwrap();
    assert!(constructor.trim_end().ends_with('}'), "{constructor}");
    assert!(!constructor.contains("record"), "{constructor}");
}
