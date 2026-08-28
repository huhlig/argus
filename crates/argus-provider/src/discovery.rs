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
    DataClassification, DeploymentMode, ModelSubstitution, ProviderCapabilities, ProviderError,
    ProviderIdentity, ProviderPolicy, ProviderRuntimeProfile, ProviderTransportProfile,
    RepairPolicy, ReviewLimits, StructuredOutputSupport,
    runtime_profile::PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
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
        }
    }

    #[must_use]
    pub const fn default_api_key_env(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some("OPENAI_API_KEY"),
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Lemonade | Self::LmStudio | Self::Ollama => None,
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
            _ => Err(ProviderError::InvalidProfile(format!(
                "unsupported provider type `{s}` (supported: lemonade, ollama, openai, anthropic, lm_studio)"
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
    }
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

    let capabilities = ProviderCapabilities {
        identity,
        deployment,
        context_window_tokens: 131_072,
        max_output_tokens: 8_192,
        structured_output: StructuredOutputSupport::BestEffort,
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
            api_key_env,
            request_timeout_seconds: request_timeout_seconds.or(Some(1800)),
        },
        DiscoveredProviderKind::Ollama => ProviderTransportProfile::Ollama {
            base_url: Some(endpoint.to_owned()),
        },
        DiscoveredProviderKind::Openai => ProviderTransportProfile::Openai {
            api_key_env: api_key_env.unwrap_or_else(|| "OPENAI_API_KEY".to_owned()),
        },
        DiscoveredProviderKind::Anthropic => ProviderTransportProfile::Anthropic {
            api_key_env: api_key_env.unwrap_or_else(|| "ANTHROPIC_API_KEY".to_owned()),
        },
        DiscoveredProviderKind::LmStudio => ProviderTransportProfile::LmStudio {
            base_url: Some(endpoint.to_owned()),
            api_key_env,
        },
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
        assert!(matches!(
            profile.transport,
            ProviderTransportProfile::Lemonade {
                request_timeout_seconds: Some(1800),
                ..
            }
        ));
    }
}
