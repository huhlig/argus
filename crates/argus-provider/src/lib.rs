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

//! Governed model-provider contracts for Argus review workflows.

mod assignment;
mod discovery;
mod executor;
mod runtime_profile;
mod transport;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    Public,
    Internal,
    Confidential,
    Restricted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    Local,
    SameNetwork,
    Online,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputSupport {
    None,
    BestEffort,
    SchemaConstrained,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderIdentity {
    pub provider: String,
    pub provider_version: String,
    pub model: String,
    pub model_version: String,
}

impl ProviderIdentity {
    pub fn validate(&self) -> Result<(), ProviderError> {
        for value in [
            &self.provider,
            &self.provider_version,
            &self.model,
            &self.model_version,
        ] {
            if value.trim().is_empty() || value.trim() != *value {
                return Err(ProviderError::InvalidProfile(
                    "provider and model identity fields must be non-empty and normalized"
                        .to_owned(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub identity: ProviderIdentity,
    pub deployment: DeploymentMode,
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
    pub structured_output: StructuredOutputSupport,
    pub tool_calling: bool,
    pub concurrency_capacity: u32,
    pub supported_classifications: BTreeSet<DataClassification>,
    pub reports_token_usage: bool,
    pub reports_estimated_cost: bool,
}

impl ProviderCapabilities {
    pub fn validate(&self) -> Result<(), ProviderError> {
        self.identity.validate()?;
        if self.context_window_tokens == 0
            || self.max_output_tokens == 0
            || self.max_output_tokens > self.context_window_tokens
        {
            return Err(ProviderError::InvalidProfile(
                "token capacities must be non-zero and output must fit the context window"
                    .to_owned(),
            ));
        }
        if self.concurrency_capacity == 0 || self.supported_classifications.is_empty() {
            return Err(ProviderError::InvalidProfile(
                "provider must advertise capacity and at least one data classification".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSubstitution {
    Pinned,
    Partitioned,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewLimits {
    pub max_requests: u32,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_evidence_bytes: u64,
    pub max_evidence_expansions: u32,
    pub max_concurrency: u32,
    pub max_estimated_cost_microusd: Option<u64>,
}

impl ReviewLimits {
    pub fn validate_for(&self, provider: &ProviderCapabilities) -> Result<(), ProviderError> {
        if self.max_requests == 0
            || self.max_input_tokens == 0
            || self.max_output_tokens == 0
            || self.max_evidence_bytes == 0
        {
            return Err(ProviderError::InvalidPolicy(
                "request, token, and evidence budgets must be bounded above zero".to_owned(),
            ));
        }
        if self.max_concurrency == 0 || self.max_concurrency > provider.concurrency_capacity {
            return Err(ProviderError::InvalidPolicy(
                "configured concurrency exceeds provider capacity".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderPolicy {
    pub repository_classification: DataClassification,
    pub authorize_online_transmission: bool,
    pub substitution: ModelSubstitution,
    pub limits: ReviewLimits,
}

impl ProviderPolicy {
    pub fn authorize(&self, provider: &ProviderCapabilities) -> Result<(), ProviderError> {
        provider.validate()?;
        self.limits.validate_for(provider)?;
        if !provider
            .supported_classifications
            .contains(&self.repository_classification)
        {
            return Err(ProviderError::ClassificationDenied);
        }
        if provider.deployment == DeploymentMode::Online && !self.authorize_online_transmission {
            return Err(ProviderError::OnlineTransmissionDenied);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub request_id: String,
    pub attempt: u32,
    pub prompt: String,
    pub structured_output_schema: serde_json::Value,
    pub evidence_bytes: u64,
    pub estimated_input_tokens: u64,
    pub max_output_tokens: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub request_id: String,
    pub attempt: u32,
    pub output: serde_json::Value,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub estimated_cost_microusd: Option<u64>,
    pub provider: ProviderIdentity,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn capabilities(&self) -> &ProviderCapabilities;
    async fn health(&self) -> Result<ProviderHealth, ProviderError>;
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ProviderError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    InvalidProfile(String),
    InvalidPolicy(String),
    ClassificationDenied,
    OnlineTransmissionDenied,
    SubstitutionDenied(String),
    BudgetExceeded(String),
    Unavailable(String),
    InvalidOutput(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProfile(message) => {
                write!(formatter, "invalid provider profile: {message}")
            }
            Self::InvalidPolicy(message) => write!(formatter, "invalid provider policy: {message}"),
            Self::ClassificationDenied => {
                formatter.write_str("provider cannot receive this data classification")
            }
            Self::OnlineTransmissionDenied => {
                formatter.write_str("online transmission is not authorized")
            }
            Self::SubstitutionDenied(message) => {
                write!(formatter, "model substitution denied: {message}")
            }
            Self::BudgetExceeded(message) => {
                write!(formatter, "provider budget exceeded: {message}")
            }
            Self::Unavailable(message) => write!(formatter, "provider unavailable: {message}"),
            Self::InvalidOutput(message) => write!(formatter, "invalid provider output: {message}"),
        }
    }
}

impl Error for ProviderError {}

pub use assignment::{ModelAssignment, ModelAssignmentBook};
pub use discovery::{
    DiscoveredProviderKind, discover_models, generate_provider_config, generate_runtime_profile,
    infer_deployment_mode, slugify_model_alias, slugify_profile_name,
};
pub use executor::{
    OutputValidator, ProviderExecutor, ProviderTelemetry, ProviderTelemetrySink, RepairPolicy,
};
pub use langchart_llm_bedrock::{BedrockAdapter, BedrockConfig, BedrockCredentials};
pub use langchart_llm_watsonx::{WatsonxConfig, WatsonxCredentials, WatsonxScope};
pub use runtime_profile::{
    BuiltProviderRuntime, PROVIDER_CONFIG_SCHEMA_VERSION, PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
    ProviderConfig, ProviderModelConfig, ProviderRuntimeProfile, ProviderTransportProfile,
    WatsonxCredentialProfile, WatsonxScopeProfile,
};
pub use transport::{LangchartModelProvider, StructuredOutputStrategy};

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(deployment: DeploymentMode) -> ProviderCapabilities {
        ProviderCapabilities {
            identity: ProviderIdentity {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                model: "reviewer".to_owned(),
                model_version: "pinned".to_owned(),
            },
            deployment,
            context_window_tokens: 16_384,
            max_output_tokens: 2_048,
            structured_output: StructuredOutputSupport::SchemaConstrained,
            tool_calling: false,
            concurrency_capacity: 2,
            supported_classifications: [DataClassification::Internal].into_iter().collect(),
            reports_token_usage: true,
            reports_estimated_cost: false,
        }
    }

    fn policy() -> ProviderPolicy {
        ProviderPolicy {
            repository_classification: DataClassification::Internal,
            authorize_online_transmission: false,
            substitution: ModelSubstitution::Pinned,
            limits: ReviewLimits {
                max_requests: 3,
                max_input_tokens: 30_000,
                max_output_tokens: 6_000,
                max_evidence_bytes: 1_000_000,
                max_evidence_expansions: 2,
                max_concurrency: 1,
                max_estimated_cost_microusd: Some(100_000),
            },
        }
    }

    #[test]
    fn local_provider_is_authorized_within_limits() {
        assert!(
            policy()
                .authorize(&capabilities(DeploymentMode::Local))
                .is_ok()
        );
    }

    #[test]
    fn online_provider_requires_explicit_authorization() {
        assert_eq!(
            policy().authorize(&capabilities(DeploymentMode::Online)),
            Err(ProviderError::OnlineTransmissionDenied)
        );
    }

    #[test]
    fn classification_and_concurrency_are_enforced() {
        let mut policy = policy();
        policy.repository_classification = DataClassification::Confidential;
        assert_eq!(
            policy.authorize(&capabilities(DeploymentMode::Local)),
            Err(ProviderError::ClassificationDenied)
        );

        policy.repository_classification = DataClassification::Internal;
        policy.limits.max_concurrency = 3;
        assert!(matches!(
            policy.authorize(&capabilities(DeploymentMode::Local)),
            Err(ProviderError::InvalidPolicy(_))
        ));
    }
}
