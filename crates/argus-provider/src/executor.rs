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

use crate::{
    ModelProvider, ModelRequest, ModelResponse, ProviderCapabilities, ProviderError,
    ProviderHealth, ProviderIdentity, ProviderPolicy, StructuredOutputSupport,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Semaphore;

pub trait OutputValidator: Send + Sync {
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String>;
}

pub trait ProviderTelemetrySink: Send + Sync {
    fn publish(
        &self,
        identity: &ProviderIdentity,
        telemetry: &ProviderTelemetry,
    ) -> Result<(), ProviderError>;
}

impl<F> OutputValidator for F
where
    F: Fn(&Value, &Value) -> Result<(), String> + Send + Sync,
{
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String> {
        self(schema, output)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepairPolicy {
    pub max_repair_attempts: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderTelemetry {
    pub last_health: Option<ProviderHealth>,
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub repair_attempts: u64,
    pub provider_call_millis: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_microusd: u64,
    pub unreported_token_responses: u64,
    pub unreported_cost_responses: u64,
    pub waiting: u64,
    pub peak_waiting: u64,
    pub in_flight: u64,
    pub peak_in_flight: u64,
}

pub struct ProviderExecutor {
    provider: Arc<dyn ModelProvider>,
    expected_identity: ProviderIdentity,
    policy: ProviderPolicy,
    repair: RepairPolicy,
    validator: Arc<dyn OutputValidator>,
    concurrency: Semaphore,
    telemetry: Mutex<ProviderTelemetry>,
    telemetry_sink: Option<Arc<dyn ProviderTelemetrySink>>,
}

impl ProviderExecutor {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        expected_identity: ProviderIdentity,
        policy: ProviderPolicy,
        repair: RepairPolicy,
        validator: Arc<dyn OutputValidator>,
    ) -> Result<Self, ProviderError> {
        policy.authorize(provider.capabilities())?;
        expected_identity.validate()?;
        if provider.capabilities().identity != expected_identity {
            return Err(ProviderError::SubstitutionDenied(
                "adapter identity does not match the assigned provider/model".to_owned(),
            ));
        }
        Ok(Self {
            provider,
            expected_identity,
            concurrency: Semaphore::new(
                usize::try_from(policy.limits.max_concurrency).unwrap_or(usize::MAX),
            ),
            policy,
            repair,
            validator,
            telemetry: Mutex::new(ProviderTelemetry::default()),
            telemetry_sink: None,
        })
    }

    #[must_use]
    pub fn with_telemetry_sink(mut self, sink: Arc<dyn ProviderTelemetrySink>) -> Self {
        self.telemetry_sink = Some(sink);
        self
    }

    pub async fn execute(&self, request: ModelRequest) -> Result<ModelResponse, ProviderError> {
        self.execute_with_provider(self.provider.as_ref(), request)
            .await
    }

    /// Executes through a per-invocation provider with the assigned capability profile.
    ///
    /// This supports broker-bound transports without weakening the executor's
    /// identity, privacy, concurrency, budget, or validation rules.
    pub async fn execute_with_provider(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        let result = self.execute_with_provider_inner(provider, request).await;
        let published = self.publish_telemetry();
        match (result, published) {
            (Ok(response), Ok(())) => Ok(response),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(execution), Err(publication)) => Err(ProviderError::Unavailable(format!(
                "{execution}; provider telemetry publication also failed: {publication}"
            ))),
        }
    }

    async fn execute_with_provider_inner(
        &self,
        provider: &dyn ModelProvider,
        request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        if provider.capabilities() != self.provider.capabilities() {
            return Err(ProviderError::SubstitutionDenied(
                "invocation provider capabilities differ from the assigned provider".to_owned(),
            ));
        }
        self.validate_request(provider.capabilities(), &request)?;
        let health = match provider.health().await {
            Ok(health) => health,
            Err(error) => {
                self.mark_failure();
                return Err(error);
            }
        };
        lock(&self.telemetry).last_health = Some(health);
        match health {
            ProviderHealth::Ready | ProviderHealth::Degraded => {}
            ProviderHealth::Unavailable => {
                self.mark_failure();
                return Err(ProviderError::Unavailable(
                    "health check reported unavailable".to_owned(),
                ));
            }
        }

        self.begin_wait();
        let permit = self.concurrency.acquire().await.map_err(|_| {
            ProviderError::Unavailable("provider concurrency gate closed".to_owned())
        })?;
        self.end_wait_and_begin_call();
        let _call = ActiveCall::new(&self.telemetry);
        let _permit = permit;

        let mut attempt = 0_u32;
        let mut current = request;
        loop {
            self.reserve_request(&current)?;
            let call_started = std::time::Instant::now();
            let completed = provider.complete(current.clone()).await;
            let call_millis = u64::try_from(call_started.elapsed().as_millis())
                .unwrap_or(u64::MAX)
                .max(1);
            {
                let mut telemetry = lock(&self.telemetry);
                telemetry.provider_call_millis =
                    telemetry.provider_call_millis.saturating_add(call_millis);
            }
            let response = match completed {
                Ok(response) => response,
                Err(error) => {
                    self.mark_failure();
                    return Err(error);
                }
            };
            self.validate_response_identity(&current, &response)?;
            self.record_usage(&current, &response)?;
            match self
                .validator
                .validate(&current.structured_output_schema, &response.output)
            {
                Ok(()) => {
                    lock(&self.telemetry).successes += 1;
                    return Ok(response);
                }
                Err(message) if attempt < self.repair.max_repair_attempts => {
                    attempt += 1;
                    lock(&self.telemetry).repair_attempts += 1;
                    let repaired =
                        repair_prompt(&current.prompt, &current.structured_output_schema, &message);
                    let added_bytes = repaired.len().saturating_sub(current.prompt.len());
                    current.estimated_input_tokens = current
                        .estimated_input_tokens
                        .saturating_add(u64::try_from(added_bytes).unwrap_or(u64::MAX));
                    current.prompt = repaired;
                    current.attempt = attempt;
                }
                Err(message) => {
                    self.mark_failure();
                    return Err(ProviderError::InvalidOutput(message));
                }
            }
        }
    }

    #[must_use]
    pub fn telemetry(&self) -> ProviderTelemetry {
        lock(&self.telemetry).clone()
    }

    #[must_use]
    pub fn capabilities(&self) -> &ProviderCapabilities {
        self.provider.capabilities()
    }

    #[must_use]
    pub const fn expected_identity(&self) -> &ProviderIdentity {
        &self.expected_identity
    }

    #[must_use]
    pub const fn policy(&self) -> &ProviderPolicy {
        &self.policy
    }

    fn publish_telemetry(&self) -> Result<(), ProviderError> {
        self.telemetry_sink.as_ref().map_or(Ok(()), |sink| {
            sink.publish(&self.expected_identity, &lock(&self.telemetry))
        })
    }

    fn validate_request(
        &self,
        capabilities: &ProviderCapabilities,
        request: &ModelRequest,
    ) -> Result<(), ProviderError> {
        if request.request_id.trim().is_empty()
            || request.request_id.trim() != request.request_id
            || request.prompt.trim().is_empty()
            || request.attempt != 0
        {
            return Err(ProviderError::InvalidPolicy(
                "new request identity and prompt must be normalized and attempt must be zero"
                    .to_owned(),
            ));
        }
        if !request.structured_output_schema.is_object() {
            return Err(ProviderError::InvalidPolicy(
                "structured output schema must be a JSON object".to_owned(),
            ));
        }
        if capabilities.structured_output == StructuredOutputSupport::None {
            return Err(ProviderError::InvalidPolicy(
                "provider does not support structured output".to_owned(),
            ));
        }
        if request.evidence_bytes > self.policy.limits.max_evidence_bytes {
            return Err(ProviderError::InvalidPolicy(
                "request evidence exceeds the configured byte limit".to_owned(),
            ));
        }
        if request.estimated_input_tokens == 0
            || request.max_output_tokens == 0
            || request.max_output_tokens > capabilities.max_output_tokens
            || u64::from(request.max_output_tokens) > self.policy.limits.max_output_tokens
        {
            return Err(ProviderError::InvalidPolicy(
                "request token bounds exceed the assigned provider or policy".to_owned(),
            ));
        }
        Ok(())
    }

    fn reserve_request(&self, request: &ModelRequest) -> Result<(), ProviderError> {
        let mut telemetry = lock(&self.telemetry);
        if telemetry.requests >= u64::from(self.policy.limits.max_requests) {
            telemetry.failures += 1;
            return Err(ProviderError::BudgetExceeded(
                "request limit reached".to_owned(),
            ));
        }
        if telemetry
            .input_tokens
            .saturating_add(request.estimated_input_tokens)
            > self.policy.limits.max_input_tokens
        {
            telemetry.failures += 1;
            return Err(ProviderError::BudgetExceeded(
                "input token limit reached".to_owned(),
            ));
        }
        if telemetry
            .output_tokens
            .saturating_add(u64::from(request.max_output_tokens))
            > self.policy.limits.max_output_tokens
        {
            telemetry.failures += 1;
            return Err(ProviderError::BudgetExceeded(
                "output token limit reached".to_owned(),
            ));
        }
        telemetry.requests += 1;
        Ok(())
    }

    fn validate_response_identity(
        &self,
        request: &ModelRequest,
        response: &ModelResponse,
    ) -> Result<(), ProviderError> {
        if response.request_id != request.request_id
            || response.attempt != request.attempt
            || response.provider != self.expected_identity
        {
            self.mark_failure();
            return Err(ProviderError::SubstitutionDenied(
                "response request or provider/model identity changed in flight".to_owned(),
            ));
        }
        Ok(())
    }

    fn record_usage(
        &self,
        request: &ModelRequest,
        response: &ModelResponse,
    ) -> Result<(), ProviderError> {
        let mut telemetry = lock(&self.telemetry);
        let input_tokens = response
            .input_tokens
            .unwrap_or(request.estimated_input_tokens);
        if response.input_tokens.is_none() || response.output_tokens.is_none() {
            telemetry.unreported_token_responses += 1;
        }
        let output_tokens = response
            .output_tokens
            .unwrap_or(u64::from(request.max_output_tokens));
        telemetry.input_tokens = telemetry.input_tokens.saturating_add(input_tokens);
        telemetry.output_tokens = telemetry.output_tokens.saturating_add(output_tokens);
        if let Some(cost) = response.estimated_cost_microusd {
            telemetry.estimated_cost_microusd =
                telemetry.estimated_cost_microusd.saturating_add(cost);
        } else {
            telemetry.unreported_cost_responses += 1;
        }
        let cost_exceeded = self
            .policy
            .limits
            .max_estimated_cost_microusd
            .is_some_and(|limit| telemetry.estimated_cost_microusd > limit);
        if telemetry.input_tokens > self.policy.limits.max_input_tokens
            || telemetry.output_tokens > self.policy.limits.max_output_tokens
            || cost_exceeded
        {
            telemetry.failures += 1;
            return Err(ProviderError::BudgetExceeded(
                "provider reported usage beyond the configured cumulative limit".to_owned(),
            ));
        }
        Ok(())
    }

    fn begin_wait(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.waiting += 1;
        telemetry.peak_waiting = telemetry.peak_waiting.max(telemetry.waiting);
    }

    fn end_wait_and_begin_call(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.waiting = telemetry.waiting.saturating_sub(1);
        telemetry.in_flight += 1;
        telemetry.peak_in_flight = telemetry.peak_in_flight.max(telemetry.in_flight);
    }

    fn mark_failure(&self) {
        lock(&self.telemetry).failures += 1;
    }
}

struct ActiveCall<'a> {
    telemetry: &'a Mutex<ProviderTelemetry>,
}

impl<'a> ActiveCall<'a> {
    const fn new(telemetry: &'a Mutex<ProviderTelemetry>) -> Self {
        Self { telemetry }
    }
}

impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        let mut telemetry = lock(self.telemetry);
        telemetry.in_flight = telemetry.in_flight.saturating_sub(1);
    }
}

fn repair_prompt(prompt: &str, schema: &Value, error: &str) -> String {
    format!(
        "{prompt}\n\nYour previous response was invalid: {error}. Return only JSON matching this schema: {schema}"
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DataClassification, DeploymentMode, ModelSubstitution, ProviderCapabilities, ReviewLimits,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::{BTreeSet, VecDeque};

    struct FixtureProvider {
        capabilities: ProviderCapabilities,
        health: ProviderHealth,
        responses: Mutex<VecDeque<Result<ModelResponse, ProviderError>>>,
        seen: Mutex<Vec<ModelRequest>>,
    }

    #[derive(Default)]
    struct RecordingTelemetrySink {
        snapshots: Mutex<Vec<(ProviderIdentity, ProviderTelemetry)>>,
    }

    impl ProviderTelemetrySink for RecordingTelemetrySink {
        fn publish(
            &self,
            identity: &ProviderIdentity,
            telemetry: &ProviderTelemetry,
        ) -> Result<(), ProviderError> {
            lock(&self.snapshots).push((identity.clone(), telemetry.clone()));
            Ok(())
        }
    }

    #[async_trait]
    impl ModelProvider for FixtureProvider {
        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }

        async fn health(&self) -> Result<ProviderHealth, ProviderError> {
            Ok(self.health)
        }

        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ProviderError> {
            lock(&self.seen).push(request);
            lock(&self.responses).pop_front().unwrap_or_else(|| {
                Err(ProviderError::Unavailable("no fixture response".to_owned()))
            })
        }
    }

    fn identity() -> ProviderIdentity {
        ProviderIdentity {
            provider: "fixture-local".to_owned(),
            provider_version: "1".to_owned(),
            model: "reviewer".to_owned(),
            model_version: "pinned".to_owned(),
        }
    }

    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            identity: identity(),
            deployment: DeploymentMode::Local,
            context_window_tokens: 16_384,
            max_output_tokens: 2_048,
            structured_output: StructuredOutputSupport::SchemaConstrained,
            tool_calling: false,
            concurrency_capacity: 2,
            supported_classifications: BTreeSet::from([DataClassification::Internal]),
            reports_token_usage: true,
            reports_estimated_cost: true,
        }
    }

    fn policy(max_requests: u32) -> ProviderPolicy {
        ProviderPolicy {
            repository_classification: DataClassification::Internal,
            authorize_online_transmission: false,
            substitution: ModelSubstitution::Pinned,
            limits: ReviewLimits {
                max_requests,
                max_input_tokens: 1_000,
                max_output_tokens: 500,
                max_evidence_bytes: 10_000,
                max_evidence_expansions: 1,
                max_concurrency: 1,
                max_estimated_cost_microusd: Some(100),
            },
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            request_id: "review-1".to_owned(),
            attempt: 0,
            prompt: "Review the bounded evidence.".to_owned(),
            structured_output_schema: json!({"required": ["status"]}),
            evidence_bytes: 1_000,
            estimated_input_tokens: 100,
            max_output_tokens: 100,
        }
    }

    fn response(output: Value, attempt: u32) -> ModelResponse {
        ModelResponse {
            request_id: "review-1".to_owned(),
            attempt,
            output,
            input_tokens: Some(90),
            output_tokens: Some(20),
            estimated_cost_microusd: Some(10),
            provider: identity(),
        }
    }

    fn status_validator() -> Arc<dyn OutputValidator> {
        Arc::new(|_: &Value, output: &Value| {
            output
                .get("status")
                .and_then(Value::as_str)
                .map(|_| ())
                .ok_or_else(|| "`status` must be a string".to_owned())
        })
    }

    fn fixture(
        responses: impl IntoIterator<Item = Result<ModelResponse, ProviderError>>,
    ) -> Arc<FixtureProvider> {
        Arc::new(FixtureProvider {
            capabilities: capabilities(),
            health: ProviderHealth::Ready,
            responses: Mutex::new(responses.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn invalid_output_is_repaired_within_cumulative_budgets() {
        let provider = fixture([
            Ok(response(json!({"wrong": true}), 0)),
            Ok(response(json!({"status": "pass"}), 1)),
        ]);
        let executor = ProviderExecutor::new(
            provider.clone(),
            identity(),
            policy(2),
            RepairPolicy {
                max_repair_attempts: 1,
            },
            status_validator(),
        )
        .unwrap();

        let result = executor.execute(request()).await.unwrap();
        assert_eq!(result.output, json!({"status": "pass"}));
        let telemetry = executor.telemetry();
        assert_eq!(telemetry.requests, 2);
        assert_eq!(telemetry.repair_attempts, 1);
        assert_eq!(telemetry.successes, 1);
        assert_eq!(telemetry.input_tokens, 180);
        assert_eq!(telemetry.last_health, Some(ProviderHealth::Ready));
        assert_eq!(lock(&provider.seen)[1].attempt, 1);
        assert!(lock(&provider.seen)[1].estimated_input_tokens > 100);
        assert!(
            lock(&provider.seen)[1]
                .prompt
                .contains("previous response was invalid")
        );
    }

    #[tokio::test]
    async fn repair_cannot_cross_the_request_budget() {
        let provider = fixture([Ok(response(json!({"wrong": true}), 0))]);
        let executor = ProviderExecutor::new(
            provider,
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 1,
            },
            status_validator(),
        )
        .unwrap();

        assert!(matches!(
            executor.execute(request()).await,
            Err(ProviderError::BudgetExceeded(_))
        ));
        assert_eq!(executor.telemetry().requests, 1);
    }

    #[tokio::test]
    async fn changed_response_identity_and_excess_cost_fail_closed() {
        let mut changed = response(json!({"status": "pass"}), 0);
        changed.provider.model = "substitute".to_owned();
        let provider = fixture([Ok(changed)]);
        let executor = ProviderExecutor::new(
            provider,
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            status_validator(),
        )
        .unwrap();
        assert!(matches!(
            executor.execute(request()).await,
            Err(ProviderError::SubstitutionDenied(_))
        ));

        let mut costly = response(json!({"status": "pass"}), 0);
        costly.estimated_cost_microusd = Some(101);
        let provider = fixture([Ok(costly)]);
        let executor = ProviderExecutor::new(
            provider,
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            status_validator(),
        )
        .unwrap();
        assert!(matches!(
            executor.execute(request()).await,
            Err(ProviderError::BudgetExceeded(_))
        ));
        assert_eq!(executor.telemetry().estimated_cost_microusd, 101);
    }

    #[tokio::test]
    async fn unavailable_health_and_call_failures_are_visible() {
        let provider = Arc::new(FixtureProvider {
            capabilities: capabilities(),
            health: ProviderHealth::Unavailable,
            responses: Mutex::new(VecDeque::new()),
            seen: Mutex::new(Vec::new()),
        });
        let executor = ProviderExecutor::new(
            provider,
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            status_validator(),
        )
        .unwrap();
        assert!(matches!(
            executor.execute(request()).await,
            Err(ProviderError::Unavailable(_))
        ));
        assert_eq!(executor.telemetry().failures, 1);
        assert_eq!(
            executor.telemetry().last_health,
            Some(ProviderHealth::Unavailable)
        );

        let provider = fixture([Err(ProviderError::Unavailable("offline".to_owned()))]);
        let executor = ProviderExecutor::new(
            provider,
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            status_validator(),
        )
        .unwrap();
        assert!(executor.execute(request()).await.is_err());
        let telemetry = executor.telemetry();
        assert_eq!(telemetry.requests, 1);
        assert_eq!(telemetry.failures, 1);
        assert_eq!(telemetry.successes, 0);
    }

    #[tokio::test]
    async fn oversized_evidence_is_rejected_before_provider_execution() {
        let provider = fixture([Ok(response(json!({"status": "pass"}), 0))]);
        let executor = ProviderExecutor::new(
            provider.clone(),
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            status_validator(),
        )
        .unwrap();
        let mut oversized = request();
        oversized.evidence_bytes = 10_001;
        assert!(matches!(
            executor.execute(oversized).await,
            Err(ProviderError::InvalidPolicy(_))
        ));
        assert!(lock(&provider.seen).is_empty());
    }

    #[tokio::test]
    async fn invocation_provider_must_match_the_assigned_capability_profile() {
        let assigned = fixture([Ok(response(json!({"status": "pass"}), 0))]);
        let executor = ProviderExecutor::new(
            assigned,
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            status_validator(),
        )
        .unwrap();
        let mut substituted_capabilities = capabilities();
        substituted_capabilities.identity.model_version = "different".to_owned();
        let substituted = FixtureProvider {
            capabilities: substituted_capabilities,
            health: ProviderHealth::Ready,
            responses: Mutex::new(VecDeque::from([Ok(response(json!({"status": "pass"}), 0))])),
            seen: Mutex::new(Vec::new()),
        };

        assert!(matches!(
            executor
                .execute_with_provider(&substituted, request())
                .await,
            Err(ProviderError::SubstitutionDenied(_))
        ));
        assert!(lock(&substituted.seen).is_empty());
        assert_eq!(executor.telemetry().requests, 0);
    }

    #[tokio::test]
    async fn telemetry_sink_receives_the_post_execution_snapshot() {
        let sink = Arc::new(RecordingTelemetrySink::default());
        let executor = ProviderExecutor::new(
            fixture([Ok(response(json!({"status": "pass"}), 0))]),
            identity(),
            policy(1),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            status_validator(),
        )
        .unwrap()
        .with_telemetry_sink(sink.clone());

        executor.execute(request()).await.unwrap();
        let snapshots = lock(&sink.snapshots);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].0, identity());
        assert_eq!(snapshots[0].1.requests, 1);
        assert_eq!(snapshots[0].1.successes, 1);
        assert!(snapshots[0].1.provider_call_millis >= 1);
        assert_eq!(snapshots[0].1.input_tokens, 90);
        assert_eq!(snapshots[0].1.output_tokens, 20);
    }
}
