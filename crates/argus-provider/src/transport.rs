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
    DeploymentMode, ModelProvider, ModelRequest, ModelResponse, ProviderCapabilities,
    ProviderError, ProviderHealth, StructuredOutputSupport,
};
use async_trait::async_trait;
use langchart_adapters::llm::{FinishReason, LlmAdapter, LlmError, LlmRequest, Message};
use langchart_llm_generic::GenericLlmAdapter;
use langchart_llm_watsonx::{WatsonxAdapter, WatsonxConfig, WatsonxCredentials};
use langchart_model::policy::ModelPolicy;
use std::{sync::Arc, time::Duration};
use url::{Host, Url};

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434/v1";
const LEMONADE_BASE_URL: &str = "http://127.0.0.1:13305/v1";
const LM_STUDIO_BASE_URL: &str = "http://127.0.0.1:1234/v1";

/// Adapts a Langchart wire transport to Argus's governed provider contract.
pub struct LangchartModelProvider {
    capabilities: ProviderCapabilities,
    adapter: Arc<dyn LlmAdapter>,
}

impl LangchartModelProvider {
    pub fn new(
        capabilities: ProviderCapabilities,
        adapter: Arc<dyn LlmAdapter>,
    ) -> Result<Self, ProviderError> {
        capabilities.validate()?;
        if capabilities.structured_output != StructuredOutputSupport::BestEffort
            || capabilities.tool_calling
            || capabilities.reports_estimated_cost
        {
            return Err(ProviderError::InvalidProfile(
                "Langchart transport bridge supports best-effort JSON without tools or cost reporting"
                    .to_owned(),
            ));
        }
        Ok(Self {
            capabilities,
            adapter,
        })
    }

    #[must_use]
    pub fn adapter(&self) -> Arc<dyn LlmAdapter> {
        self.adapter.clone()
    }

    pub fn openai_compatible(
        capabilities: ProviderCapabilities,
        base_url: &str,
        api_key: Option<String>,
    ) -> Result<Self, ProviderError> {
        Self::openai_compatible_with_timeout(capabilities, base_url, api_key, None)
    }

    fn openai_compatible_with_timeout(
        capabilities: ProviderCapabilities,
        base_url: &str,
        api_key: Option<String>,
        request_timeout_seconds: Option<u64>,
    ) -> Result<Self, ProviderError> {
        if capabilities.identity.model.starts_with("claude") {
            return Err(ProviderError::InvalidProfile(
                "OpenAI-compatible transport cannot use a Claude-prefixed model because Langchart routes it to the Anthropic API"
                    .to_owned(),
            ));
        }
        let endpoint = validate_endpoint(base_url, capabilities.deployment)?;
        let mut builder = GenericLlmAdapter::builder().openai_base_url(endpoint.as_str());
        if let Some(api_key) = api_key {
            validate_secret("OpenAI-compatible API key", &api_key)?;
            builder = builder.openai_api_key(api_key);
        }
        if let Some(seconds) = request_timeout_seconds {
            if seconds == 0 {
                return Err(ProviderError::InvalidProfile(
                    "provider request timeout must be positive".to_owned(),
                ));
            }
            builder = builder.timeout(Duration::from_secs(seconds));
        }
        let adapter = builder
            .build()
            .map_err(|error| ProviderError::InvalidProfile(error.to_string()))?;
        Self::new(capabilities, Arc::new(adapter))
    }

    pub fn openai(
        capabilities: ProviderCapabilities,
        api_key: String,
    ) -> Result<Self, ProviderError> {
        require_deployment(&capabilities, DeploymentMode::Online, "OpenAI")?;
        Self::openai_compatible(capabilities, OPENAI_BASE_URL, Some(api_key))
    }

    pub fn ollama(
        capabilities: ProviderCapabilities,
        base_url: Option<&str>,
    ) -> Result<Self, ProviderError> {
        require_private_deployment(&capabilities, "Ollama")?;
        Self::openai_compatible(capabilities, base_url.unwrap_or(OLLAMA_BASE_URL), None)
    }

    pub fn lemonade(
        capabilities: ProviderCapabilities,
        base_url: Option<&str>,
        api_key: Option<String>,
    ) -> Result<Self, ProviderError> {
        Self::lemonade_with_timeout(capabilities, base_url, api_key, None)
    }

    pub fn lemonade_with_timeout(
        capabilities: ProviderCapabilities,
        base_url: Option<&str>,
        api_key: Option<String>,
        request_timeout_seconds: Option<u64>,
    ) -> Result<Self, ProviderError> {
        require_private_deployment(&capabilities, "Lemonade")?;
        Self::openai_compatible_with_timeout(
            capabilities,
            base_url.unwrap_or(LEMONADE_BASE_URL),
            api_key,
            request_timeout_seconds,
        )
    }

    pub fn lm_studio(
        capabilities: ProviderCapabilities,
        base_url: Option<&str>,
        api_key: Option<String>,
    ) -> Result<Self, ProviderError> {
        require_private_deployment(&capabilities, "LM Studio")?;
        Self::openai_compatible(
            capabilities,
            base_url.unwrap_or(LM_STUDIO_BASE_URL),
            api_key,
        )
    }

    pub fn anthropic(
        capabilities: ProviderCapabilities,
        api_key: String,
    ) -> Result<Self, ProviderError> {
        if capabilities.deployment != DeploymentMode::Online
            || !capabilities.identity.model.starts_with("claude")
        {
            return Err(ProviderError::InvalidProfile(
                "Anthropic transport requires online deployment and a Claude model".to_owned(),
            ));
        }
        validate_secret("Anthropic API key", &api_key)?;
        let adapter = GenericLlmAdapter::builder()
            .anthropic_api_key(api_key)
            .build()
            .map_err(|error| ProviderError::InvalidProfile(error.to_string()))?;
        Self::new(capabilities, Arc::new(adapter))
    }

    pub fn watsonx(
        capabilities: ProviderCapabilities,
        config: WatsonxConfig,
        credentials: WatsonxCredentials,
    ) -> Result<Self, ProviderError> {
        require_deployment(&capabilities, DeploymentMode::Online, "watsonx")?;
        let adapter = WatsonxAdapter::new(config, credentials)
            .map_err(|error| ProviderError::InvalidProfile(error.to_string()))?;
        Self::new(capabilities, Arc::new(adapter))
    }
}

fn require_deployment(
    capabilities: &ProviderCapabilities,
    expected: DeploymentMode,
    provider: &str,
) -> Result<(), ProviderError> {
    if capabilities.deployment == expected {
        Ok(())
    } else {
        Err(ProviderError::InvalidProfile(format!(
            "{provider} transport requires {expected:?} deployment"
        )))
    }
}

fn require_private_deployment(
    capabilities: &ProviderCapabilities,
    provider: &str,
) -> Result<(), ProviderError> {
    if matches!(
        capabilities.deployment,
        DeploymentMode::Local | DeploymentMode::SameNetwork
    ) {
        Ok(())
    } else {
        Err(ProviderError::InvalidProfile(format!(
            "{provider} transport requires local or same-network deployment"
        )))
    }
}

fn validate_endpoint(value: &str, deployment: DeploymentMode) -> Result<Url, ProviderError> {
    if value.trim() != value || value.ends_with('/') {
        return Err(ProviderError::InvalidProfile(
            "provider endpoint must be normalized without a trailing slash".to_owned(),
        ));
    }
    let endpoint = Url::parse(value).map_err(|error| {
        ProviderError::InvalidProfile(format!("provider endpoint is invalid: {error}"))
    })?;
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ProviderError::InvalidProfile(
            "provider endpoint cannot contain credentials, query, or fragment".to_owned(),
        ));
    }
    match deployment {
        DeploymentMode::Local if !is_loopback(&endpoint) => Err(ProviderError::InvalidProfile(
            "local provider endpoint must use localhost or a loopback address".to_owned(),
        )),
        DeploymentMode::Online if endpoint.scheme() != "https" => Err(
            ProviderError::InvalidProfile("online provider endpoint must use HTTPS".to_owned()),
        ),
        _ if !matches!(endpoint.scheme(), "http" | "https") => Err(ProviderError::InvalidProfile(
            "provider endpoint must use HTTP or HTTPS".to_owned(),
        )),
        _ => Ok(endpoint),
    }
}

fn is_loopback(endpoint: &Url) -> bool {
    match endpoint.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn validate_secret(name: &str, value: &str) -> Result<(), ProviderError> {
    if value.trim().is_empty() || value.trim() != value {
        Err(ProviderError::InvalidProfile(format!(
            "{name} must be non-empty and normalized"
        )))
    } else {
        Ok(())
    }
}

#[async_trait]
impl ModelProvider for LangchartModelProvider {
    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        let models = self.adapter.list_models().await.map_err(map_error)?;
        if models.is_empty() {
            return Ok(ProviderHealth::Degraded);
        }
        let identity = &self.capabilities.identity;
        if models
            .iter()
            .any(|model| model.id == identity.model || model.id == identity.model_version)
        {
            Ok(ProviderHealth::Ready)
        } else {
            Ok(ProviderHealth::Unavailable)
        }
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ProviderError> {
        let response = self
            .adapter
            .complete(LlmRequest {
                model_policy: ModelPolicy {
                    profile: None,
                    model: Some(self.capabilities.identity.model.clone()),
                    temperature: Some(0.0),
                    max_tokens: Some(request.max_output_tokens),
                },
                messages: vec![
                    Message::System {
                        content: format!(
                            "Return only JSON matching this schema: {}",
                            request.structured_output_schema
                        ),
                    },
                    Message::User {
                        content: request.prompt,
                    },
                ],
                tools: Vec::new(),
            })
            .await
            .map_err(map_error)?;
        if response.model != self.capabilities.identity.model_version {
            return Err(ProviderError::SubstitutionDenied(format!(
                "transport returned model `{}` instead of pinned `{}`",
                response.model, self.capabilities.identity.model_version
            )));
        }
        match response.finish_reason {
            FinishReason::Stop => {}
            FinishReason::Length => {
                return Err(ProviderError::BudgetExceeded(
                    "model output reached its token limit".to_owned(),
                ));
            }
            FinishReason::ContentFilter => {
                return Err(ProviderError::InvalidOutput(
                    "model output was filtered".to_owned(),
                ));
            }
            FinishReason::ToolCalls | FinishReason::Other(_) => {
                return Err(ProviderError::InvalidOutput(
                    "transport did not return a terminal JSON response".to_owned(),
                ));
            }
        }
        if !response.tool_calls.is_empty() {
            return Err(ProviderError::InvalidOutput(
                "model tool calls are not authorized for review".to_owned(),
            ));
        }
        let content = response.content.ok_or_else(|| {
            ProviderError::InvalidOutput("model response has no JSON content".to_owned())
        })?;
        let output = parse_json_response(&content).map_err(|error| {
            ProviderError::InvalidOutput(format!("model response is not valid JSON: {error}"))
        })?;
        Ok(ModelResponse {
            request_id: request.request_id,
            attempt: request.attempt,
            output,
            input_tokens: Some(u64::from(response.usage.prompt_tokens)),
            output_tokens: Some(u64::from(response.usage.completion_tokens)),
            estimated_cost_microusd: None,
            provider: self.capabilities.identity.clone(),
        })
    }
}

fn parse_json_response(content: &str) -> Result<serde_json::Value, serde_json::Error> {
    let trimmed = content.trim();
    let json = trimmed
        .strip_prefix("```json")
        .and_then(|fenced| fenced.strip_suffix("```"))
        .map_or(trimmed, str::trim);
    serde_json::from_str(json)
}

fn map_error(error: LlmError) -> ProviderError {
    match error {
        LlmError::ContextLengthExceeded => {
            ProviderError::BudgetExceeded("provider context window exceeded".to_owned())
        }
        LlmError::ContentFiltered => {
            ProviderError::InvalidOutput("provider filtered the response".to_owned())
        }
        LlmError::RateLimited(message) => {
            ProviderError::Unavailable(format!("provider rate limited the request: {message}"))
        }
        LlmError::ModelNotFound { model } => {
            ProviderError::Unavailable(format!("provider model `{model}` was not found"))
        }
        LlmError::Provider(message) => ProviderError::Unavailable(message),
        LlmError::Timeout => ProviderError::Unavailable("provider request timed out".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataClassification, DeploymentMode, ProviderIdentity, WatsonxScope};
    use langchart_adapters::llm::{LlmResponse, ModelInfo, TokenUsage, ToolCall};
    use serde_json::json;
    use std::{
        collections::{BTreeSet, VecDeque},
        sync::Mutex,
    };

    struct FixtureAdapter {
        models: Vec<ModelInfo>,
        responses: Mutex<VecDeque<Result<LlmResponse, LlmError>>>,
        seen: Mutex<Vec<LlmRequest>>,
    }

    #[async_trait]
    impl LlmAdapter for FixtureAdapter {
        async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
            self.seen.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(LlmError::Provider("no fixture response".to_owned())))
        }

        async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
            Ok(self.models.clone())
        }
    }

    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            identity: ProviderIdentity {
                provider: "openai-compatible".to_owned(),
                provider_version: "langchart-generic@0.1.0".to_owned(),
                model: "reviewer".to_owned(),
                model_version: "reviewer@sha256:fixture".to_owned(),
            },
            deployment: DeploymentMode::Local,
            context_window_tokens: 16_384,
            max_output_tokens: 2_048,
            structured_output: StructuredOutputSupport::BestEffort,
            tool_calling: false,
            concurrency_capacity: 1,
            supported_classifications: BTreeSet::from([DataClassification::Internal]),
            reports_token_usage: true,
            reports_estimated_cost: false,
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            request_id: "review-1".to_owned(),
            attempt: 0,
            prompt: "Review the bounded evidence.".to_owned(),
            structured_output_schema: json!({"type": "object"}),
            evidence_bytes: 100,
            estimated_input_tokens: 50,
            max_output_tokens: 200,
        }
    }

    fn response(content: &str, model: &str) -> LlmResponse {
        LlmResponse {
            content: Some(content.to_owned()),
            tool_calls: Vec::new(),
            usage: TokenUsage {
                prompt_tokens: 42,
                completion_tokens: 7,
                total_tokens: 49,
            },
            finish_reason: FinishReason::Stop,
            model: model.to_owned(),
        }
    }

    fn adapter(
        models: impl IntoIterator<Item = &'static str>,
        responses: impl IntoIterator<Item = Result<LlmResponse, LlmError>>,
    ) -> Arc<FixtureAdapter> {
        Arc::new(FixtureAdapter {
            models: models
                .into_iter()
                .map(|id| ModelInfo {
                    id: id.to_owned(),
                    description: None,
                })
                .collect(),
            responses: Mutex::new(responses.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn health_reports_exact_missing_and_unenumerated_models() {
        let exact =
            LangchartModelProvider::new(capabilities(), adapter(["reviewer"], std::iter::empty()))
                .unwrap();
        assert_eq!(exact.health().await.unwrap(), ProviderHealth::Ready);

        let missing =
            LangchartModelProvider::new(capabilities(), adapter(["different"], std::iter::empty()))
                .unwrap();
        assert_eq!(missing.health().await.unwrap(), ProviderHealth::Unavailable);

        let unenumerated =
            LangchartModelProvider::new(capabilities(), adapter([], std::iter::empty())).unwrap();
        assert_eq!(
            unenumerated.health().await.unwrap(),
            ProviderHealth::Degraded
        );
    }

    #[tokio::test]
    async fn completion_preserves_identity_usage_schema_and_json() {
        let transport = adapter(
            ["reviewer"],
            [Ok(response(
                r#"{"event_type":"review.pass","payload":{"result_ref":"assessment:1"}}"#,
                "reviewer@sha256:fixture",
            ))],
        );
        let provider = LangchartModelProvider::new(capabilities(), transport.clone()).unwrap();
        let result = provider.complete(request()).await.unwrap();

        assert_eq!(result.request_id, "review-1");
        assert_eq!(result.input_tokens, Some(42));
        assert_eq!(result.output_tokens, Some(7));
        assert_eq!(result.output["event_type"], "review.pass");
        let seen = transport.seen.lock().unwrap();
        assert_eq!(seen[0].model_policy.model.as_deref(), Some("reviewer"));
        assert_eq!(seen[0].tools.len(), 0);
        let Message::System { content } = &seen[0].messages[0] else {
            panic!("first transport message must contain the trusted schema");
        };
        assert!(content.contains(r#""type":"object""#));
    }

    #[tokio::test]
    async fn completion_accepts_only_a_whole_response_json_fence() {
        let fenced = LangchartModelProvider::new(
            capabilities(),
            adapter(
                ["reviewer"],
                [Ok(response(
                    "```json\n{\"event_type\":\"review.pass\",\"payload\":{}}\n```",
                    "reviewer@sha256:fixture",
                ))],
            ),
        )
        .unwrap();
        assert_eq!(
            fenced.complete(request()).await.unwrap().output["event_type"],
            "review.pass"
        );

        let prose = LangchartModelProvider::new(
            capabilities(),
            adapter(
                ["reviewer"],
                [Ok(response(
                    "Result:\n```json\n{}\n```",
                    "reviewer@sha256:fixture",
                ))],
            ),
        )
        .unwrap();
        assert!(matches!(
            prose.complete(request()).await,
            Err(ProviderError::InvalidOutput(_))
        ));
    }

    #[tokio::test]
    async fn model_substitution_invalid_json_and_tool_calls_fail_closed() {
        let substituted = LangchartModelProvider::new(
            capabilities(),
            adapter(["reviewer"], [Ok(response("{}", "unexpected-model"))]),
        )
        .unwrap();
        assert!(matches!(
            substituted.complete(request()).await,
            Err(ProviderError::SubstitutionDenied(_))
        ));

        let invalid = LangchartModelProvider::new(
            capabilities(),
            adapter(
                ["reviewer"],
                [Ok(response("not-json", "reviewer@sha256:fixture"))],
            ),
        )
        .unwrap();
        assert!(matches!(
            invalid.complete(request()).await,
            Err(ProviderError::InvalidOutput(_))
        ));

        let mut tool_response = response("{}", "reviewer@sha256:fixture");
        tool_response.tool_calls.push(ToolCall {
            id: "call-1".to_owned(),
            name: "publish".to_owned(),
            arguments: json!({}),
        });
        let tools =
            LangchartModelProvider::new(capabilities(), adapter(["reviewer"], [Ok(tool_response)]))
                .unwrap();
        assert!(matches!(
            tools.complete(request()).await,
            Err(ProviderError::InvalidOutput(_))
        ));
    }

    #[test]
    fn concrete_transport_constructors_enforce_deployment_boundaries() {
        assert!(
            LangchartModelProvider::openai_compatible(
                capabilities(),
                "http://127.0.0.1:11434/v1",
                None,
            )
            .is_ok()
        );
        assert!(
            LangchartModelProvider::openai_compatible(
                capabilities(),
                "https://api.openai.com/v1",
                None,
            )
            .is_err()
        );
        assert!(
            LangchartModelProvider::openai_compatible(
                capabilities(),
                "http://user:secret@localhost:11434/v1",
                None,
            )
            .is_err()
        );

        let mut online = capabilities();
        online.deployment = DeploymentMode::Online;
        assert!(
            LangchartModelProvider::openai_compatible(
                online.clone(),
                "http://api.openai.com/v1",
                Some("secret".to_owned()),
            )
            .is_err()
        );
        assert!(
            LangchartModelProvider::openai_compatible(
                online,
                "https://api.openai.com/v1",
                Some("secret".to_owned()),
            )
            .is_ok()
        );

        let mut anthropic = capabilities();
        anthropic.deployment = DeploymentMode::Online;
        anthropic.identity.model = "claude-reviewer".to_owned();
        anthropic.identity.model_version = "claude-reviewer@pinned".to_owned();
        assert!(LangchartModelProvider::anthropic(anthropic, "secret".to_owned()).is_ok());

        let local = capabilities();
        assert!(LangchartModelProvider::ollama(local.clone(), None).is_ok());
        assert!(LangchartModelProvider::lemonade(local.clone(), None, None).is_ok());
        assert!(
            LangchartModelProvider::lemonade_with_timeout(local.clone(), None, None, Some(300))
                .is_ok()
        );
        assert!(
            LangchartModelProvider::lemonade_with_timeout(local.clone(), None, None, Some(0))
                .is_err()
        );
        assert!(LangchartModelProvider::lm_studio(local, None, None).is_ok());

        let mut openai = capabilities();
        openai.deployment = DeploymentMode::Online;
        assert!(LangchartModelProvider::openai(openai, "secret".to_owned()).is_ok());

        let mut misrouted = capabilities();
        misrouted.identity.model = "claude-local".to_owned();
        assert!(LangchartModelProvider::ollama(misrouted, None).is_err());

        let mut watsonx = capabilities();
        watsonx.deployment = DeploymentMode::Online;
        assert!(
            LangchartModelProvider::watsonx(
                watsonx,
                WatsonxConfig::new(
                    "https://us-south.ml.cloud.ibm.com",
                    "2024-05-31",
                    WatsonxScope::Project("project-1".to_owned()),
                ),
                WatsonxCredentials::ApiKey("secret".to_owned()),
            )
            .is_ok()
        );
    }
}
