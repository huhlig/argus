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

//! Immutable evidence storage and bounded review-context construction.

mod context;
mod package;
mod request;
mod store;

pub use package::{
    CandidateAvailability, EvidenceBudget, EvidenceCandidate, EvidenceDisposition, EvidencePackage,
    EvidencePackageBuilder, EvidencePackageItem, PackageArtifact, PolicyEvidenceRequirements,
};
pub use request::{
    AuthorizedEvidenceExpansion, EvidenceExpansionPolicy, EvidenceRequest,
    EvidenceRequestAuthorizer, ExpansionDenial, ExpansionDenialReason, ExpansionUsage,
};
pub use store::{DataClassification, EvidenceEnvelope, EvidenceStore, StoredEvidence};

pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;
pub use context::{
    ContextArtifact, FramedEvidence, ReviewContextBuilder, ReviewContextFrame, TrustedControl,
};
