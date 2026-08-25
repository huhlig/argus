use crate::{EffectiveOutcome, OutcomeDisposition, OutcomeRecorder};
use argus_storage::DurableQueue;
use async_trait::async_trait;
use langchart_runtime::{
    AgentActor, AgentError, AgentInvocation, AgentOutputEvent, CapabilityBroker, CapabilityEnvelope,
};
use serde_json::json;
use std::sync::Arc;

/// Deterministic Langchart actor that commits or retrieves one effective Argus outcome.
pub struct OutcomeRecorderActor {
    inbox: Arc<DurableQueue>,
    proposed: EffectiveOutcome,
}

impl OutcomeRecorderActor {
    #[must_use]
    pub const fn new(inbox: Arc<DurableQueue>, proposed: EffectiveOutcome) -> Self {
        Self { inbox, proposed }
    }
}

#[async_trait]
impl AgentActor for OutcomeRecorderActor {
    async fn run(
        &self,
        _invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        let inbox = self.inbox.clone();
        let proposed = self.proposed.clone();
        let receipt = tokio::task::spawn_blocking(move || {
            OutcomeRecorder::new(inbox.as_ref()).record(&proposed)
        })
        .await
        .map_err(|error| AgentError::Internal(format!("outcome recorder task failed: {error}")))?
        .map_err(|error| AgentError::Internal(error.to_string()))?;
        let disposition = match receipt.disposition {
            OutcomeDisposition::Inserted => "inserted",
            OutcomeDisposition::Existing => "existing",
        };
        Ok(AgentOutputEvent {
            event_type: "outcome.recorded".to_owned(),
            payload: json!({
                "result_ref": receipt.outcome.result_ref,
                "disposition": disposition,
                "storage_key": receipt.storage_key,
            }),
        })
    }
}
