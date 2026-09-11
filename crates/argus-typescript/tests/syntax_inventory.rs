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

use argus_core::{ConfigurationId, EvidenceKind, PortableTargetKind, SnapshotId, SourcePath, TargetKind, TargetVisibility};
use argus_language::SourceAccess;
use argus_typescript::TypeScriptSyntaxProvider;
use std::collections::BTreeMap;

struct MemorySource {
    snapshot: SnapshotId,
    files: BTreeMap<SourcePath, Vec<u8>>,
}

impl SourceAccess for MemorySource {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.snapshot
    }

    fn contains(&self, path: &SourcePath) -> bool {
        self.files.contains_key(path)
    }

    fn read(&self, path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| argus_core::ArgusError::invalid_input("missing source"))
    }
}

#[test]
fn extracts_all_typescript_syntax_symbols_and_jsdoc() {
    let source_code = r#"
/**
 * Application greeting utility.
 */
export function greet(name: string): string {
    return `Hello, ${name}!`;
}

/**
 * Calculator class providing arithmetic operations.
 */
export class Calculator {
    private precision: number = 2;

    constructor(precision?: number) {
        if (precision !== undefined) {
            this.precision = precision;
        }
    }

    /**
     * Adds two numbers together.
     */
    public add(a: number, b: number): number {
        return a + b;
    }
}

/**
 * Supported theme modes.
 */
export enum Theme {
    Light = "light",
    Dark = "dark",
}

/**
 * User data representation.
 */
export interface User {
    id: string;
    username: string;
}

export type UserId = string | number;

export const DEFAULT_TIMEOUT = 5000;

export const formatUser = (user: User): string => {
    return `${user.username} (${user.id})`;
};

namespace LegacyApi {
    export function fetchLegacy(): void {}
}
"#;

    let path = SourcePath::new("src/index.ts").unwrap();
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-ts-syntax".as_slice()]),
        files: BTreeMap::from([(path.clone(), source_code.as_bytes().to_vec())]),
    };

    let provider = TypeScriptSyntaxProvider::new(ConfigurationId::derive([b"default".as_slice()]));
    let inventory = provider
        .inventory_file(&source, &path, None)
        .expect("inventory succeeds");

    assert!(inventory.diagnostics.is_empty());

    // Verify File target
    let file_target = inventory
        .targets
        .iter()
        .find(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::File }))
        .expect("file target exists");
    assert_eq!(file_target.name, "src/index.ts");

    // Verify function greet
    let greet_target = inventory
        .targets
        .iter()
        .find(|t| t.name == "greet")
        .expect("greet target exists");
    assert_eq!(greet_target.visibility, TargetVisibility::Public);
    assert_eq!(
        greet_target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable
        }
    );
    let greet_doc = inventory
        .documentation
        .get(&greet_target.id)
        .expect("greet has doc");
    assert!(greet_doc.contains("Application greeting utility."));

    // Verify Calculator class and methods
    let calc_target = inventory
        .targets
        .iter()
        .find(|t| t.name == "Calculator")
        .expect("Calculator class exists");
    assert_eq!(calc_target.visibility, TargetVisibility::Public);
    assert_eq!(
        calc_target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Type
        }
    );

    let add_method = inventory
        .targets
        .iter()
        .find(|t| t.name == "add")
        .expect("add method exists");
    assert_eq!(add_method.parent.as_ref(), Some(&calc_target.id));
    assert_eq!(add_method.visibility, TargetVisibility::Public);
    let add_doc = inventory
        .documentation
        .get(&add_method.id)
        .expect("add method has doc");
    assert!(add_doc.contains("Adds two numbers together."));

    // Verify Enum and member
    let theme_target = inventory
        .targets
        .iter()
        .find(|t| t.name == "Theme")
        .expect("Theme enum exists");
    assert_eq!(
        theme_target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Type
        }
    );

    let light_member = inventory
        .targets
        .iter()
        .find(|t| t.name == "Light")
        .expect("Light member exists");
    assert_eq!(light_member.parent.as_ref(), Some(&theme_target.id));
    assert_eq!(
        light_member.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Constant
        }
    );

    // Verify Interface & Type alias
    assert!(inventory.targets.iter().any(|t| t.name == "User"));
    assert!(inventory.targets.iter().any(|t| t.name == "UserId"));

    // Verify Arrow function
    let format_fn = inventory
        .targets
        .iter()
        .find(|t| t.name == "formatUser")
        .expect("formatUser arrow function exists");
    assert_eq!(
        format_fn.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable
        }
    );

    // Verify Namespace
    assert!(inventory.targets.iter().any(|t| t.name == "LegacyApi"));

    // Verify Evidence Records (Source and Documentation)
    let evidence = provider
        .review_evidence(&source, &inventory)
        .expect("review evidence succeeds");
    assert!(evidence.len() >= inventory.targets.len() * 2);

    let greet_src_evidence = evidence
        .iter()
        .find(|e| e.target.as_ref() == Some(&greet_target.id) && e.kind == EvidenceKind::Source)
        .expect("greet source evidence exists");
    assert!(greet_src_evidence.detail.as_deref().unwrap().contains("function greet"));

    let greet_doc_evidence = evidence
        .iter()
        .find(|e| e.target.as_ref() == Some(&greet_target.id) && e.kind == EvidenceKind::Documentation)
        .expect("greet doc evidence exists");
    assert!(greet_doc_evidence.detail.as_deref().unwrap().contains("Application greeting utility."));
}

#[test]
fn parses_jsx_and_tsx_components() {
    let tsx_code = r#"
import React from 'react';

export interface ButtonProps {
    label: string;
    onClick: () => void;
}

export const Button: React.FC<ButtonProps> = ({ label, onClick }) => {
    return <button onClick={onClick}>{label}</button>;
};
"#;

    let path = SourcePath::new("src/Button.tsx").unwrap();
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-tsx".as_slice()]),
        files: BTreeMap::from([(path.clone(), tsx_code.as_bytes().to_vec())]),
    };

    let provider = TypeScriptSyntaxProvider::new(ConfigurationId::derive([b"default".as_slice()]));
    let inventory = provider
        .inventory_file(&source, &path, None)
        .expect("inventory succeeds");

    assert!(inventory.diagnostics.is_empty());
    assert!(inventory.targets.iter().any(|t| t.name == "ButtonProps"));
    assert!(inventory.targets.iter().any(|t| t.name == "Button"));
    assert_eq!(inventory.imports.len(), 1);
    assert_eq!(inventory.imports[0].source_module, "react");
}
