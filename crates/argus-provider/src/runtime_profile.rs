use crate::{
    LangchartModelProvider, ProviderCapabilities, ProviderError, ProviderPolicy, RepairPolicy,
    WatsonxConfig, WatsonxCredentials, WatsonxScope,
};
use langchart_adapters::llm::LlmAdapter;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION: u32 = 1;

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
        api_key_env: String,
    },
    Openai {
        api_key_env: String,
    },
    Ollama {
        base_url: Option<String>,
    },
    Lemonade {
        base_url: Option<String>,
        api_key_env: Option<String>,
    },
    LmStudio {
        base_url: Option<String>,
        api_key_env: Option<String>,
    },
    Watsonx {
        service_url: String,
        api_version: String,
        scope: WatsonxScopeProfile,
        credential: WatsonxCredentialProfile,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum WatsonxScopeProfile {
    Project(String),
    Space(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "env", rename_all = "snake_case")]
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
            ProviderTransportProfile::Anthropic { api_key_env } => {
                LangchartModelProvider::anthropic(
                    self.capabilities.clone(),
                    resolve_secret(api_key_env, &mut read_secret)?,
                )
            }
            ProviderTransportProfile::Openai { api_key_env } => LangchartModelProvider::openai(
                self.capabilities.clone(),
                resolve_secret(api_key_env, &mut read_secret)?,
            ),
            ProviderTransportProfile::Ollama { base_url } => {
                LangchartModelProvider::ollama(self.capabilities.clone(), base_url.as_deref())
            }
            ProviderTransportProfile::Lemonade {
                base_url,
                api_key_env,
            } => LangchartModelProvider::lemonade(
                self.capabilities.clone(),
                base_url.as_deref(),
                resolve_optional_secret(api_key_env.as_deref(), &mut read_secret)?,
            ),
            ProviderTransportProfile::LmStudio {
                base_url,
                api_key_env,
            } => LangchartModelProvider::lm_studio(
                self.capabilities.clone(),
                base_url.as_deref(),
                resolve_optional_secret(api_key_env.as_deref(), &mut read_secret)?,
            ),
            ProviderTransportProfile::Watsonx {
                service_url,
                api_version,
                scope,
                credential,
            } => LangchartModelProvider::watsonx(
                self.capabilities.clone(),
                WatsonxConfig::new(service_url, api_version, scope.resolve()),
                credential.resolve(&mut read_secret)?,
            ),
        }?;
        let adapter = provider.adapter();
        Ok(BuiltProviderRuntime {
            provider: Arc::new(provider),
            adapter,
        })
    }
}

impl ProviderTransportProfile {
    const fn provider_name(&self) -> &'static str {
        match self {
            Self::Anthropic { .. } => "anthropic",
            Self::Openai { .. } => "openai",
            Self::Ollama { .. } => "ollama",
            Self::Lemonade { .. } => "lemonade",
            Self::LmStudio { .. } => "lmstudio",
            Self::Watsonx { .. } => "watsonx",
        }
    }
}

impl WatsonxScopeProfile {
    fn resolve(&self) -> WatsonxScope {
        match self {
            Self::Project(id) => WatsonxScope::Project(id.clone()),
            Self::Space(id) => WatsonxScope::Space(id.clone()),
        }
    }
}

impl WatsonxCredentialProfile {
    fn resolve(
        &self,
        read_secret: &mut impl FnMut(&str) -> Option<String>,
    ) -> Result<WatsonxCredentials, ProviderError> {
        match self {
            Self::ApiKey(name) => resolve_secret(name, read_secret).map(WatsonxCredentials::ApiKey),
            Self::BearerToken(name) => {
                resolve_secret(name, read_secret).map(WatsonxCredentials::BearerToken)
            }
        }
    }
}

fn resolve_optional_secret(
    name: Option<&str>,
    read_secret: &mut impl FnMut(&str) -> Option<String>,
) -> Result<Option<String>, ProviderError> {
    name.map(|name| resolve_secret(name, read_secret))
        .transpose()
}

fn resolve_secret(
    name: &str,
    read_secret: &mut impl FnMut(&str) -> Option<String>,
) -> Result<String, ProviderError> {
    if name.trim().is_empty() || name.trim() != name {
        return Err(ProviderError::InvalidProfile(
            "secret environment variable name must be normalized".to_owned(),
        ));
    }
    read_secret(name).ok_or_else(|| {
        ProviderError::InvalidProfile(format!(
            "required secret environment variable `{name}` is unavailable"
        ))
    })
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
                api_key_env: "ANTHROPIC_API_KEY".to_owned(),
            }),
            profile(ProviderTransportProfile::Openai {
                api_key_env: "OPENAI_API_KEY".to_owned(),
            }),
            profile(ProviderTransportProfile::Ollama { base_url: None }),
            profile(ProviderTransportProfile::Lemonade {
                base_url: None,
                api_key_env: Some("LEMONADE_API_KEY".to_owned()),
            }),
            profile(ProviderTransportProfile::LmStudio {
                base_url: None,
                api_key_env: None,
            }),
            profile(ProviderTransportProfile::Watsonx {
                service_url: "https://us-south.ml.cloud.ibm.com".to_owned(),
                api_version: "2024-05-31".to_owned(),
                scope: WatsonxScopeProfile::Project("project-1".to_owned()),
                credential: WatsonxCredentialProfile::ApiKey("WATSONX_API_KEY".to_owned()),
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
            api_key_env: "OPENAI_API_KEY".to_owned(),
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
}
