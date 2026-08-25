use crate::{
    CandidateFindingRecord, WorkflowDataRecord, WorkflowDataStore, WorkflowDataWrite,
    verification_work_id,
};
use argus_storage::{DurableQueue, QueueWork};
use async_trait::async_trait;
use langchart_runtime::{
    AgentActor, AgentError, AgentInvocation, AgentOutputEvent, CapabilityBroker, CapabilityEnvelope,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct CandidateRecorderActor {
    workflow_data: Arc<WorkflowDataStore>,
}

impl CandidateRecorderActor {
    #[must_use]
    pub const fn new(workflow_data: Arc<WorkflowDataStore>) -> Self {
        Self { workflow_data }
    }

    async fn record(&self, run_id: &str) -> Result<AgentOutputEvent, AgentError> {
        let record = load(self.workflow_data.clone(), run_id).await?;
        let evidence_revision = record.data.evidence_revision;
        let existing = record
            .data
            .candidate_findings
            .iter()
            .filter(|candidate| candidate.evidence_revision == evidence_revision)
            .collect::<Vec<_>>();
        if !existing.is_empty() {
            return Ok(recorded_event(
                existing.into_iter().map(|item| item.id.as_str()),
            ));
        }
        let decision = record
            .data
            .primary_decisions
            .last()
            .filter(|decision| decision.evidence_revision == evidence_revision)
            .ok_or_else(|| {
                AgentError::Internal("current primary decision is missing".to_owned())
            })?;
        if decision.event_type != "review.candidate_found" {
            return Err(AgentError::Internal(
                "candidate recorder requires a candidate-found decision".to_owned(),
            ));
        }
        let drafts = decision
            .payload
            .get("candidates")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                AgentError::Internal("candidate decision payload is invalid".to_owned())
            })?;
        let mut proposed = record.data.clone();
        for draft in drafts {
            proposed.candidate_findings.push(
                CandidateFindingRecord::derive(&proposed.work_id, evidence_revision, draft.clone())
                    .map_err(|error| AgentError::Internal(error.to_string()))?,
            );
        }
        let effective = compare_and_swap(
            self.workflow_data.clone(),
            run_id,
            record.revision,
            proposed,
        )
        .await?;
        let candidates = effective
            .data
            .candidate_findings
            .iter()
            .filter(|candidate| candidate.evidence_revision == evidence_revision);
        Ok(recorded_event(candidates.map(|item| item.id.as_str())))
    }
}

#[async_trait]
impl AgentActor for CandidateRecorderActor {
    async fn run(
        &self,
        invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        self.record(invocation.run_id.as_ref()).await
    }
}

pub struct FindingWorkSchedulerActor {
    workflow_data: Arc<WorkflowDataStore>,
    queue: Arc<DurableQueue>,
}

impl FindingWorkSchedulerActor {
    #[must_use]
    pub const fn new(workflow_data: Arc<WorkflowDataStore>, queue: Arc<DurableQueue>) -> Self {
        Self {
            workflow_data,
            queue,
        }
    }

    async fn schedule(&self, run_id: &str) -> Result<AgentOutputEvent, AgentError> {
        let record = load(self.workflow_data.clone(), run_id).await?;
        let unscheduled = record
            .data
            .candidate_findings
            .iter()
            .filter(|candidate| {
                !record
                    .data
                    .scheduled_verification_work
                    .contains(&verification_work_id(&candidate.id))
            })
            .cloned()
            .collect::<Vec<_>>();
        if unscheduled.is_empty() {
            return Ok(scheduled_event(&record.data.scheduled_verification_work));
        }
        let work = unscheduled
            .iter()
            .map(|candidate| {
                serde_json::to_vec(candidate)
                    .map(|payload| QueueWork::pending(verification_work_id(&candidate.id), payload))
                    .map_err(|error| AgentError::Internal(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let queue = self.queue.clone();
        let admitted_work = work.clone();
        tokio::task::spawn_blocking(move || queue.admit_batch(&admitted_work, 0))
            .await
            .map_err(|error| AgentError::Internal(format!("work scheduler task failed: {error}")))?
            .map_err(|error| AgentError::Internal(error.to_string()))?;
        let mut proposed = record.data;
        proposed.scheduled_verification_work.extend(
            unscheduled
                .iter()
                .map(|candidate| verification_work_id(&candidate.id)),
        );
        let effective = compare_and_swap(
            self.workflow_data.clone(),
            run_id,
            record.revision,
            proposed,
        )
        .await?;
        Ok(scheduled_event(&effective.data.scheduled_verification_work))
    }
}

#[async_trait]
impl AgentActor for FindingWorkSchedulerActor {
    async fn run(
        &self,
        invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        self.schedule(invocation.run_id.as_ref()).await
    }
}

async fn load(
    store: Arc<WorkflowDataStore>,
    run_id: &str,
) -> Result<WorkflowDataRecord, AgentError> {
    let run_id = run_id.to_owned();
    tokio::task::spawn_blocking(move || store.load(&run_id))
        .await
        .map_err(|error| AgentError::Internal(format!("workflow data task failed: {error}")))?
        .map_err(|error| AgentError::Internal(error.to_string()))?
        .ok_or_else(|| AgentError::Internal("workflow data record is missing".to_owned()))
}

async fn compare_and_swap(
    store: Arc<WorkflowDataStore>,
    run_id: &str,
    revision: u64,
    proposed: crate::ReviewWorkflowData,
) -> Result<WorkflowDataRecord, AgentError> {
    let run_id = run_id.to_owned();
    let write =
        tokio::task::spawn_blocking(move || store.compare_and_swap(&run_id, revision, proposed))
            .await
            .map_err(|error| AgentError::Internal(format!("workflow data task failed: {error}")))?
            .map_err(|error| AgentError::Internal(error.to_string()))?;
    match write {
        WorkflowDataWrite::Updated(record) | WorkflowDataWrite::Existing(record) => Ok(record),
        WorkflowDataWrite::Inserted(_) => Err(AgentError::Internal(
            "actor unexpectedly inserted workflow data".to_owned(),
        )),
    }
}

fn recorded_event<'a>(candidate_ids: impl IntoIterator<Item = &'a str>) -> AgentOutputEvent {
    AgentOutputEvent {
        event_type: "candidates.recorded".to_owned(),
        payload: json!({"candidate_ids": candidate_ids.into_iter().collect::<Vec<_>>() }),
    }
}

fn scheduled_event(work_ids: &[argus_core::WorkItemId]) -> AgentOutputEvent {
    AgentOutputEvent {
        event_type: "finding_work.scheduled".to_owned(),
        payload: json!({
            "work_ids": work_ids.iter().map(argus_core::WorkItemId::as_str).collect::<Vec<_>>()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PrimaryReviewDecision, ReviewWorkflowData};
    use argus_core::{PolicyId, WorkItemId};
    use argus_provider::ProviderIdentity;

    fn store() -> Arc<WorkflowDataStore> {
        let directory = tempfile::tempdir().unwrap().keep();
        let store = Arc::new(WorkflowDataStore::open(&directory).unwrap());
        let work_id = WorkItemId::derive([b"candidate-parent".as_slice()]);
        store
            .create(
                "run-1",
                ReviewWorkflowData {
                    work_id,
                    review_unit_id: "crate:fixture".to_owned(),
                    policy_id: PolicyId::derive([b"documentation".as_slice()]),
                    evidence_package_ref: "evidence:1".to_owned(),
                    evidence_revision: 1,
                    primary_decisions: vec![PrimaryReviewDecision {
                        evidence_revision: 1,
                        event_type: "review.candidate_found".to_owned(),
                        payload: json!({
                            "assessment": {},
                            "candidates": [{
                                "title": "Missing contract",
                                "description": "Public behavior is not documented.",
                                "severity": "medium",
                                "confidence_basis_points": 8500
                            }]
                        }),
                        provider: ProviderIdentity {
                            provider: "fixture-local".to_owned(),
                            provider_version: "1".to_owned(),
                            model: "reviewer".to_owned(),
                            model_version: "pinned".to_owned(),
                        },
                        request_id: "run-1:primary_review:evidence-1".to_owned(),
                        attempt: 0,
                    }],
                    candidate_findings: Vec::new(),
                    scheduled_verification_work: Vec::new(),
                    verification_results: Vec::new(),
                    evidence_request_decisions: Vec::new(),
                    evidence_expansions: Vec::new(),
                    escalation_count: 0,
                    evidence_expansion_count: 0,
                    adjudication: None,
                },
            )
            .unwrap();
        store
    }

    #[tokio::test]
    async fn candidate_recording_and_scheduling_are_replay_safe() {
        let store = store();
        let recorder = CandidateRecorderActor::new(store.clone());
        recorder.record("run-1").await.unwrap();
        recorder.record("run-1").await.unwrap();
        assert_eq!(
            store
                .load("run-1")
                .unwrap()
                .unwrap()
                .data
                .candidate_findings
                .len(),
            1
        );

        let directory = tempfile::tempdir().unwrap();
        let queue = Arc::new(DurableQueue::open(&directory.path().join("queue.redb")).unwrap());
        let scheduler = FindingWorkSchedulerActor::new(store.clone(), queue.clone());
        scheduler.schedule("run-1").await.unwrap();
        scheduler.schedule("run-1").await.unwrap();
        let persisted = store.load("run-1").unwrap().unwrap();
        assert_eq!(persisted.data.scheduled_verification_work.len(), 1);
        assert!(
            queue
                .get(&persisted.data.scheduled_verification_work[0])
                .unwrap()
                .is_some()
        );
    }
}
