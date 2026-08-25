//! IBM watsonx.ai transport for Langchart.
//!
//! [`WatsonxAdapter`] implements Langchart's [`LlmAdapter`] contract using the
//! watsonx text-chat API. IBM Cloud API keys are exchanged for short-lived IAM
//! tokens and cached in memory until shortly before expiry.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use async_trait::async_trait;
use langchart_adapters::llm::{
    FinishReason, LlmAdapter, LlmError, LlmRequest, LlmResponse, Message, TokenUsage,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use url::Url;

const IAM_TOKEN_URL: &str = "https://iam.cloud.ibm.com/identity/token";
const IAM_GRANT_TYPE: &str = "urn:ibm:params:oauth:grant-type:apikey";
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// A watsonx deployment scope. Exactly one project or space ID is required.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatsonxScope {
    Project(String),
    Space(String),
}

/// Connection settings for an IBM watsonx.ai service instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatsonxConfig {
    pub service_url: String,
    pub api_version: String,
    pub scope: WatsonxScope,
}

impl WatsonxConfig {
    pub fn new(
        service_url: impl Into<String>,
        api_version: impl Into<String>,
        scope: WatsonxScope,
    ) -> Self {
        Self {
            service_url: service_url.into(),
            api_version: api_version.into(),
            scope,
        }
    }
}

/// Authentication material. This type deliberately does not implement `Debug`.
#[derive(Clone)]
pub enum WatsonxCredentials {
    /// An IBM Cloud API key exchanged for a cached IAM bearer token.
    ApiKey(String),
    /// A caller-managed IAM bearer token.
    BearerToken(String),
}

/// Configuration failure detected before a provider call is attempted.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("invalid watsonx configuration: {0}")]
    InvalidConfig(String),
    #[error("failed to build watsonx HTTP client: {0}")]
    Client(String),
}

/// Langchart LLM adapter for the watsonx.ai text-chat API.
pub struct WatsonxAdapter {
    client: Client,
    config: WatsonxConfig,
    credentials: WatsonxCredentials,
    cached_token: Mutex<Option<CachedToken>>,
}

struct CachedToken {
    value: String,
    refresh_at: Instant,
}

impl WatsonxAdapter {
    /// Creates an adapter with a 120-second request timeout.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError`] when the endpoint, API version, scope, or
    /// credentials are malformed, or when the HTTP client cannot be built.
    pub fn new(config: WatsonxConfig, credentials: WatsonxCredentials) -> Result<Self, BuildError> {
        validate_config(&config, &credentials)?;
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|error| BuildError::Client(error.to_string()))?;
        Ok(Self {
            client,
            config,
            credentials,
            cached_token: Mutex::new(None),
        })
    }

    async fn bearer_token(&self) -> Result<String, LlmError> {
        match &self.credentials {
            WatsonxCredentials::BearerToken(token) => Ok(token.clone()),
            WatsonxCredentials::ApiKey(api_key) => {
                let mut cached = self.cached_token.lock().await;
                if let Some(token) = cached.as_ref()
                    && Instant::now() < token.refresh_at
                {
                    return Ok(token.value.clone());
                }

                let response = self
                    .client
                    .post(IAM_TOKEN_URL)
                    .header("Accept", "application/json")
                    .form(&[("grant_type", IAM_GRANT_TYPE), ("apikey", api_key)])
                    .send()
                    .await
                    .map_err(|error| map_reqwest_error(&error))?;
                let response = checked_response(response, "watsonx IAM").await?;
                let token: IamTokenResponse = response.json().await.map_err(|error| {
                    LlmError::Provider(format!("invalid watsonx IAM response: {error}"))
                })?;
                validate_wire_secret("watsonx IAM access token", &token.access_token)?;

                let lifetime = Duration::from_secs(token.expires_in);
                let margin = TOKEN_REFRESH_MARGIN.min(lifetime / 10);
                let refresh_at = Instant::now() + lifetime.saturating_sub(margin);
                *cached = Some(CachedToken {
                    value: token.access_token.clone(),
                    refresh_at,
                });
                Ok(token.access_token)
            }
        }
    }

    fn chat_url(&self) -> String {
        format!(
            "{}/ml/v1/text/chat?version={}",
            self.config.service_url, self.config.api_version
        )
    }
}

#[async_trait]
impl LlmAdapter for WatsonxAdapter {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        if !request.tools.is_empty() {
            return Err(LlmError::Provider(
                "watsonx adapter does not support tool calls".to_owned(),
            ));
        }
        let model =
            request.model_policy.model.clone().ok_or_else(|| {
                LlmError::Provider("watsonx requires an explicit model".to_owned())
            })?;
        let body = WatsonxChatRequest::from_request(&model, &self.config, request)?;
        let response = self
            .client
            .post(self.chat_url())
            .bearer_auth(self.bearer_token().await?)
            .json(&body)
            .send()
            .await
            .map_err(|error| map_reqwest_error(&error))?;
        let response = checked_response(response, "watsonx chat").await?;
        let response: WatsonxChatResponse = response.json().await.map_err(|error| {
            LlmError::Provider(format!("invalid watsonx chat response: {error}"))
        })?;
        response.into_llm_response()
    }
}

#[derive(Serialize)]
struct WatsonxChatRequest {
    model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    space_id: Option<String>,
    messages: Vec<WatsonxMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    response_format: WatsonxResponseFormat,
}

impl WatsonxChatRequest {
    fn from_request(
        model: &str,
        config: &WatsonxConfig,
        request: LlmRequest,
    ) -> Result<Self, LlmError> {
        let (project_id, space_id) = match &config.scope {
            WatsonxScope::Project(value) => (Some(value.clone()), None),
            WatsonxScope::Space(value) => (None, Some(value.clone())),
        };
        let messages = request
            .messages
            .into_iter()
            .map(WatsonxMessage::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            model_id: model.to_owned(),
            project_id,
            space_id,
            messages,
            temperature: request.model_policy.temperature,
            max_tokens: request.model_policy.max_tokens,
            response_format: WatsonxResponseFormat {
                kind: "json_object",
            },
        })
    }
}

#[derive(Serialize)]
struct WatsonxMessage {
    role: &'static str,
    content: String,
}

impl TryFrom<Message> for WatsonxMessage {
    type Error = LlmError;

    fn try_from(message: Message) -> Result<Self, Self::Error> {
        match message {
            Message::System { content } => Ok(Self {
                role: "system",
                content,
            }),
            Message::User { content } => Ok(Self {
                role: "user",
                content,
            }),
            Message::Assistant { content } => Ok(Self {
                role: "assistant",
                content,
            }),
            Message::Tool { .. } => Err(LlmError::Provider(
                "watsonx adapter does not support tool messages".to_owned(),
            )),
        }
    }
}

#[derive(Serialize)]
struct WatsonxResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Deserialize)]
struct IamTokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct WatsonxChatResponse {
    model_id: String,
    model_version: Option<String>,
    choices: Vec<WatsonxChoice>,
    #[serde(default)]
    usage: WatsonxUsage,
}

impl WatsonxChatResponse {
    fn into_llm_response(self) -> Result<LlmResponse, LlmError> {
        let choice = self
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::Provider("watsonx returned no choices".to_owned()))?;
        let prompt_tokens = self.usage.prompt;
        let completion_tokens = self.usage.completion;
        Ok(LlmResponse {
            content: Some(choice.message.content),
            tool_calls: Vec::new(),
            usage: TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens: self
                    .usage
                    .total
                    .unwrap_or(prompt_tokens.saturating_add(completion_tokens)),
            },
            finish_reason: map_finish_reason(choice.finish_reason.as_deref()),
            model: self.model_version.unwrap_or(self.model_id),
        })
    }
}

#[derive(Deserialize)]
struct WatsonxChoice {
    message: WatsonxResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WatsonxResponseMessage {
    content: String,
}

#[derive(Default, Deserialize)]
struct WatsonxUsage {
    #[serde(default, rename = "prompt_tokens", alias = "input_tokens")]
    prompt: u32,
    #[serde(default, rename = "completion_tokens", alias = "generated_tokens")]
    completion: u32,
    #[serde(rename = "total_tokens")]
    total: Option<u32>,
}

fn map_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop" | "eos_token") | None => FinishReason::Stop,
        Some("length" | "max_tokens") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(other) => FinishReason::Other(other.to_owned()),
    }
}

async fn checked_response(
    response: reqwest::Response,
    operation: &str,
) -> Result<reqwest::Response, LlmError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let detail: String = body.chars().take(512).collect();
    match status {
        StatusCode::TOO_MANY_REQUESTS => Err(LlmError::RateLimited(detail)),
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => Err(LlmError::Timeout),
        StatusCode::NOT_FOUND => Err(LlmError::ModelNotFound {
            model: "configured watsonx model".to_owned(),
        }),
        _ => Err(LlmError::Provider(format!(
            "{operation} failed with HTTP {status}: {detail}"
        ))),
    }
}

fn map_reqwest_error(error: &reqwest::Error) -> LlmError {
    if error.is_timeout() {
        LlmError::Timeout
    } else {
        LlmError::Provider(error.to_string())
    }
}

fn validate_config(
    config: &WatsonxConfig,
    credentials: &WatsonxCredentials,
) -> Result<(), BuildError> {
    validate_endpoint(&config.service_url)?;
    validate_api_version(&config.api_version)?;
    validate_scope(&config.scope)?;
    match credentials {
        WatsonxCredentials::ApiKey(value) => validate_secret("watsonx API key", value),
        WatsonxCredentials::BearerToken(value) => validate_secret("watsonx bearer token", value),
    }
}

fn validate_endpoint(value: &str) -> Result<(), BuildError> {
    if value.trim() != value || value.ends_with('/') {
        return Err(BuildError::InvalidConfig(
            "service URL must be normalized without a trailing slash".to_owned(),
        ));
    }
    let endpoint = Url::parse(value)
        .map_err(|error| BuildError::InvalidConfig(format!("invalid service URL: {error}")))?;
    if endpoint.scheme() != "https" {
        return Err(BuildError::InvalidConfig(
            "service URL must use HTTPS".to_owned(),
        ));
    }
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(BuildError::InvalidConfig(
            "service URL cannot contain credentials, query, or fragment".to_owned(),
        ));
    }
    Ok(())
}

fn validate_api_version(value: &str) -> Result<(), BuildError> {
    let bytes = value.as_bytes();
    if bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        Ok(())
    } else {
        Err(BuildError::InvalidConfig(
            "API version must use YYYY-MM-DD".to_owned(),
        ))
    }
}

fn validate_scope(scope: &WatsonxScope) -> Result<(), BuildError> {
    let (name, value) = match scope {
        WatsonxScope::Project(value) => ("watsonx project ID", value),
        WatsonxScope::Space(value) => ("watsonx space ID", value),
    };
    validate_secret(name, value)
}

fn validate_secret(name: &str, value: &str) -> Result<(), BuildError> {
    if value.trim().is_empty() || value.trim() != value {
        Err(BuildError::InvalidConfig(format!(
            "{name} must be non-empty and normalized"
        )))
    } else {
        Ok(())
    }
}

fn validate_wire_secret(name: &str, value: &str) -> Result<(), LlmError> {
    if value.trim().is_empty() || value.trim() != value {
        Err(LlmError::Provider(format!("{name} was empty or malformed")))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use langchart_model::policy::ModelPolicy;
    use serde_json::json;

    fn config(scope: WatsonxScope) -> WatsonxConfig {
        WatsonxConfig::new("https://us-south.ml.cloud.ibm.com", "2024-05-31", scope)
    }

    fn request() -> LlmRequest {
        LlmRequest {
            model_policy: ModelPolicy {
                profile: None,
                model: Some("ibm/granite-4-h-small".to_owned()),
                temperature: Some(0.0),
                max_tokens: Some(512),
            },
            messages: vec![Message::User {
                content: "Return JSON".to_owned(),
            }],
            tools: Vec::new(),
        }
    }

    #[test]
    fn configuration_rejects_unsafe_endpoints_versions_and_secrets() {
        assert!(
            WatsonxAdapter::new(
                config(WatsonxScope::Project("project-1".to_owned())),
                WatsonxCredentials::ApiKey("secret".to_owned()),
            )
            .is_ok()
        );
        let mut insecure = config(WatsonxScope::Project("project-1".to_owned()));
        insecure.service_url = "http://us-south.ml.cloud.ibm.com".to_owned();
        assert!(
            WatsonxAdapter::new(insecure, WatsonxCredentials::ApiKey("secret".to_owned())).is_err()
        );
        let mut invalid_version = config(WatsonxScope::Space("space-1".to_owned()));
        invalid_version.api_version = "latest".to_owned();
        assert!(
            WatsonxAdapter::new(
                invalid_version,
                WatsonxCredentials::BearerToken("token".to_owned())
            )
            .is_err()
        );
    }

    #[test]
    fn request_serialization_selects_one_scope_and_json_output() {
        let body = WatsonxChatRequest::from_request(
            "ibm/granite-4-h-small",
            &config(WatsonxScope::Project("project-1".to_owned())),
            request(),
        )
        .unwrap();
        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["project_id"], "project-1");
        assert!(value.get("space_id").is_none());
        assert_eq!(value["response_format"], json!({"type": "json_object"}));
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["max_tokens"], 512);
    }

    #[test]
    fn response_parsing_preserves_model_usage_and_finish_reason() {
        let response: WatsonxChatResponse = serde_json::from_value(json!({
            "model_id": "ibm/granite-4-h-small",
            "model_version": "4.0.0",
            "choices": [{
                "message": {"content": "{\"result\":\"pass\"}"},
                "finish_reason": "max_tokens"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 5, "total_tokens": 25}
        }))
        .unwrap();
        let response = response.into_llm_response().unwrap();
        assert_eq!(response.model, "4.0.0");
        assert_eq!(response.usage.total_tokens, 25);
        assert_eq!(response.finish_reason, FinishReason::Length);
    }
}
