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

use argus_core::{
    AdjudicationState, ApplicabilityState, AssessmentState, AuditState, ByteSpan, Capability,
    CapabilityStatus, ExecutionState, InventoryState, LineColumn, PortableTargetKind,
    SourceLocation, SourcePath, Target, TargetId, TargetKind, TargetVisibility, VerificationState,
    Versioned,
};

#[test]
fn portable_domain_record_round_trips() {
    let record = Versioned::current((
        TargetId::derive([b"package/item".as_slice()]),
        TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        },
        SourceLocation {
            path: SourcePath::new("src/lib.rs").unwrap(),
            bytes: ByteSpan::new(4, 18).unwrap(),
            start: Some(LineColumn { line: 1, column: 4 }),
            end: Some(LineColumn { line: 2, column: 3 }),
        },
        Capability {
            name: "syntax".to_owned(),
            status: CapabilityStatus::Complete,
            detail: None,
            provider: Some("synthetic@1".to_owned()),
        },
        AuditState {
            inventory: InventoryState::Represented,
            applicability: ApplicabilityState::Applicable,
            execution: ExecutionState::Succeeded,
            assessment: AssessmentState::CandidateFinding,
            verification: VerificationState::Pending,
            adjudication: AdjudicationState::Unreviewed,
        },
    ));

    let json = serde_json::to_string(&record).unwrap();
    let decoded = serde_json::from_str(&json).unwrap();
    assert_eq!(record, decoded);
}

#[test]
fn unknown_language_specific_kind_is_retained() {
    let json = r#"{"scope":"language_specific","language":"future-lang","kind":"quantum_block"}"#;
    let decoded: TargetKind = serde_json::from_str(json).unwrap();
    assert_eq!(
        decoded,
        TargetKind::LanguageSpecific {
            language: "future-lang".to_owned(),
            kind: "quantum_block".to_owned(),
        }
    );
    assert_eq!(serde_json::to_string(&decoded).unwrap(), json);
}

#[test]
fn target_visibility_round_trips_and_defaults_for_existing_records() {
    let target = Target {
        id: TargetId::derive([b"restricted-api".as_slice()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        },
        visibility: TargetVisibility::Restricted,
        name: "restricted_api".to_owned(),
        parent: None,
        location: None,
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    };
    let value = serde_json::to_value(&target).unwrap();
    assert_eq!(
        serde_json::from_value::<Target>(value.clone()).unwrap(),
        target
    );

    let mut existing = value.as_object().unwrap().clone();
    existing.remove("visibility");
    let decoded: Target = serde_json::from_value(existing.into()).unwrap();
    assert_eq!(decoded.visibility, TargetVisibility::Unknown);
}

#[test]
fn unsupported_schema_is_not_loaded_as_current() {
    let old = Versioned {
        schema_version: 0,
        record: TargetKind::Portable {
            kind: PortableTargetKind::Module,
        },
    };
    assert!(old.into_current().is_err());
}
