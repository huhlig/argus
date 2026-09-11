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

use argus_core::{ConfigurationId, SnapshotId, SourcePath};
use argus_language::SourceAccess;
use argus_typescript::{TypeScriptRelationshipProvider, TypeScriptSyntaxProvider};
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
fn infers_inheritance_and_implementation_relationships() {
    let animal_ts = r#"
export interface Pet {
    play(): void;
}

export class Animal {
    name: string;
    constructor(name: string) {
        this.name = name;
    }
}
"#;

    let dog_ts = r#"
import { Animal, Pet } from './animal';

export class Dog extends Animal implements Pet {
    play(): void {
        console.log("Playing fetch");
    }
}
"#;

    let animal_path = SourcePath::new("src/animal.ts").unwrap();
    let dog_path = SourcePath::new("src/dog.ts").unwrap();

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-relationships".as_slice()]),
        files: BTreeMap::from([
            (animal_path.clone(), animal_ts.as_bytes().to_vec()),
            (dog_path.clone(), dog_ts.as_bytes().to_vec()),
        ]),
    };

    let cfg = ConfigurationId::derive([b"default".as_slice()]);
    let syntax_provider = TypeScriptSyntaxProvider::new(cfg.clone());

    let mut all_targets = Vec::new();
    let mut all_inheritances = Vec::new();
    let mut all_imports = Vec::new();

    for p in [&animal_path, &dog_path] {
        let inv = syntax_provider.inventory_file(&source, p, None).unwrap();
        all_targets.extend(inv.targets);
        all_inheritances.extend(inv.inheritances);
        all_imports.extend(inv.imports);
    }

    let rel_provider = TypeScriptRelationshipProvider::new(cfg);
    let rel_inv = rel_provider
        .infer(&source, &all_targets, &all_inheritances, &all_imports)
        .unwrap();

    // 1. Verify extends relation
    let extends_rel = rel_inv
        .relations
        .iter()
        .find(|r| r.kind == "typescript:extends")
        .expect("extends relation exists");

    let dog_target = all_targets.iter().find(|t| t.name == "Dog").unwrap();
    let animal_target = all_targets.iter().find(|t| t.name == "Animal").unwrap();
    assert_eq!(extends_rel.source, dog_target.id);
    assert_eq!(extends_rel.target, animal_target.id);

    // 2. Verify implements relation
    let implements_rel = rel_inv
        .relations
        .iter()
        .find(|r| r.kind == "typescript:implements")
        .expect("implements relation exists");
    let pet_target = all_targets.iter().find(|t| t.name == "Pet").unwrap();
    assert_eq!(implements_rel.source, dog_target.id);
    assert_eq!(implements_rel.target, pet_target.id);

    // 3. Verify relative import relation
    let import_rel = rel_inv
        .relations
        .iter()
        .find(|r| r.kind == "core:imports")
        .expect("imports relation exists");
    let dog_file_target = all_targets.iter().find(|t| t.name == "src/dog.ts").unwrap();
    let animal_file_target = all_targets
        .iter()
        .find(|t| t.name == "src/animal.ts")
        .unwrap();
    assert_eq!(import_rel.source, dog_file_target.id);
    assert_eq!(import_rel.target, animal_file_target.id);
}

#[test]
fn infers_lexical_calls_and_references() {
    let math_ts = r#"
export function computeTotal(base: number, tax: number): number {
    return base + tax;
}
"#;

    let checkout_ts = r#"
import { computeTotal } from './math';

export function runCheckout(price: number): number {
    return computeTotal(price, 5);
}
"#;

    let math_path = SourcePath::new("src/math.ts").unwrap();
    let checkout_path = SourcePath::new("src/checkout.ts").unwrap();

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-calls".as_slice()]),
        files: BTreeMap::from([
            (math_path.clone(), math_ts.as_bytes().to_vec()),
            (checkout_path.clone(), checkout_ts.as_bytes().to_vec()),
        ]),
    };

    let cfg = ConfigurationId::derive([b"default".as_slice()]);
    let syntax_provider = TypeScriptSyntaxProvider::new(cfg.clone());

    let mut all_targets = Vec::new();
    let mut all_inheritances = Vec::new();
    let mut all_imports = Vec::new();

    for p in [&math_path, &checkout_path] {
        let inv = syntax_provider.inventory_file(&source, p, None).unwrap();
        all_targets.extend(inv.targets);
        all_inheritances.extend(inv.inheritances);
        all_imports.extend(inv.imports);
    }

    let rel_provider = TypeScriptRelationshipProvider::new(cfg);
    let rel_inv = rel_provider
        .infer(&source, &all_targets, &all_inheritances, &all_imports)
        .unwrap();

    // Verify calls relation
    let call_rel = rel_inv
        .relations
        .iter()
        .find(|r| r.kind == "typescript:calls")
        .expect("calls relation found");

    let checkout_fn = all_targets
        .iter()
        .find(|t| t.name == "runCheckout")
        .unwrap();
    let compute_fn = all_targets
        .iter()
        .find(|t| t.name == "computeTotal")
        .unwrap();

    assert_eq!(call_rel.source, checkout_fn.id);
    assert_eq!(call_rel.target, compute_fn.id);
}
