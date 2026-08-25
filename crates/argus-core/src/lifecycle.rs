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

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryState {
    Pending,
    Represented,
    Excluded,
    Unsupported,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicabilityState {
    Pending,
    Applicable,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Pending,
    Admitted,
    Leased,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentState {
    Pending,
    Passed,
    CandidateFinding,
    UnableToVerify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    NotRequested,
    Pending,
    Corroborated,
    Disputed,
    UnableToVerify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdjudicationState {
    Unreviewed,
    Accepted,
    Rejected,
    Deferred,
}

/// Orthogonal lifecycle dimensions for a logical target-policy work item.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditState {
    pub inventory: InventoryState,
    pub applicability: ApplicabilityState,
    pub execution: ExecutionState,
    pub assessment: AssessmentState,
    pub verification: VerificationState,
    pub adjudication: AdjudicationState,
}

impl AuditState {
    pub fn validate(&self) -> Result<(), crate::ArgusError> {
        if (self.inventory != InventoryState::Represented
            || self.applicability != ApplicabilityState::Applicable)
            && self.execution != ExecutionState::Pending
        {
            return Err(crate::ArgusError::invariant(
                "only represented, applicable targets can execute",
            ));
        }
        if self.execution != ExecutionState::Succeeded
            && self.assessment != AssessmentState::Pending
        {
            return Err(crate::ArgusError::invariant(
                "assessment requires successful execution",
            ));
        }
        if self.assessment != AssessmentState::CandidateFinding
            && self.verification != VerificationState::NotRequested
        {
            return Err(crate::ArgusError::invariant(
                "verification requires a candidate finding",
            ));
        }
        if self.assessment != AssessmentState::CandidateFinding
            && self.adjudication != AdjudicationState::Unreviewed
        {
            return Err(crate::ArgusError::invariant(
                "adjudication requires a candidate finding",
            ));
        }
        Ok(())
    }

    pub fn transition_execution(&mut self, next: ExecutionState) -> Result<(), crate::ArgusError> {
        let legal = matches!(
            (self.execution, next),
            (
                ExecutionState::Pending | ExecutionState::Failed,
                ExecutionState::Admitted
            ) | (
                ExecutionState::Admitted,
                ExecutionState::Leased | ExecutionState::Cancelled
            ) | (
                ExecutionState::Leased,
                ExecutionState::Admitted
                    | ExecutionState::Succeeded
                    | ExecutionState::Failed
                    | ExecutionState::Cancelled
            )
        );
        if !legal
            || self.inventory != InventoryState::Represented
            || self.applicability != ApplicabilityState::Applicable
        {
            return Err(crate::ArgusError::invariant("illegal execution transition"));
        }
        self.execution = next;
        Ok(())
    }

    pub fn transition_assessment(
        &mut self,
        next: AssessmentState,
    ) -> Result<(), crate::ArgusError> {
        if self.execution != ExecutionState::Succeeded
            || self.assessment != AssessmentState::Pending
            || next == AssessmentState::Pending
        {
            return Err(crate::ArgusError::invariant(
                "illegal assessment transition",
            ));
        }
        self.assessment = next;
        Ok(())
    }

    pub fn transition_verification(
        &mut self,
        next: VerificationState,
    ) -> Result<(), crate::ArgusError> {
        let legal = matches!(
            (self.verification, next),
            (VerificationState::NotRequested, VerificationState::Pending)
                | (
                    VerificationState::Pending,
                    VerificationState::Corroborated
                        | VerificationState::Disputed
                        | VerificationState::UnableToVerify
                )
        );
        if self.assessment != AssessmentState::CandidateFinding || !legal {
            return Err(crate::ArgusError::invariant(
                "illegal verification transition",
            ));
        }
        self.verification = next;
        Ok(())
    }

    pub fn transition_adjudication(
        &mut self,
        next: AdjudicationState,
    ) -> Result<(), crate::ArgusError> {
        if self.assessment != AssessmentState::CandidateFinding
            || self.adjudication != AdjudicationState::Unreviewed
            || next == AdjudicationState::Unreviewed
        {
            return Err(crate::ArgusError::invariant(
                "illegal adjudication transition",
            ));
        }
        self.adjudication = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn represented() -> AuditState {
        AuditState {
            inventory: InventoryState::Represented,
            applicability: ApplicabilityState::Applicable,
            execution: ExecutionState::Pending,
            assessment: AssessmentState::Pending,
            verification: VerificationState::NotRequested,
            adjudication: AdjudicationState::Unreviewed,
        }
    }

    #[test]
    fn normal_execution_path_is_legal() {
        let mut state = represented();
        state
            .transition_execution(ExecutionState::Admitted)
            .unwrap();
        state.transition_execution(ExecutionState::Leased).unwrap();
        state
            .transition_execution(ExecutionState::Succeeded)
            .unwrap();
        assert!(state.validate().is_ok());
    }

    #[test]
    fn finding_verification_and_adjudication_are_independent() {
        let mut state = represented();
        state
            .transition_execution(ExecutionState::Admitted)
            .unwrap();
        state.transition_execution(ExecutionState::Leased).unwrap();
        state
            .transition_execution(ExecutionState::Succeeded)
            .unwrap();
        state
            .transition_assessment(AssessmentState::CandidateFinding)
            .unwrap();
        state
            .transition_verification(VerificationState::Pending)
            .unwrap();
        state
            .transition_adjudication(AdjudicationState::Deferred)
            .unwrap();
        state
            .transition_verification(VerificationState::Corroborated)
            .unwrap();
        assert!(state.validate().is_ok());
    }

    proptest! {
        #[test]
        fn pending_cannot_jump_to_terminal(terminal in prop_oneof![
            Just(ExecutionState::Succeeded), Just(ExecutionState::Failed), Just(ExecutionState::Cancelled)
        ]) {
            let mut state = represented();
            prop_assert!(state.transition_execution(terminal).is_err());
            prop_assert_eq!(state.execution, ExecutionState::Pending);
        }
    }
}
