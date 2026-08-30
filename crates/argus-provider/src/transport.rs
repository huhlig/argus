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
use langchart_adapters::llm::{
    FinishReason, LlmAdapter, LlmError, LlmRequest, Message, ResponseFormat,
};
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
        if capabilities.structured_output == StructuredOutputSupport::None
            || capabilities.tool_calling
            || capabilities.reports_estimated_cost
        {
            return Err(ProviderError::InvalidProfile(
                "Langchart transport bridge requires structured JSON without tools or cost reporting"
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
        let seconds = request_timeout_seconds.unwrap_or(1800);
        if seconds == 0 {
            return Err(ProviderError::InvalidProfile(
                "provider request timeout must be positive".to_owned(),
            ));
        }
        let timeout = Duration::from_secs(seconds);
        builder = builder
            .timeout(timeout)
            .first_byte_timeout(timeout)
            .stream_idle_timeout(timeout);
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

    pub fn bedrock(
        capabilities: ProviderCapabilities,
        config: crate::BedrockConfig,
        credentials: crate::BedrockCredentials,
    ) -> Result<Self, ProviderError> {
        require_deployment(&capabilities, DeploymentMode::Online, "bedrock")?;
        let adapter = crate::BedrockAdapter::new(config, credentials)
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
        DeploymentMode::Local if !is_loopback_or_private(&endpoint) => Err(ProviderError::InvalidProfile(
            "local provider endpoint must use localhost, loopback, or a private network address".to_owned(),
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

fn is_loopback_or_private(endpoint: &Url) -> bool {
    match endpoint.host() {
        Some(Host::Domain(host)) => {
            let lower = host.to_ascii_lowercase();
            lower == "localhost"
                || lower.strip_suffix(".local").is_some()
                || lower.strip_suffix(".internal").is_some()
        }
        Some(Host::Ipv4(address)) => address.is_loopback() || address.is_private(),
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

#[allow(clippy::too_many_lines)]
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
        let strategy = StructuredOutputStrategy::for_provider(
            &self.capabilities.identity.provider,
            self.capabilities.structured_output,
        );
        let response_format = match strategy {
            StructuredOutputStrategy::PromptGuidedText => ResponseFormat::Text,
            StructuredOutputStrategy::NativeJsonObject => ResponseFormat::JsonObject,
            StructuredOutputStrategy::NativeJsonSchema => ResponseFormat::JsonSchema {
                name: "argus_review".to_owned(),
                description: Some("Argus policy review decision".to_owned()),
                schema: request.structured_output_schema.clone(),
                strict: true,
            },
        };
        let system_content = match strategy {
            StructuredOutputStrategy::NativeJsonSchema => {
                "Return only JSON matching the supplied response schema.".to_owned()
            }
            StructuredOutputStrategy::NativeJsonObject | StructuredOutputStrategy::PromptGuidedText => {
                format!(
                    "Return only JSON matching this schema: {}",
                    request.structured_output_schema
                )
            }
        };
        tracing::debug!(
            provider = %self.capabilities.identity.provider,
            model = %self.capabilities.identity.model,
            system_prompt = %system_content,
            prompt = %request.prompt,
            "Sending prompt to LLM provider"
        );
        let response = self
            .adapter
            .complete(LlmRequest {
                model_policy: ModelPolicy {
                    profile: None,
                    model: Some(self.capabilities.identity.model.clone()),
                    temperature: Some(0.0),
                    max_tokens: Some(request.max_output_tokens.min(self.capabilities.max_output_tokens)),
                },
                messages: vec![
                    Message::System {
                        content: system_content,
                    },
                    Message::User {
                        content: request.prompt,
                    },
                ],
                tools: Vec::new(),
                response_format,
            })
            .await
            .map_err(map_error)?;
        tracing::debug!(
            provider = %self.capabilities.identity.provider,
            model = %self.capabilities.identity.model,
            response_model = %response.model,
            content = response.content.as_deref().unwrap_or(""),
            finish_reason = ?response.finish_reason,
            "Received response from LLM provider"
        );
        if response.model != self.capabilities.identity.model
            && response.model != self.capabilities.identity.model_version
            && self.capabilities.identity.model_version != "latest"
        {
            return Err(ProviderError::SubstitutionDenied(format!(
                "transport returned model `{}` instead of expected `{}`",
                response.model, self.capabilities.identity.model
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
        if let Some(refusal) = response.refusal {
            return Err(ProviderError::InvalidOutput(format!(
                "model refused the review request: {refusal}"
            )));
        }
        let content = response.content.ok_or_else(|| {
            ProviderError::InvalidOutput("model response has no JSON content".to_owned())
        })?;
        let output = sanitize_and_parse_model_output(&content).map_err(|error| {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuredOutputStrategy {
    NativeJsonSchema,
    NativeJsonObject,
    PromptGuidedText,
}

impl StructuredOutputStrategy {
    #[must_use]
    pub fn for_provider(provider: &str, support: StructuredOutputSupport) -> Self {
        match support {
            StructuredOutputSupport::None => Self::PromptGuidedText,
            StructuredOutputSupport::BestEffort => match provider {
                "bedrock" | "anthropic" | "watsonx" => Self::PromptGuidedText,
                _ => Self::NativeJsonObject,
            },
            StructuredOutputSupport::SchemaConstrained => match provider {
                "bedrock" | "anthropic" | "watsonx" => Self::PromptGuidedText,
                _ => Self::NativeJsonSchema,
            },
        }
    }
}

fn sanitize_and_parse_model_output(content: &str) -> Result<serde_json::Value, serde_json::Error> {
    let trimmed = content.trim();

    // 1. Strip reasoning / thinking blocks: <think>...</think>
    let unthought = if let (Some(start), Some(end)) = (trimmed.find("<think>"), trimmed.find("</think>")) {
        let before = &trimmed[..start];
        let after = &trimmed[end + "</think>".len()..];
        format!("{}{}", before.trim(), after.trim())
    } else {
        trimmed.to_owned()
    };

    let text = unthought.trim();

    // 2. Strip whole-response markdown fences: ```json ... ``` or ``` ... ```
    let json = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .and_then(|fenced| fenced.strip_suffix("```"))
        .map_or(text, str::trim);

    // 3. Try strict JSON parse first
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
        return Ok(value);
    }

    // 4. Try lenient JSON5 parse
    if let Ok(value) = json5::from_str::<serde_json::Value>(json) {
        return Ok(value);
    }

    // 5. Repair common model formatting issues (unquoted keys, trailing commas)
    let repaired = repair_json_syntax(json);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&repaired) {
        return Ok(value);
    }

    serde_json::from_str(json)
}

fn repair_json_syntax(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 64);
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape = false;

    while let Some(c) = chars.next() {
        if escape {
            out.push(c);
            escape = false;
            continue;
        }

        if c == '\\' && in_string {
            out.push(c);
            escape = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            out.push(c);
            continue;
        }

        if in_string {
            out.push(c);
            continue;
        }

        // Single quotes converted to double quotes for strings
        if c == '\'' {
            out.push('"');
            continue;
        }

        // Strip single-line comments // ...
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for nc in chars.by_ref() {
                if nc == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }

        // Strip trailing commas before } or ]
        if c == ',' {
            let mut whitespace = String::new();
            let mut found_closer = false;
            while let Some(&next_c) = chars.peek() {
                if next_c.is_whitespace() {
                    whitespace.push(chars.next().unwrap());
                } else if next_c == '}' || next_c == ']' {
                    found_closer = true;
                    break;
                } else {
                    break;
                }
            }
            if found_closer {
                out.push_str(&whitespace);
            } else {
                out.push(',');
                out.push_str(&whitespace);
            }
            continue;
        }

        // Quote unquoted alphanumeric/ident keys before a colon
        if c.is_alphanumeric() || c == '_' || c == '-' {
            let mut ident = String::new();
            ident.push(c);
            while let Some(&next_c) = chars.peek() {
                if next_c.is_alphanumeric() || next_c == '_' || next_c == '-' {
                    ident.push(chars.next().unwrap());
                } else {
                    break;
                }
            }

            // Check if following token is ':'
            let mut whitespace = String::new();
            while let Some(&next_c) = chars.peek() {
                if next_c.is_whitespace() {
                    whitespace.push(chars.next().unwrap());
                } else {
                    break;
                }
            }

            if chars.peek() == Some(&':') {
                out.push('"');
                out.push_str(&ident);
                out.push('"');
            } else if ident == "true" || ident == "false" || ident == "null" {
                out.push_str(&ident);
            } else if ident.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
                out.push_str(&ident);
            } else {
                out.push_str(&ident);
            }
            out.push_str(&whitespace);
            continue;
        }

        out.push(c);
    }

    out
}

#[allow(clippy::match_wildcard_for_single_variants)]
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
        LlmError::UnsupportedResponseFormat { adapter, requested } => {
            ProviderError::InvalidProfile(format!(
                "adapter `{adapter}` cannot honor required response format `{requested}`"
            ))
        }
        LlmError::Provider(message) => ProviderError::Unavailable(message),
        other => ProviderError::Unavailable(other.to_string()),
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
        #[allow(clippy::result_large_err)]
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
            refusal: None,
            model: model.to_owned(),
            reported_model: None,
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
        assert_eq!(seen[0].response_format, ResponseFormat::JsonObject);
        let Message::System { content } = &seen[0].messages[0] else {
            panic!("first transport message must contain the trusted schema");
        };
        assert!(content.contains(r#""type":"object""#));
    }

    #[tokio::test]
    async fn schema_constrained_capability_uses_the_native_schema_contract() {
        let transport = adapter(
            ["reviewer"],
            [Ok(response("{}", "reviewer@sha256:fixture"))],
        );
        let mut native = capabilities();
        native.structured_output = StructuredOutputSupport::SchemaConstrained;
        let provider = LangchartModelProvider::new(native, transport.clone()).unwrap();

        provider.complete(request()).await.unwrap();

        let seen = transport.seen.lock().unwrap();
        let ResponseFormat::JsonSchema {
            name,
            schema,
            strict,
            ..
        } = &seen[0].response_format
        else {
            panic!("schema-constrained capability must use native JSON Schema");
        };
        assert_eq!(name, "argus_review");
        assert_eq!(schema, &json!({"type": "object"}));
        assert!(*strict);
        let Message::System { content } = &seen[0].messages[0] else {
            panic!("first transport message must contain trusted instructions");
        };
        assert!(!content.contains(r#""type":"object""#));
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

        let mut caps = capabilities();
        caps.identity.model = "Qwen3.6-35B-A3B-GGUF".to_owned();
        caps.identity.model_version = "latest".to_owned();
        let latest_provider = LangchartModelProvider::new(
            caps,
            adapter(
                ["Qwen3.6-35B-A3B-GGUF"],
                [Ok(response("{}", "Qwen3.6-35B-A3B-GGUF"))],
            ),
        )
        .unwrap();
        assert!(latest_provider.complete(request()).await.is_ok());

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

        let mut refusal_response = response("", "reviewer@sha256:fixture");
        refusal_response.content = None;
        refusal_response.refusal = Some("declined".to_owned());
        let refusal = LangchartModelProvider::new(
            capabilities(),
            adapter(["reviewer"], [Ok(refusal_response)]),
        )
        .unwrap();
        assert!(matches!(
            refusal.complete(request()).await,
            Err(ProviderError::InvalidOutput(message)) if message.contains("declined")
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

    #[test]
    fn sanitize_and_parse_model_output_handles_varied_formats() {
        // 1. Clean JSON
        let clean = r#"{"status": "pass", "claims": []}"#;
        let parsed = sanitize_and_parse_model_output(clean).unwrap();
        assert_eq!(parsed["status"], "pass");

        // 2. Markdown fence with json tag
        let fenced = "```json\n{\n  \"status\": \"pass\",\n  \"claims\": []\n}\n```";
        let parsed = sanitize_and_parse_model_output(fenced).unwrap();
        assert_eq!(parsed["status"], "pass");

        // 3. Markdown fence without tag
        let generic_fenced = "```\n{\n  \"status\": \"candidate_findings\"\n}\n```";
        let parsed = sanitize_and_parse_model_output(generic_fenced).unwrap();
        assert_eq!(parsed["status"], "candidate_findings");

        // 4. Thinking tags from DeepSeek/Qwen/Nova CoT models
        let thinking = "<think>\nLet's evaluate documentation thoroughly.\nDone.\n</think>\n```json\n{\"status\": \"pass\"}\n```";
        let parsed = sanitize_and_parse_model_output(thinking).unwrap();
        assert_eq!(parsed["status"], "pass");

        // 5. Surrounding whitespace around fence
        let padded = "   \n```json\n{\"status\": \"unable_to_verify\"}\n```\n   ";
        let parsed = sanitize_and_parse_model_output(padded).unwrap();
        assert_eq!(parsed["status"], "unable_to_verify");

        // 6. Lenient JSON: unquoted keys, comments, trailing commas
        let lenient = "{\n  status: 'pass',\n  // note\n  claims: [],\n}";
        let parsed = sanitize_and_parse_model_output(lenient).unwrap();
        assert_eq!(parsed["status"], "pass");
    }

    #[test]
    fn structured_output_strategy_selection() {
        assert_eq!(
            StructuredOutputStrategy::for_provider("bedrock", StructuredOutputSupport::BestEffort),
            StructuredOutputStrategy::PromptGuidedText
        );
        assert_eq!(
            StructuredOutputStrategy::for_provider("anthropic", StructuredOutputSupport::BestEffort),
            StructuredOutputStrategy::PromptGuidedText
        );
        assert_eq!(
            StructuredOutputStrategy::for_provider("lemonade", StructuredOutputSupport::BestEffort),
            StructuredOutputStrategy::NativeJsonObject
        );
        assert_eq!(
            StructuredOutputStrategy::for_provider("openai", StructuredOutputSupport::SchemaConstrained),
            StructuredOutputStrategy::NativeJsonSchema
        );
    }
}
