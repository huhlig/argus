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

/// Security and confidentiality tiers for data exchanged with providers.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    /// Public repository data with no confidentiality constraints.
    Public,
    /// Internal proprietary data suitable for standard internal reviews.
    Internal,
    /// Confidential data requiring restricted or local provider processing.
    Confidential,
    /// Highly restricted data subject to air-gap or strict local execution.
    Restricted,
}

/// Network boundary and deployment topology of a model provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    /// Executed entirely locally on the host machine (e.g. Ollama, llama.cpp).
    Local,
    /// Hosted within the same private network or VPC.
    SameNetwork,
    /// Hosted externally over the public Internet (e.g. OpenAI, Anthropic).
    Online,
}

/// Capability level for constraining LLM completions to JSON schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputSupport {
    /// No native JSON or schema enforcement.
    None,
    /// Prompt-guided or best-effort structured JSON output.
    BestEffort,
    /// Guaranteed schema adherence enforced by the model engine (e.g. grammar sampling).
    SchemaConstrained,
}

/// Normalized provider and model naming tuple.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderIdentity {
    /// Name of the provider platform or vendor.
    pub provider: String,
    /// Provider implementation version.
    pub provider_version: String,
    /// Name or alias of the model.
    pub model: String,
    /// Pinned version or release identifier of the model.
    pub model_version: String,
}

impl ProviderIdentity {
    /// Validates that all provider and model identity fields are non-empty and trimmed.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidProfile`] if any field is empty or contains leading/trailing whitespace.
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

/// Technical capacities, deployment mode, and supported classifications for a provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Provider and model identity.
    pub identity: ProviderIdentity,
    /// Network deployment boundary.
    pub deployment: DeploymentMode,
    /// Maximum total context window size in tokens.
    pub context_window_tokens: u32,
    /// Maximum allowable completion tokens in a single request.
    pub max_output_tokens: u32,
    /// Degree of structured JSON output support.
    pub structured_output: StructuredOutputSupport,
    /// Whether the model supports native tool calling.
    pub tool_calling: bool,
    /// Maximum concurrent in-flight requests permitted.
    pub concurrency_capacity: u32,
    /// Set of data classifications the provider is authorized to receive.
    pub supported_classifications: BTreeSet<DataClassification>,
    /// Whether the provider returns token usage statistics in responses.
    pub reports_token_usage: bool,
    /// Whether the provider estimates financial cost in responses.
    pub reports_estimated_cost: bool,
}

impl ProviderCapabilities {
    /// Validates that token limits, concurrency capacity, and classifications are consistent.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidProfile`] if limits are zero, output exceeds context, or classifications are empty.
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

/// Policy rule for substituting alternative models during execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSubstitution {
    /// Model identity is pinned; substitution is forbidden.
    Pinned,
    /// Model substitution is permitted across partitioned reviewer roles.
    Partitioned,
}

/// Resource budgets and concurrency boundaries enforced during an automated review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewLimits {
    /// Maximum number of model inference requests.
    pub max_requests: u32,
    /// Upper bound on cumulative input tokens consumed.
    pub max_input_tokens: u64,
    /// Upper bound on cumulative output tokens generated.
    pub max_output_tokens: u64,
    /// Upper bound on total evidence bytes transferred.
    pub max_evidence_bytes: u64,
    /// Maximum allowed evidence expansion cycles.
    pub max_evidence_expansions: u32,
    /// Concurrency limit for parallel requests.
    pub max_concurrency: u32,
    /// Optional maximum spending limit in micro-USD ($0.000001).
    pub max_estimated_cost_microusd: Option<u64>,
}

impl ReviewLimits {
    /// Validates review limits against a provider's stated capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidPolicy`] if bounds are zero or concurrency exceeds capacity.
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

/// Governing policy defining classification, network permission, and budget constraints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderPolicy {
    /// Classification tier of the target repository.
    pub repository_classification: DataClassification,
    /// Whether outbound Internet transmission is authorized for online models.
    pub authorize_online_transmission: bool,
    /// Model substitution permissions.
    pub substitution: ModelSubstitution,
    /// Execution budget limits.
    pub limits: ReviewLimits,
}

impl ProviderPolicy {
    /// Authorizes a candidate provider against this policy, ensuring classification and network constraints are satisfied.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::ClassificationDenied`] if the provider cannot process the repository classification,
    /// or [`ProviderError::OnlineTransmissionDenied`] if online transmission is not allowed.
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

/// Operational readiness status of a model provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    /// Provider is fully operational and responsive.
    Ready,
    /// Provider is experiencing degraded performance or high latency.
    Degraded,
    /// Provider endpoint is currently unreachable.
    Unavailable,
}

/// Structured input request submitted to a [`ModelProvider`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Correlation identifier for the request.
    pub request_id: String,
    /// Attempt count for retry tracking.
    pub attempt: u32,
    /// Rendered prompt text.
    pub prompt: String,
    /// Expected JSON Schema describing valid structured output.
    pub structured_output_schema: serde_json::Value,
    /// Byte length of evidence included in the prompt.
    pub evidence_bytes: u64,
    /// Estimated input token count.
    pub estimated_input_tokens: u64,
    /// Upper bound on completion tokens to generate.
    pub max_output_tokens: u32,
}

/// Structured completion returned by a [`ModelProvider`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    /// Matching correlation identifier from the request.
    pub request_id: String,
    /// Attempt number that produced this response.
    pub attempt: u32,
    /// Parsed JSON output matching the requested schema.
    pub output: serde_json::Value,
    /// Actual input tokens consumed, if reported.
    pub input_tokens: Option<u64>,
    /// Actual output tokens generated, if reported.
    pub output_tokens: Option<u64>,
    /// Estimated financial cost in micro-USD, if reported.
    pub estimated_cost_microusd: Option<u64>,
    /// Identity of the provider and model that generated the completion.
    pub provider: ProviderIdentity,
}

/// Asynchronous interface for governed LLM completion providers.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Returns the advertised capabilities of this provider.
    fn capabilities(&self) -> &ProviderCapabilities;
    /// Probes provider operational health.
    async fn health(&self) -> Result<ProviderHealth, ProviderError>;
    /// Executes a structured inference request.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError`] if execution fails, budget is exceeded, or output cannot be validated.
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ProviderError>;
}

/// Errors occurring during model provider validation, authorization, and inference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    /// Provider configuration profile is invalid.
    InvalidProfile(String),
    /// Provider policy definition is invalid or unfulfillable.
    InvalidPolicy(String),
    /// Provider does not support the data classification required by the repository.
    ClassificationDenied,
    /// Online provider communication was attempted without authorization.
    OnlineTransmissionDenied,
    /// Model substitution was attempted but forbidden by policy.
    SubstitutionDenied(String),
    /// Token, byte, or financial budget was exceeded.
    BudgetExceeded(String),
    /// Provider endpoint was unreachable or returned an unrecoverable failure.
    Unavailable(String),
    /// Model output failed JSON schema validation or syntax parsing.
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
    WatsonxCredentialProfile, WatsonxScopeProfile, substitute_optional_value, substitute_value,
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
