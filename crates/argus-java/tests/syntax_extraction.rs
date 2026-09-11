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
    let account_cls = inv.targets.iter().find(|t| t.name == "AccountService" && matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Type })).unwrap();
    assert!(account_cls.location.is_some());

    let deposit_m = inv.targets.iter().find(|t| t.name == "deposit").unwrap();
    assert!(matches!(deposit_m.kind, TargetKind::Portable { kind: PortableTargetKind::Callable }));

    // Check Documentation Evidence
    let doc_evidence: Vec<_> = inv.evidence.iter().filter(|e| e.kind == EvidenceKind::Documentation).collect();
    assert!(!doc_evidence.is_empty());

    // AccountService has doc
    assert!(doc_evidence.iter().any(|e| e.detail.as_deref().unwrap_or("").contains("Account service managing transactions")));
    // deposit has doc
    assert!(doc_evidence.iter().any(|e| e.detail.as_deref().unwrap_or("").contains("Deposits amount into account")));
    // Transaction has doc
    assert!(doc_evidence.iter().any(|e| e.detail.as_deref().unwrap_or("").contains("Immutable transaction record")));

    // Check Inheritances
    assert_eq!(inv.inheritances.len(), 3); // AccountService extends BaseService, implements Serializable; Transaction implements Serializable
    assert!(inv.inheritances.iter().any(|i| i.base_name == "BaseService" && !i.is_interface));
    assert!(inv.inheritances.iter().any(|i| i.base_name == "Serializable" && i.is_interface));

    // Check Imports
    assert_eq!(inv.imports.len(), 2);
    assert!(inv.imports.iter().any(|i| i.path == "java.util.List"));
    assert!(inv.imports.iter().any(|i| i.path == "java.io.Serializable"));
}
