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
    DataClassification, DeploymentMode, ModelSubstitution, ProviderCapabilities, ProviderConfig,
    ProviderError, ProviderIdentity, ProviderModelConfig, ProviderPolicy, ProviderRuntimeProfile,
    ProviderTransportProfile, RepairPolicy, ReviewLimits, StructuredOutputSupport,
    WatsonxCredentialProfile, WatsonxScopeProfile,
    runtime_profile::{PROVIDER_CONFIG_SCHEMA_VERSION, PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, str::FromStr};
use url::Url;

/// Supported provider kinds for automated model discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveredProviderKind {
    Lemonade,
    Ollama,
    Openai,
    Anthropic,
    LmStudio,
    Bedrock,
    Watsonx,
}

impl DiscoveredProviderKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lemonade => "lemonade",
            Self::Ollama => "ollama",
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::LmStudio => "lm_studio",
            Self::Bedrock => "bedrock",
            Self::Watsonx => "watsonx",
        }
    }

    #[must_use]
    pub const fn default_endpoint(self) -> &'static str {
        match self {
            Self::Lemonade => "http://127.0.0.1:13305/v1",
            Self::Ollama => "http://127.0.0.1:11434",
            Self::Openai => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::LmStudio => "http://127.0.0.1:1234/v1",
            Self::Bedrock => "https://bedrock-runtime.us-east-1.amazonaws.com",
            Self::Watsonx => "https://us-south.ml.cloud.ibm.com",
        }
    }

    #[must_use]
    pub const fn default_api_key_env(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some("OPENAI_API_KEY"),
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Watsonx => Some("WATSONX_API_KEY"),
            Self::Bedrock | Self::Lemonade | Self::LmStudio | Self::Ollama => None,
        }
    }
}

impl FromStr for DiscoveredProviderKind {
    type Err = ProviderError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "lemonade" => Ok(Self::Lemonade),
            "ollama" => Ok(Self::Ollama),
            "openai" => Ok(Self::Openai),
            "anthropic" => Ok(Self::Anthropic),
            "lm_studio" | "lmstudio" | "lm-studio" => Ok(Self::LmStudio),
            "bedrock" | "aws_bedrock" | "aws-bedrock" => Ok(Self::Bedrock),
            "watsonx" | "ibm" | "ibm_watsonx" | "ibm-watsonx" | "watsonx.ai" | "watsonx_ai" => {
                Ok(Self::Watsonx)
            }
            _ => Err(ProviderError::InvalidProfile(format!(
                "unsupported provider type `{s}` (supported: lemonade, ollama, openai, anthropic, lm_studio, bedrock, watsonx)"
            ))),
        }
    }
}

/// Discovers available model IDs from the given provider endpoint.
pub async fn discover_models(
    kind: DiscoveredProviderKind,
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> Result<Vec<String>, ProviderError> {
    let endpoint = endpoint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| kind.default_endpoint());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| ProviderError::Unavailable(format!("cannot build HTTP client: {err}")))?;

    match kind {
        DiscoveredProviderKind::Lemonade
        | DiscoveredProviderKind::LmStudio
        | DiscoveredProviderKind::Openai => {
            discover_openai_compatible_models(&client, endpoint, api_key).await
        }
        DiscoveredProviderKind::Ollama => discover_ollama_models(&client, endpoint).await,
        DiscoveredProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| {
                ProviderError::InvalidProfile(
                    "anthropic model discovery requires an API key (--api-key or --api-key-env)"
                        .to_owned(),
                )
            })?;
            discover_anthropic_models(&client, endpoint, key).await
        }
        DiscoveredProviderKind::Bedrock => {
            discover_openai_compatible_models(&client, endpoint, api_key).await
        }
        DiscoveredProviderKind::Watsonx => {
            discover_watsonx_models(&client, endpoint, api_key).await
        }
    }
}

async fn discover_watsonx_models(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, ProviderError> {
    let token = if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        let key = key.trim();
        if key.starts_with("Bearer ") || key.starts_with("bearer ") {
            key.trim_start_matches("Bearer ")
                .trim_start_matches("bearer ")
                .trim()
                .to_owned()
        } else if key.len() > 100 && !key.contains('_') && !key.contains('-') {
            key.to_owned()
        } else {
            // Exchange IBM Cloud IAM API Key for IAM bearer token
            let params = [
                ("grant_type", "urn:ibm:params:oauth:grant-type:apikey"),
                ("apikey", key),
            ];
            let iam_res = client
                .post("https://iam.cloud.ibm.com/identity/token")
                .form(&params)
                .send()
                .await
                .map_err(|err| {
                    ProviderError::Unavailable(format!("IBM Cloud IAM token exchange failed: {err}"))
                })?;

            if !iam_res.status().is_success() {
                let status = iam_res.status();
                let body = iam_res.text().await.unwrap_or_default();
                return Err(ProviderError::Unavailable(format!(
                    "IBM Cloud IAM authentication failed with HTTP {status}: {body}"
                )));
            }

            let iam_json: serde_json::Value = iam_res.json().await.map_err(|err| {
                ProviderError::InvalidOutput(format!("cannot parse IAM token response: {err}"))
            })?;

            iam_json
                .get("access_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ProviderError::InvalidOutput(
                        "IAM token response missing `access_token`".to_owned(),
                    )
                })?
                .to_owned()
        }
    } else {
        return Err(ProviderError::InvalidProfile(
            "watsonx model discovery requires an API key (--api-key or --api-key-env)".to_owned(),
        ));
    };

    let (base_clean, query_pid) = if let Some((clean, query)) = base_url.split_once('?') {
        let extracted_pid = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("project_id="))
            .map(ToOwned::to_owned);
        (clean.trim_end_matches('/'), extracted_pid)
    } else {
        (base_url.trim_end_matches('/'), None)
    };

    let project_id_env = std::env::var("WATSONX_PROJECT_ID").ok();
    let effective_pid = query_pid.or(project_id_env);

    let mut url = if base_clean.contains("/ml/v1") {
        format!("{base_clean}/foundation_model_specs?version=2023-05-29")
    } else {
        format!("{base_clean}/ml/v1/foundation_model_specs?version=2023-05-29")
    };
    if let Some(pid) = effective_pid.as_deref().filter(|p| !p.trim().is_empty()) {
        url.push_str(&format!("&project_id={}", pid.trim()));
    }

    let response = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|err| {
            ProviderError::Unavailable(format!(
                "failed to query WatsonX models at `{url}`: {err}"
            ))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Unavailable(format!(
            "WatsonX model discovery request to `{url}` returned HTTP {status}: {body}"
        )));
    }

    let json: serde_json::Value = response.json().await.map_err(|err| {
        ProviderError::InvalidOutput(format!("cannot parse JSON from `{url}`: {err}"))
    })?;

    if let Some(resources) = json.get("resources").and_then(|r| r.as_array()) {
        let ids: Vec<String> = resources
            .iter()
            .filter_map(|item| {
                item.get("model_id")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned)
            })
            .collect();
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    parse_model_ids_from_json(&json)
}

async fn discover_openai_compatible_models(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, ProviderError> {
    let base = base_url.trim_end_matches('/');
    let url = if base.ends_with("/models") {
        base.to_owned()
    } else {
        format!("{base}/models")
    };

    let mut request = client.get(&url);
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        request = request.bearer_auth(key.trim());
    }

    let response = request
        .send()
        .await
        .map_err(|err| ProviderError::Unavailable(format!("failed to query `{url}`: {err}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unavailable>".to_owned());
        return Err(ProviderError::Unavailable(format!(
            "model discovery request to `{url}` returned HTTP {status}: {body}"
        )));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|err| ProviderError::InvalidOutput(format!("cannot parse JSON from `{url}`: {err}")))?;

    parse_model_ids_from_json(&json)
}

async fn discover_ollama_models(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<Vec<String>, ProviderError> {
    let base = base_url.trim_end_matches('/');
    let tags_url = format!("{base}/api/tags");

    let response = client.get(&tags_url).send().await;
    if let Ok(resp) = response {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(models) = json.get("models").and_then(|m| m.as_array()) {
                    let ids: Vec<String> = models
                        .iter()
                        .filter_map(|item| {
                            item.get("name")
                                .or_else(|| item.get("model"))
                                .and_then(|v| v.as_str())
                                .map(ToOwned::to_owned)
                        })
                        .collect();
                    if !ids.is_empty() {
                        return Ok(ids);
                    }
                }
            }
        }
    }

    // Fallback: try OpenAI-compatible endpoint
    discover_openai_compatible_models(client, base_url, None).await
}

async fn discover_anthropic_models(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, ProviderError> {
    let base = base_url.trim_end_matches('/');
    let url = if base.ends_with("/models") {
        base.to_owned()
    } else {
        format!("{base}/models")
    };

    let response = client
        .get(&url)
        .header("x-api-key", api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|err| ProviderError::Unavailable(format!("failed to query `{url}`: {err}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unavailable>".to_owned());
        return Err(ProviderError::Unavailable(format!(
            "model discovery request to `{url}` returned HTTP {status}: {body}"
        )));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|err| ProviderError::InvalidOutput(format!("cannot parse JSON from `{url}`: {err}")))?;

    parse_model_ids_from_json(&json)
}

fn parse_model_ids_from_json(json: &serde_json::Value) -> Result<Vec<String>, ProviderError> {
    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        let ids: Vec<String> = data
            .iter()
            .filter_map(|item| item.get("id").and_then(|id| id.as_str()).map(ToOwned::to_owned))
            .collect();
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    if let Some(models) = json.get("models").and_then(|m| m.as_array()) {
        let ids: Vec<String> = models
            .iter()
            .filter_map(|item| {
                item.get("id")
                    .or_else(|| item.get("name"))
                    .or_else(|| item.get("model"))
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned)
            })
            .collect();
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    if let Some(array) = json.as_array() {
        let ids: Vec<String> = array
            .iter()
            .filter_map(|item| {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    Some(id.to_owned())
                } else {
                    item.as_str().map(ToOwned::to_owned)
                }
            })
            .collect();
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    Err(ProviderError::InvalidOutput(
        "response payload contains no recognized model entries".to_owned(),
    ))
}

/// Generates a sanitized profile name/slug from provider kind and model ID.
#[must_use]
pub fn slugify_profile_name(kind: DiscoveredProviderKind, model_id: &str) -> String {
    let mut slug = format!("{}-{}", kind.as_str(), model_id.to_ascii_lowercase());
    slug = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Collapse repeated hyphens
    let mut result = String::with_capacity(slug.len());
    let mut last_was_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !last_was_hyphen {
                result.push(c);
                last_was_hyphen = true;
            }
        } else {
            result.push(c);
            last_was_hyphen = false;
        }
    }
    result.trim_matches('-').to_owned()
}

/// Infers the deployment mode from a given endpoint URL.
#[must_use]
pub fn infer_deployment_mode(endpoint: &str) -> DeploymentMode {
    let Ok(parsed) = Url::parse(endpoint) else {
        return DeploymentMode::Local;
    };

    if let Some(host) = parsed.host_str() {
        if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "0.0.0.0" {
            return DeploymentMode::Local;
        }

        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            match ip {
                std::net::IpAddr::V4(v4) => {
                    if v4.is_loopback() {
                        return DeploymentMode::Local;
                    }
                    if v4.is_private() {
                        return DeploymentMode::SameNetwork;
                    }
                }
                std::net::IpAddr::V6(v6) => {
                    if v6.is_loopback() {
                        return DeploymentMode::Local;
                    }
                }
            }
        }

        if host.ends_with(".local") || host.ends_with(".lan") || host.ends_with(".internal") {
            return DeploymentMode::SameNetwork;
        }
    }

    DeploymentMode::Online
}

/// Builds a complete `ProviderRuntimeProfile` for a discovered model.
pub fn generate_runtime_profile(
    kind: DiscoveredProviderKind,
    endpoint: &str,
    model_id: &str,
    api_key_env: Option<String>,
    request_timeout_seconds: Option<u64>,
) -> Result<ProviderRuntimeProfile, ProviderError> {
    let deployment = infer_deployment_mode(endpoint);
    let host_tag = Url::parse(endpoint)
        .ok()
        .and_then(|u| u.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "1".to_owned());

    let provider_version = format!("{}@{}", kind.as_str(), host_tag);

    let identity = ProviderIdentity {
        provider: kind.as_str().to_owned(),
        provider_version,
        model: model_id.to_owned(),
        model_version: model_id.to_owned(),
    };

    let structured_output = match kind {
        DiscoveredProviderKind::Openai => StructuredOutputSupport::SchemaConstrained,
        DiscoveredProviderKind::Bedrock
        | DiscoveredProviderKind::Anthropic
        | DiscoveredProviderKind::Ollama
        | DiscoveredProviderKind::Lemonade
        | DiscoveredProviderKind::LmStudio
        | DiscoveredProviderKind::Watsonx => StructuredOutputSupport::BestEffort,
    };

    let capabilities = ProviderCapabilities {
        identity,
        deployment,
        context_window_tokens: 131_072,
        max_output_tokens: 8_192,
        structured_output,
        tool_calling: false,
        concurrency_capacity: 1,
        supported_classifications: BTreeSet::from([DataClassification::Internal]),
        reports_token_usage: true,
        reports_estimated_cost: false,
    };

    let authorize_online = deployment == DeploymentMode::Online;

    let policy = ProviderPolicy {
        repository_classification: DataClassification::Internal,
        authorize_online_transmission: authorize_online,
        substitution: ModelSubstitution::Pinned,
        limits: ReviewLimits {
            max_requests: 20,
            max_input_tokens: 1_000_000,
            max_output_tokens: 163_840,
            max_evidence_bytes: 10_000_000,
            max_evidence_expansions: 0,
            max_concurrency: 1,
            max_estimated_cost_microusd: None,
        },
    };

    let repair = RepairPolicy {
        max_repair_attempts: 1,
    };

    let transport = match kind {
        DiscoveredProviderKind::Lemonade => ProviderTransportProfile::Lemonade {
            base_url: Some(endpoint.to_owned()),
            api_key: api_key_env,
            request_timeout_seconds: request_timeout_seconds.or(Some(1800)),
        },
        DiscoveredProviderKind::Ollama => ProviderTransportProfile::Ollama {
            base_url: Some(endpoint.to_owned()),
        },
        DiscoveredProviderKind::Openai => ProviderTransportProfile::Openai {
            api_key: api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .unwrap_or_else(|| "${OPENAI_API_KEY}".to_owned()),
        },
        DiscoveredProviderKind::Anthropic => ProviderTransportProfile::Anthropic {
            api_key: api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .unwrap_or_else(|| "${ANTHROPIC_API_KEY}".to_owned()),
        },
        DiscoveredProviderKind::LmStudio => ProviderTransportProfile::LmStudio {
            base_url: Some(endpoint.to_owned()),
            api_key: api_key_env,
        },
        DiscoveredProviderKind::Bedrock => {
            let region = if let Some(pos) = endpoint.find(".api.aws") {
                let prefix = &endpoint[..pos];
                prefix
                    .split('.')
                    .last()
                    .unwrap_or("us-east-1")
                    .to_owned()
            } else if endpoint.contains("bedrock-runtime.") {
                endpoint
                    .split("bedrock-runtime.")
                    .nth(1)
                    .and_then(|s| s.split('.').next())
                    .unwrap_or("us-east-1")
                    .to_owned()
            } else {
                "${AWS_REGION:-us-east-1}".to_owned()
            };
            let bearer_token = api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .or_else(|| Some("${AWS_BEARER_TOKEN_BEDROCK}".to_owned()));
            ProviderTransportProfile::Bedrock {
                region,
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
                bearer_token,
                endpoint_url: if endpoint == DiscoveredProviderKind::Bedrock.default_endpoint() {
                    None
                } else {
                    Some(endpoint.to_owned())
                },
                profile_name: None,
            }
        }
        DiscoveredProviderKind::Watsonx => {
            let service_url = if endpoint.is_empty() {
                "${WATSONX_AI_ENDPOINT:-https://us-south.ml.cloud.ibm.com}".to_owned()
            } else {
                endpoint.to_owned()
            };
            let api_key_var = api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .unwrap_or_else(|| "${WATSONX_API_KEY}".to_owned());
            ProviderTransportProfile::Watsonx {
                service_url,
                api_version: "2023-05-29".to_owned(),
                scope: WatsonxScopeProfile::Project("${WATSONX_PROJECT_ID}".to_owned()),
                credential: WatsonxCredentialProfile::ApiKey(api_key_var),
            }
        }
    };

    let profile = ProviderRuntimeProfile {
        schema_version: PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
        capabilities,
        policy,
        repair,
        transport,
    };

    profile.capabilities.validate()?;
    profile.policy.authorize(&profile.capabilities)?;

    Ok(profile)
}

/// Helper to produce a simplified model alias from a full model ID.
#[must_use]
pub fn slugify_model_alias(model_id: &str) -> String {
    let trimmed = if let Some(stripped) = model_id.strip_prefix("anthropic.") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("amazon.") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("meta.") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("google.") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("mistral.") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("cohere.") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("ibm/") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("meta-llama/") {
        stripped
    } else if let Some(stripped) = model_id.strip_prefix("mistralai/") {
        stripped
    } else {
        model_id
    };

    let without_version = if let Some(pos) = trimmed.find("-202") {
        &trimmed[..pos]
    } else if let Some(pos) = trimmed.find("-v1:") {
        &trimmed[..pos]
    } else if let Some(pos) = trimmed.find(":latest") {
        &trimmed[..pos]
    } else {
        trimmed
    };

    slugify_profile_name(DiscoveredProviderKind::Openai, without_version)
        .replace("openai-", "")
}

/// Generates a complete ProviderConfig containing all discovered models with aliases.
pub fn generate_provider_config(
    kind: DiscoveredProviderKind,
    endpoint: Option<&str>,
    api_key_env: Option<String>,
    request_timeout_seconds: Option<u64>,
    model_ids: &[String],
) -> Result<ProviderConfig, ProviderError> {
    let endpoint = endpoint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| kind.default_endpoint());

    let transport = match kind {
        DiscoveredProviderKind::Lemonade => ProviderTransportProfile::Lemonade {
            base_url: Some(endpoint.to_owned()),
            api_key: api_key_env,
            request_timeout_seconds: request_timeout_seconds.or(Some(1800)),
        },
        DiscoveredProviderKind::Ollama => ProviderTransportProfile::Ollama {
            base_url: Some(endpoint.to_owned()),
        },
        DiscoveredProviderKind::Openai => ProviderTransportProfile::Openai {
            api_key: api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .unwrap_or_else(|| "${OPENAI_API_KEY}".to_owned()),
        },
        DiscoveredProviderKind::Anthropic => ProviderTransportProfile::Anthropic {
            api_key: api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .unwrap_or_else(|| "${ANTHROPIC_API_KEY}".to_owned()),
        },
        DiscoveredProviderKind::LmStudio => ProviderTransportProfile::LmStudio {
            base_url: Some(endpoint.to_owned()),
            api_key: api_key_env,
        },
        DiscoveredProviderKind::Bedrock => {
            let region = if let Some(pos) = endpoint.find(".api.aws") {
                let prefix = &endpoint[..pos];
                prefix
                    .split('.')
                    .last()
                    .unwrap_or("us-east-1")
                    .to_owned()
            } else if endpoint.contains("bedrock-runtime.") {
                endpoint
                    .split("bedrock-runtime.")
                    .nth(1)
                    .and_then(|s| s.split('.').next())
                    .unwrap_or("us-east-1")
                    .to_owned()
            } else {
                "${AWS_REGION:-us-east-1}".to_owned()
            };
            let bearer_token = api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .or_else(|| Some("${AWS_BEARER_TOKEN_BEDROCK}".to_owned()));
            ProviderTransportProfile::Bedrock {
                region,
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
                bearer_token,
                endpoint_url: if endpoint == DiscoveredProviderKind::Bedrock.default_endpoint() {
                    None
                } else {
                    Some(endpoint.to_owned())
                },
                profile_name: None,
            }
        }
        DiscoveredProviderKind::Watsonx => {
            let service_url = if endpoint.is_empty() {
                "${WATSONX_AI_ENDPOINT:-https://us-south.ml.cloud.ibm.com}".to_owned()
            } else {
                endpoint.to_owned()
            };
            let api_key_var = api_key_env
                .map(|k| if k.starts_with('$') { k } else { format!("${{{k}}}") })
                .unwrap_or_else(|| "${WATSONX_API_KEY}".to_owned());
            ProviderTransportProfile::Watsonx {
                service_url,
                api_version: "2023-05-29".to_owned(),
                scope: WatsonxScopeProfile::Project("${WATSONX_PROJECT_ID}".to_owned()),
                credential: WatsonxCredentialProfile::ApiKey(api_key_var),
            }
        }
    };

    let deployment = transport.infer_deployment_mode();
    let default_structured_output = match kind {
        DiscoveredProviderKind::Openai => StructuredOutputSupport::SchemaConstrained,
        _ => StructuredOutputSupport::BestEffort,
    };

    let mut models = std::collections::BTreeMap::new();
    for (idx, model_id) in model_ids.iter().enumerate() {
        let mut aliases = Vec::new();
        if idx == 0 {
            aliases.push("default".to_owned());
        }
        let alias = slugify_model_alias(model_id);
        if !alias.is_empty()
            && alias != model_id.to_ascii_lowercase()
            && !aliases.contains(&alias)
        {
            aliases.push(alias);
        }

        let (context_window_tokens, max_output_tokens) = if model_id.contains("haiku") {
            (200_000, 4_096)
        } else if model_id.contains("claude") {
            (200_000, 8_192)
        } else if model_id.contains("nova") {
            (300_000, 5_120)
        } else if model_id.contains("llama") {
            (128_000, 8_192)
        } else {
            (131_072, 8_192)
        };

        models.insert(
            model_id.clone(),
            ProviderModelConfig {
                context_window_tokens,
                max_output_tokens,
                structured_output: Some(default_structured_output),
                concurrency_capacity: if deployment == DeploymentMode::Local { 2 } else { 4 },
                aliases,
            },
        );
    }

    Ok(ProviderConfig {
        schema_version: PROVIDER_CONFIG_SCHEMA_VERSION,
        provider: kind.as_str().to_owned(),
        transport,
        default_policy: None,
        default_repair: None,
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_model_ids_from_openai_payload() {
        let payload = json!({
            "object": "list",
            "data": [
                {"id": "gpt-4o", "object": "model"},
                {"id": "gpt-4o-mini", "object": "model"}
            ]
        });
        let ids = parse_model_ids_from_json(&payload).unwrap();
        assert_eq!(ids, vec!["gpt-4o", "gpt-4o-mini"]);
    }

    #[test]
    fn parse_model_ids_from_ollama_payload() {
        let payload = json!({
            "models": [
                {"name": "llama3.2:latest", "model": "llama3.2:latest"},
                {"name": "qwen2.5-coder:32b", "model": "qwen2.5-coder:32b"}
            ]
        });
        let ids = parse_model_ids_from_json(&payload).unwrap();
        assert_eq!(ids, vec!["llama3.2:latest", "qwen2.5-coder:32b"]);
    }

    #[test]
    fn slugify_produces_clean_names() {
        assert_eq!(
            slugify_profile_name(
                DiscoveredProviderKind::Lemonade,
                "Qwen3.6-35B-A3B-GGUF"
            ),
            "lemonade-qwen3.6-35b-a3b-gguf"
        );
        assert_eq!(
            slugify_profile_name(
                DiscoveredProviderKind::Ollama,
                "llama3.2:latest"
            ),
            "ollama-llama3.2-latest"
        );
        assert_eq!(
            slugify_profile_name(
                DiscoveredProviderKind::Openai,
                "gpt-4o"
            ),
            "openai-gpt-4o"
        );
    }

    #[test]
    fn infer_deployment_mode_classifies_correctly() {
        assert_eq!(
            infer_deployment_mode("http://127.0.0.1:13305/v1"),
            DeploymentMode::Local
        );
        assert_eq!(
            infer_deployment_mode("http://localhost:11434"),
            DeploymentMode::Local
        );
        assert_eq!(
            infer_deployment_mode("http://10.0.0.51:13305/v1"),
            DeploymentMode::SameNetwork
        );
        assert_eq!(
            infer_deployment_mode("http://192.168.1.100:1234/v1"),
            DeploymentMode::SameNetwork
        );
        assert_eq!(
            infer_deployment_mode("https://api.openai.com/v1"),
            DeploymentMode::Online
        );
    }

    #[test]
    fn generate_runtime_profile_creates_valid_authorized_profile() {
        let profile = generate_runtime_profile(
            DiscoveredProviderKind::Lemonade,
            "http://10.0.0.51:13305/v1",
            "Qwen3.6-35B-A3B-GGUF",
            None,
            Some(1800),
        )
        .unwrap();

        assert_eq!(profile.capabilities.identity.provider, "lemonade");
        assert_eq!(profile.capabilities.identity.model, "Qwen3.6-35B-A3B-GGUF");
        assert_eq!(profile.capabilities.deployment, DeploymentMode::SameNetwork);
        assert_eq!(
            profile.capabilities.structured_output,
            StructuredOutputSupport::BestEffort
        );
        assert!(matches!(
            profile.transport,
            ProviderTransportProfile::Lemonade {
                request_timeout_seconds: Some(1800),
                ..
            }
        ));

        let bedrock_profile = generate_runtime_profile(
            DiscoveredProviderKind::Bedrock,
            "https://bedrock-runtime.us-west-2.amazonaws.com",
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
            None,
            None,
        )
        .unwrap();

        assert_eq!(bedrock_profile.capabilities.identity.provider, "bedrock");
        assert_eq!(
            bedrock_profile.capabilities.identity.model,
            "anthropic.claude-3-7-sonnet-20250219-v1:0"
        );
        assert_eq!(
            bedrock_profile.capabilities.deployment,
            DeploymentMode::Online
        );
        assert!(matches!(
            bedrock_profile.transport,
            ProviderTransportProfile::Bedrock { region, .. } if region == "us-west-2"
        ));
    }

    #[test]
    fn generate_provider_config_creates_valid_config_and_aliases() {
        let models = vec![
            "anthropic.claude-3-7-sonnet-20250219-v1:0".to_owned(),
            "anthropic.claude-3-haiku-20240307-v1:0".to_owned(),
        ];
        let config = generate_provider_config(
            DiscoveredProviderKind::Bedrock,
            Some("https://bedrock-runtime.us-east-1.amazonaws.com"),
            None,
            None,
            &models,
        )
        .unwrap();

        assert_eq!(config.provider, "bedrock");
        assert_eq!(config.models.len(), 2);

        let sonnet = config.models.get("anthropic.claude-3-7-sonnet-20250219-v1:0").unwrap();
        assert!(sonnet.aliases.contains(&"default".to_owned()));
        assert!(sonnet.aliases.contains(&"claude-3-7-sonnet".to_owned()));

        let haiku = config.models.get("anthropic.claude-3-haiku-20240307-v1:0").unwrap();
        assert!(haiku.aliases.contains(&"claude-3-haiku".to_owned()));

        // Resolve by alias
        let resolved = config.resolve_runtime_profile(Some("claude-3-haiku")).unwrap();
        assert_eq!(resolved.capabilities.identity.model, "anthropic.claude-3-haiku-20240307-v1:0");
    }

    #[test]
    fn generate_provider_config_creates_valid_watsonx_config_and_aliases() {
        let models = vec![
            "ibm/granite-3-8b-instruct".to_owned(),
            "meta-llama/llama-3-3-70b-instruct".to_owned(),
        ];
        let config = generate_provider_config(
            DiscoveredProviderKind::Watsonx,
            Some("https://us-south.ml.cloud.ibm.com"),
            None,
            None,
            &models,
        )
        .unwrap();

        assert_eq!(config.provider, "watsonx");
        assert_eq!(config.models.len(), 2);

        let granite = config.models.get("ibm/granite-3-8b-instruct").unwrap();
        assert!(granite.aliases.contains(&"default".to_owned()));
        assert!(granite.aliases.contains(&"granite-3-8b-instruct".to_owned()));

        let llama = config.models.get("meta-llama/llama-3-3-70b-instruct").unwrap();
        assert!(llama.aliases.contains(&"llama-3-3-70b-instruct".to_owned()));

        // Resolve by alias
        let resolved = config.resolve_runtime_profile(Some("llama-3-3-70b-instruct")).unwrap();
        assert_eq!(resolved.capabilities.identity.model, "meta-llama/llama-3-3-70b-instruct");
    }
}
