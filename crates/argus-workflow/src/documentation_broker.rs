use crate::DocumentationWorkerRuntime;
use argus_provider::ProviderExecutor;
use async_trait::async_trait;
use langchart_adapters::{
    event::{EventSink, EventSinkError, RuntimeEvent},
    llm::LlmAdapter,
    mcp::{McpAdapter, McpCredential, McpError, ResourceContent, ToolDefinition},
    memory::{MemoryAdapter, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult},
    secrets::{HostMapSecretsAdapter, SecretsAdapter},
};
use langchart_model::id::{IdempotencyKey, ServerId, ToolName};
use langchart_runtime::CapabilityBroker;
use std::sync::Arc;

#[must_use]
pub fn documentation_worker_runtime(
    executor: Arc<ProviderExecutor>,
    llm: Arc<dyn LlmAdapter>,
) -> DocumentationWorkerRuntime {
    let sink: Arc<dyn EventSink> = Arc::new(DiscardingEventSink);
    DocumentationWorkerRuntime {
        executor,
        broker: Arc::new(CapabilityBroker::new(
            llm,
            Arc::new(DisabledMcp),
            Arc::new(DisabledMemory),
            Arc::new(HostMapSecretsAdapter::empty()) as Arc<dyn SecretsAdapter>,
            sink.clone(),
        )),
        event_sink: sink,
    }
}

struct DiscardingEventSink;

#[async_trait]
impl EventSink for DiscardingEventSink {
    async fn append(&self, _event: RuntimeEvent) -> Result<(), EventSinkError> {
        Ok(())
    }
}

struct DisabledMcp;

#[async_trait]
impl McpAdapter for DisabledMcp {
    async fn call_tool(
        &self,
        _server_id: &ServerId,
        _tool_name: &ToolName,
        _arguments: serde_json::Value,
        _credentials: &[McpCredential],
        _idempotency_key: Option<&IdempotencyKey>,
    ) -> Result<serde_json::Value, McpError> {
        Err(McpError::Call(
            "MCP tools are disabled for documentation review".to_owned(),
        ))
    }

    async fn list_tools(&self, _server_id: &ServerId) -> Result<Vec<ToolDefinition>, McpError> {
        Ok(Vec::new())
    }

    async fn read_resource(
        &self,
        _server_id: &ServerId,
        _uri: &str,
    ) -> Result<ResourceContent, McpError> {
        Err(McpError::Call(
            "MCP resources are disabled for documentation review".to_owned(),
        ))
    }
}

struct DisabledMemory;

#[async_trait]
impl MemoryAdapter for DisabledMemory {
    async fn store(&self, _record: MemoryRecord) -> Result<MemoryId, MemoryError> {
        Err(MemoryError::Unsupported)
    }

    async fn search(&self, _query: MemoryQuery) -> Result<Vec<MemoryResult>, MemoryError> {
        Ok(Vec::new())
    }

    async fn get(&self, _id: &MemoryId) -> Result<Option<MemoryRecord>, MemoryError> {
        Ok(None)
    }

    async fn delete(&self, _id: &MemoryId) -> Result<(), MemoryError> {
        Err(MemoryError::Unsupported)
    }
}
