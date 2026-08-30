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
    DataClassification, DeploymentMode, LangchartModelProvider, ModelSubstitution,
    ProviderCapabilities, ProviderError, ProviderIdentity, ProviderPolicy, RepairPolicy,
    ReviewLimits, StructuredOutputSupport, WatsonxConfig, WatsonxCredentials, WatsonxScope,
};
use langchart_adapters::llm::LlmAdapter;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub const PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION: u32 = 1;
pub const PROVIDER_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    #[serde(default = "default_context_window")]
    pub context_window_tokens: u32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default)]
    pub structured_output: Option<StructuredOutputSupport>,
    #[serde(default = "default_concurrency_capacity")]
    pub concurrency_capacity: u32,
    #[serde(default)]
    pub aliases: Vec<String>,
}

const fn default_context_window() -> u32 {
    131_072
}

const fn default_max_output_tokens() -> u32 {
    8_192
}

const fn default_concurrency_capacity() -> u32 {
    1
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default = "default_provider_config_schema_version")]
    pub schema_version: u32,
    pub provider: String,
    pub transport: ProviderTransportProfile,
    #[serde(default)]
    pub default_policy: Option<ProviderPolicy>,
    #[serde(default)]
    pub default_repair: Option<RepairPolicy>,
    #[serde(default)]
    pub models: BTreeMap<String, ProviderModelConfig>,
}

const fn default_provider_config_schema_version() -> u32 {
    PROVIDER_CONFIG_SCHEMA_VERSION
}

impl ProviderConfig {
    pub fn resolve_runtime_profile(
        &self,
        model_selector: Option<&str>,
    ) -> Result<ProviderRuntimeProfile, ProviderError> {
        let (model_id, model_cfg) = self.select_model(model_selector)?;
        let deployment = self.transport.infer_deployment_mode();
        let structured_output = model_cfg.structured_output.unwrap_or_else(|| {
            match &self.transport {
                ProviderTransportProfile::Openai { .. } => {
                    StructuredOutputSupport::SchemaConstrained
                }
                _ => StructuredOutputSupport::BestEffort,
            }
        });

        let identity = ProviderIdentity {
            provider: self.provider.clone(),
            provider_version: format!("{}@1", self.provider),
            model: model_id.clone(),
            model_version: model_id.clone(),
        };

        let capabilities = ProviderCapabilities {
            identity,
            deployment,
            context_window_tokens: model_cfg.context_window_tokens,
            max_output_tokens: model_cfg.max_output_tokens,
            structured_output,
            tool_calling: false,
            concurrency_capacity: model_cfg.concurrency_capacity,
            supported_classifications: std::collections::BTreeSet::from([
                DataClassification::Internal,
            ]),
            reports_token_usage: true,
            reports_estimated_cost: false,
        };

        let policy = self.default_policy.clone().unwrap_or_else(|| ProviderPolicy {
            repository_classification: DataClassification::Internal,
            authorize_online_transmission: deployment == DeploymentMode::Online,
            substitution: ModelSubstitution::Pinned,
            limits: ReviewLimits {
                max_requests: 20,
                max_input_tokens: 1_000_000,
                max_output_tokens: 163_840,
                max_evidence_bytes: 10_000_000,
                max_evidence_expansions: 0,
                max_concurrency: model_cfg.concurrency_capacity,
                max_estimated_cost_microusd: None,
            },
        });

        let repair = self
            .default_repair
            .clone()
            .unwrap_or(RepairPolicy { max_repair_attempts: 1 });

        let profile = ProviderRuntimeProfile {
            schema_version: PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
            capabilities,
            policy,
            repair,
            transport: self.transport.clone(),
        };

        profile.capabilities.validate()?;
        profile.policy.authorize(&profile.capabilities)?;
        Ok(profile)
    }

    fn select_model(
        &self,
        selector: Option<&str>,
    ) -> Result<(&String, &ProviderModelConfig), ProviderError> {
        if self.models.is_empty() {
            return Err(ProviderError::InvalidProfile(format!(
                "provider `{}` has no configured models",
                self.provider
            )));
        }

        if let Some(target) = selector.map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(entry) = self.models.get_key_value(target) {
                return Ok(entry);
            }
            for (id, cfg) in &self.models {
                if cfg.aliases.iter().any(|alias| alias.eq_ignore_ascii_case(target)) {
                    return Ok((id, cfg));
                }
            }
            for (id, cfg) in &self.models {
                if id.eq_ignore_ascii_case(target) {
                    return Ok((id, cfg));
                }
            }
            return Err(ProviderError::InvalidProfile(format!(
                "model `{target}` not found for provider `{}`. Configured models: {}",
                self.provider,
                self.models.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }

        for (id, cfg) in &self.models {
            if cfg.aliases.iter().any(|alias| alias.eq_ignore_ascii_case("default")) {
                return Ok((id, cfg));
            }
        }
        Ok(self.models.iter().next().unwrap())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderRuntimeProfile {
    pub schema_version: u32,
    pub capabilities: ProviderCapabilities,
    pub policy: ProviderPolicy,
    pub repair: RepairPolicy,
    pub transport: ProviderTransportProfile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderTransportProfile {
    Anthropic {
        #[serde(alias = "api_key_env")]
        api_key: String,
    },
    Openai {
        #[serde(alias = "api_key_env")]
        api_key: String,
    },
    Ollama {
        base_url: Option<String>,
    },
    Lemonade {
        base_url: Option<String>,
        #[serde(default, alias = "api_key_env")]
        api_key: Option<String>,
        #[serde(default)]
        request_timeout_seconds: Option<u64>,
    },
    LmStudio {
        base_url: Option<String>,
        #[serde(default, alias = "api_key_env")]
        api_key: Option<String>,
    },
    Watsonx {
        service_url: String,
        api_version: String,
        scope: WatsonxScopeProfile,
        credential: WatsonxCredentialProfile,
    },
    Bedrock {
        #[serde(default = "default_bedrock_region")]
        region: String,
        #[serde(default, alias = "access_key_id_env")]
        access_key_id: Option<String>,
        #[serde(default, alias = "secret_access_key_env")]
        secret_access_key: Option<String>,
        #[serde(default, alias = "session_token_env")]
        session_token: Option<String>,
        #[serde(default, alias = "bearer_token_env")]
        bearer_token: Option<String>,
        #[serde(default)]
        endpoint_url: Option<String>,
        #[serde(default)]
        profile_name: Option<String>,
    },
}

fn default_bedrock_region() -> String {
    "${AWS_REGION:-us-east-1}".to_owned()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum WatsonxScopeProfile {
    Project(String),
    Space(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WatsonxCredentialProfile {
    ApiKey(String),
    BearerToken(String),
}

pub struct BuiltProviderRuntime {
    pub provider: Arc<LangchartModelProvider>,
    pub adapter: Arc<dyn LlmAdapter>,
}

impl ProviderRuntimeProfile {
    pub fn build_from_environment(&self) -> Result<BuiltProviderRuntime, ProviderError> {
        self.build_with_secrets(|name| std::env::var(name).ok())
    }

    pub fn build_with_secrets(
        &self,
        mut read_secret: impl FnMut(&str) -> Option<String>,
    ) -> Result<BuiltProviderRuntime, ProviderError> {
        if self.schema_version != PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION {
            return Err(ProviderError::InvalidProfile(format!(
                "unsupported provider runtime profile schema version {}",
                self.schema_version
            )));
        }
        self.capabilities.validate()?;
        self.policy.authorize(&self.capabilities)?;
        let expected_provider = self.transport.provider_name();
        if self.capabilities.identity.provider != expected_provider {
            return Err(ProviderError::InvalidProfile(format!(
                "transport `{expected_provider}` does not match provider identity `{}`",
                self.capabilities.identity.provider
            )));
        }
        let provider = match &self.transport {
            ProviderTransportProfile::Anthropic { api_key } => {
                let key = substitute_value(api_key, &mut read_secret)?;
                LangchartModelProvider::anthropic(self.capabilities.clone(), key)
            }
            ProviderTransportProfile::Openai { api_key } => {
                let key = substitute_value(api_key, &mut read_secret)?;
                LangchartModelProvider::openai(self.capabilities.clone(), key)
            }
            ProviderTransportProfile::Ollama { base_url } => {
                let base_url = substitute_optional_value(base_url.as_deref(), &mut read_secret)?;
                LangchartModelProvider::ollama(self.capabilities.clone(), base_url.as_deref())
            }
            ProviderTransportProfile::Lemonade {
                base_url,
                api_key,
                request_timeout_seconds,
            } => {
                let base_url = substitute_optional_value(base_url.as_deref(), &mut read_secret)?;
                let api_key = substitute_optional_value(api_key.as_deref(), &mut read_secret)?;
                LangchartModelProvider::lemonade_with_timeout(
                    self.capabilities.clone(),
                    base_url.as_deref(),
                    api_key,
                    *request_timeout_seconds,
                )
            }
            ProviderTransportProfile::LmStudio { base_url, api_key } => {
                let base_url = substitute_optional_value(base_url.as_deref(), &mut read_secret)?;
                let api_key = substitute_optional_value(api_key.as_deref(), &mut read_secret)?;
                LangchartModelProvider::lm_studio(
                    self.capabilities.clone(),
                    base_url.as_deref(),
                    api_key,
                )
            }
            ProviderTransportProfile::Watsonx {
                service_url,
                api_version,
                scope,
                credential,
            } => {
                let service_url = substitute_value(service_url, &mut read_secret)?;
                let api_version = substitute_value(api_version, &mut read_secret)?;
                let resolved_scope = scope.resolve(&mut read_secret)?;
                let resolved_credential = credential.resolve(&mut read_secret)?;
                LangchartModelProvider::watsonx(
                    self.capabilities.clone(),
                    WatsonxConfig {
                        service_url,
                        api_version,
                        scope: resolved_scope,
                    },
                    resolved_credential,
                )
            }
            ProviderTransportProfile::Bedrock {
                region,
                access_key_id,
                secret_access_key,
                session_token,
                bearer_token,
                endpoint_url,
                profile_name,
            } => {
                let region = substitute_value(region, &mut read_secret)?;
                let endpoint_url =
                    substitute_optional_value(endpoint_url.as_deref(), &mut read_secret)?;
                let profile_name =
                    substitute_optional_value(profile_name.as_deref(), &mut read_secret)?;

                let bearer_token =
                    substitute_optional_value(bearer_token.as_deref(), &mut read_secret)?
                        .or_else(|| read_secret("AWS_BEARER_TOKEN_BEDROCK"));
                let access_key =
                    substitute_optional_value(access_key_id.as_deref(), &mut read_secret)?;
                let secret_key =
                    substitute_optional_value(secret_access_key.as_deref(), &mut read_secret)?;
                let session_token =
                    substitute_optional_value(session_token.as_deref(), &mut read_secret)?;

                let credentials = if let Some(token) = bearer_token {
                    crate::BedrockCredentials::BearerToken(token)
                } else {
                    match (access_key, secret_key) {
                        (Some(ak), Some(sk)) => crate::BedrockCredentials::Static {
                            access_key_id: ak,
                            secret_access_key: sk,
                            session_token,
                        },
                        (None, None) => crate::BedrockCredentials::EnvironmentOrProfile,
                        _ => {
                            return Err(ProviderError::InvalidProfile(
                                "both access_key_id and secret_access_key must be supplied for static credentials"
                                    .to_owned(),
                            ));
                        }
                    }
                };

                LangchartModelProvider::bedrock(
                    self.capabilities.clone(),
                    crate::BedrockConfig {
                        region,
                        endpoint_url,
                        profile_name,
                    },
                    credentials,
                )
            }
        }?;
        let adapter = provider.adapter();
        Ok(BuiltProviderRuntime {
            provider: Arc::new(provider),
            adapter,
        })
    }
}

impl ProviderTransportProfile {
    pub const fn provider_name(&self) -> &'static str {
        match self {
            Self::Anthropic { .. } => "anthropic",
            Self::Openai { .. } => "openai",
            Self::Ollama { .. } => "ollama",
            Self::Lemonade { .. } => "lemonade",
            Self::LmStudio { .. } => "lmstudio",
            Self::Watsonx { .. } => "watsonx",
            Self::Bedrock { .. } => "bedrock",
        }
    }

    #[must_use]
    pub fn infer_deployment_mode(&self) -> DeploymentMode {
        match self {
            Self::Ollama { base_url } => {
                base_url.as_deref().map(crate::infer_deployment_mode).unwrap_or(DeploymentMode::Local)
            }
            Self::Lemonade { base_url, .. } => {
                base_url.as_deref().map(crate::infer_deployment_mode).unwrap_or(DeploymentMode::Local)
            }
            Self::LmStudio { base_url, .. } => {
                base_url.as_deref().map(crate::infer_deployment_mode).unwrap_or(DeploymentMode::Local)
            }
            Self::Anthropic { .. }
            | Self::Openai { .. }
            | Self::Watsonx { .. }
            | Self::Bedrock { .. } => DeploymentMode::Online,
        }
    }
}

impl WatsonxScopeProfile {
    fn resolve(
        &self,
        mut read_secret: impl FnMut(&str) -> Option<String>,
    ) -> Result<WatsonxScope, ProviderError> {
        match self {
            Self::Project(id) => substitute_value(id, &mut read_secret).map(WatsonxScope::Project),
            Self::Space(id) => substitute_value(id, &mut read_secret).map(WatsonxScope::Space),
        }
    }
}

impl WatsonxCredentialProfile {
    fn resolve(
        &self,
        mut read_secret: impl FnMut(&str) -> Option<String>,
    ) -> Result<WatsonxCredentials, ProviderError> {
        match self {
            Self::ApiKey(raw) => {
                substitute_value(raw, &mut read_secret).map(WatsonxCredentials::ApiKey)
            }
            Self::BearerToken(raw) => {
                substitute_value(raw, &mut read_secret).map(WatsonxCredentials::BearerToken)
            }
        }
    }
}

struct FnVariableMap<'a, F>(&'a std::cell::RefCell<&'a mut F>);

impl<'a, F> subst::VariableMap<'_> for FnVariableMap<'a, F>
where
    F: FnMut(&str) -> Option<String>,
{
    type Value = String;

    fn get(&self, key: &str) -> Option<Self::Value> {
        (self.0.borrow_mut())(key)
    }
}

pub fn substitute_value(
    raw: &str,
    read_secret: &mut impl FnMut(&str) -> Option<String>,
) -> Result<String, ProviderError> {
    if raw.trim().is_empty() {
        return Err(ProviderError::InvalidProfile(
            "configuration value cannot be empty".to_owned(),
        ));
    }

    if raw.contains('$') {
        // Normalize bash-style `${VAR:-default}` to subst-style `${VAR:default}`
        let normalized = if raw.contains(":-") {
            raw.replace(":-", ":")
        } else {
            raw.to_owned()
        };
        let cell = std::cell::RefCell::new(read_secret);
        let map = FnVariableMap(&cell);
        let substituted = subst::substitute(&normalized, &map).map_err(|err| {
            ProviderError::InvalidProfile(format!(
                "variable substitution failed for `{raw}`: {err}"
            ))
        })?;
        if substituted.trim().is_empty() {
            return Err(ProviderError::InvalidProfile(format!(
                "variable substitution for `{raw}` resulted in an empty value"
            )));
        }
        Ok(substituted)
    } else if looks_like_env_var_name(raw) {
        if let Some(val) = read_secret(raw) {
            if val.trim().is_empty() {
                return Err(ProviderError::InvalidProfile(format!(
                    "environment variable `{raw}` contains an empty value"
                )));
            }
            Ok(val)
        } else {
            Ok(raw.to_owned())
        }
    } else {
        // Literal value
        Ok(raw.to_owned())
    }
}

fn looks_like_env_var_name(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }
    s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub fn substitute_optional_value(
    raw: Option<&str>,
    read_secret: &mut impl FnMut(&str) -> Option<String>,
) -> Result<Option<String>, ProviderError> {
    match raw {
        Some(s) if !s.trim().is_empty() => substitute_value(s, read_secret).map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DataClassification, DeploymentMode, ModelProvider, ModelSubstitution, ProviderIdentity,
        ReviewLimits, StructuredOutputSupport,
    };
    use std::collections::BTreeSet;

    fn profile(transport: ProviderTransportProfile) -> ProviderRuntimeProfile {
        let provider = transport.provider_name();
        let deployment = if matches!(
            transport,
            ProviderTransportProfile::Ollama { .. }
                | ProviderTransportProfile::Lemonade { .. }
                | ProviderTransportProfile::LmStudio { .. }
        ) {
            DeploymentMode::Local
        } else {
            DeploymentMode::Online
        };
        let model = if provider == "anthropic" {
            "claude-reviewer"
        } else {
            "reviewer"
        };
        ProviderRuntimeProfile {
            schema_version: PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
            capabilities: ProviderCapabilities {
                identity: ProviderIdentity {
                    provider: provider.to_owned(),
                    provider_version: "langchart@1".to_owned(),
                    model: model.to_owned(),
                    model_version: format!("{model}@pinned"),
                },
                deployment,
                context_window_tokens: 16_384,
                max_output_tokens: 2_048,
                structured_output: StructuredOutputSupport::BestEffort,
                tool_calling: false,
                concurrency_capacity: 1,
                supported_classifications: BTreeSet::from([DataClassification::Internal]),
                reports_token_usage: true,
                reports_estimated_cost: false,
            },
            policy: ProviderPolicy {
                repository_classification: DataClassification::Internal,
                authorize_online_transmission: true,
                substitution: ModelSubstitution::Pinned,
                limits: ReviewLimits {
                    max_requests: 10,
                    max_input_tokens: 100_000,
                    max_output_tokens: 20_480,
                    max_evidence_bytes: 1_000_000,
                    max_evidence_expansions: 0,
                    max_concurrency: 1,
                    max_estimated_cost_microusd: None,
                },
            },
            repair: RepairPolicy {
                max_repair_attempts: 1,
            },
            transport,
        }
    }

    #[test]
    fn all_supported_transports_build_without_persisting_secrets() {
        let profiles = [
            profile(ProviderTransportProfile::Anthropic {
                api_key: "${ANTHROPIC_API_KEY}".to_owned(),
            }),
            profile(ProviderTransportProfile::Openai {
                api_key: "${OPENAI_API_KEY}".to_owned(),
            }),
            profile(ProviderTransportProfile::Ollama { base_url: None }),
            profile(ProviderTransportProfile::Lemonade {
                base_url: None,
                api_key: Some("${LEMONADE_API_KEY}".to_owned()),
                request_timeout_seconds: None,
            }),
            profile(ProviderTransportProfile::LmStudio {
                base_url: None,
                api_key: None,
            }),
            profile(ProviderTransportProfile::Watsonx {
                service_url: "https://us-south.ml.cloud.ibm.com".to_owned(),
                api_version: "2024-05-31".to_owned(),
                scope: WatsonxScopeProfile::Project("project-1".to_owned()),
                credential: WatsonxCredentialProfile::ApiKey("${WATSONX_API_KEY}".to_owned()),
            }),
            profile(ProviderTransportProfile::Bedrock {
                region: "${AWS_REGION:-us-east-1}".to_owned(),
                access_key_id: Some("${AWS_ACCESS_KEY_ID}".to_owned()),
                secret_access_key: Some("${AWS_SECRET_ACCESS_KEY}".to_owned()),
                session_token: None,
                bearer_token: None,
                endpoint_url: None,
                profile_name: None,
            }),
        ];
        for value in profiles {
            let serialized = serde_json::to_string(&value).unwrap();
            assert!(!serialized.contains("super-secret"));
            let runtime = value
                .build_with_secrets(|_| Some("super-secret".to_owned()))
                .unwrap();
            assert_eq!(
                runtime.provider.capabilities().identity,
                value.capabilities.identity
            );
        }
    }

    #[test]
    fn missing_secrets_and_transport_identity_mismatches_fail_closed() {
        let missing = profile(ProviderTransportProfile::Openai {
            api_key: "${OPENAI_API_KEY}".to_owned(),
        });
        assert!(missing.build_with_secrets(|_| None).is_err());

        let mut mismatch = profile(ProviderTransportProfile::Ollama { base_url: None });
        mismatch.capabilities.identity.provider = "openai".to_owned();
        assert!(
            mismatch
                .build_with_secrets(|_| Some("unused".to_owned()))
                .is_err()
        );
    }

    #[test]
    fn subst_variable_substitution_handles_defaults_and_literals() {
        let mut mock_env = |var: &str| match var {
            "MY_TOKEN" => Some("secret-token-123".to_owned()),
            "CUSTOM_REGION" => Some("eu-west-1".to_owned()),
            _ => None,
        };

        assert_eq!(
            substitute_value("${MY_TOKEN}", &mut mock_env).unwrap(),
            "secret-token-123"
        );
        assert_eq!(
            substitute_value("${CUSTOM_REGION:-us-east-1}", &mut mock_env).unwrap(),
            "eu-west-1"
        );
        assert_eq!(
            substitute_value("${FALLBACK_REGION:-us-east-1}", &mut mock_env).unwrap(),
            "us-east-1"
        );
        assert_eq!(
            substitute_value("literal-api-key-999", &mut mock_env).unwrap(),
            "literal-api-key-999"
        );
    }

    #[test]
    fn provider_config_resolves_exact_model_and_aliases() {
        let mut models = BTreeMap::new();
        models.insert(
            "anthropic.claude-3-haiku-20240307-v1:0".to_owned(),
            ProviderModelConfig {
                context_window_tokens: 200_000,
                max_output_tokens: 4_096,
                structured_output: Some(StructuredOutputSupport::BestEffort),
                concurrency_capacity: 4,
                aliases: vec!["haiku".to_owned(), "claude-haiku".to_owned()],
            },
        );
        models.insert(
            "anthropic.claude-3-7-sonnet-20250219-v1:0".to_owned(),
            ProviderModelConfig {
                context_window_tokens: 200_000,
                max_output_tokens: 8_192,
                structured_output: Some(StructuredOutputSupport::BestEffort),
                concurrency_capacity: 2,
                aliases: vec!["sonnet".to_owned(), "default".to_owned()],
            },
        );

        let config = ProviderConfig {
            schema_version: PROVIDER_CONFIG_SCHEMA_VERSION,
            provider: "bedrock".to_owned(),
            transport: ProviderTransportProfile::Bedrock {
                region: "${AWS_REGION:-us-east-1}".to_owned(),
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
                bearer_token: None,
                endpoint_url: None,
                profile_name: None,
            },
            default_policy: None,
            default_repair: None,
            models,
        };

        // 1. Default resolution resolves the one aliased as "default" (sonnet)
        let default_profile = config.resolve_runtime_profile(None).unwrap();
        assert_eq!(
            default_profile.capabilities.identity.model,
            "anthropic.claude-3-7-sonnet-20250219-v1:0"
        );
        assert_eq!(default_profile.capabilities.concurrency_capacity, 2);

        // 2. Alias resolution for "haiku"
        let haiku_profile = config.resolve_runtime_profile(Some("haiku")).unwrap();
        assert_eq!(
            haiku_profile.capabilities.identity.model,
            "anthropic.claude-3-haiku-20240307-v1:0"
        );
        assert_eq!(haiku_profile.capabilities.concurrency_capacity, 4);

        // 3. Exact model ID resolution
        let exact_profile = config
            .resolve_runtime_profile(Some("anthropic.claude-3-haiku-20240307-v1:0"))
            .unwrap();
        assert_eq!(
            exact_profile.capabilities.identity.model,
            "anthropic.claude-3-haiku-20240307-v1:0"
        );

        // 4. Non-existent model fails
        assert!(config.resolve_runtime_profile(Some("non-existent")).is_err());
    }
}
