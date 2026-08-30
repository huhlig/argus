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

use argus_provider::{
    BedrockConfig, BedrockCredentials, DataClassification, DeploymentMode,
    DiscoveredProviderKind, LangchartModelProvider, ModelProvider, ModelSubstitution,
    PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION, ProviderCapabilities, ProviderIdentity,
    ProviderPolicy, ProviderRuntimeProfile, ProviderTransportProfile, RepairPolicy, ReviewLimits,
    StructuredOutputSupport, discover_models, generate_runtime_profile,
};
use std::collections::BTreeSet;

fn bedrock_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        identity: ProviderIdentity {
            provider: "bedrock".to_owned(),
            provider_version: "bedrock@us-east-1".to_owned(),
            model: "anthropic.claude-3-7-sonnet-20250219-v1:0".to_owned(),
            model_version: "anthropic.claude-3-7-sonnet-20250219-v1:0@pinned".to_owned(),
        },
        deployment: DeploymentMode::Online,
        context_window_tokens: 200_000,
        max_output_tokens: 8_192,
        structured_output: StructuredOutputSupport::SchemaConstrained,
        tool_calling: false,
        concurrency_capacity: 8,
        supported_classifications: BTreeSet::from([DataClassification::Internal]),
        reports_token_usage: true,
        reports_estimated_cost: false,
    }
}

fn bedrock_policy() -> ProviderPolicy {
    ProviderPolicy {
        repository_classification: DataClassification::Internal,
        authorize_online_transmission: true,
        substitution: ModelSubstitution::Pinned,
        limits: ReviewLimits {
            max_requests: 100,
            max_input_tokens: 1_000_000,
            max_output_tokens: 100_000,
            max_evidence_bytes: 10_000_000,
            max_evidence_expansions: 0,
            max_concurrency: 4,
            max_estimated_cost_microusd: None,
        },
    }
}

#[tokio::test]
async fn bedrock_provider_creation_and_policy_authorization() {
    let caps = bedrock_capabilities();
    let pol = bedrock_policy();
    assert!(pol.authorize(&caps).is_ok());

    let provider = LangchartModelProvider::bedrock(
        caps.clone(),
        BedrockConfig::new("us-east-1"),
        BedrockCredentials::Static {
            access_key_id: "AKIA_MOCK".to_owned(),
            secret_access_key: "MOCK_SECRET".to_owned(),
            session_token: None,
        },
    )
    .unwrap();

    assert_eq!(provider.capabilities().identity, caps.identity);

    let models = provider.adapter().list_models().await.unwrap();
    assert!(
        models
            .iter()
            .any(|m| m.id == "anthropic.claude-3-7-sonnet-20250219-v1:0")
    );
}

#[test]
fn bedrock_runtime_profile_roundtrip_and_build() {
    let profile = ProviderRuntimeProfile {
        schema_version: PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
        capabilities: bedrock_capabilities(),
        policy: bedrock_policy(),
        repair: RepairPolicy {
            max_repair_attempts: 1,
        },
        transport: ProviderTransportProfile::Bedrock {
            region: "us-east-1".to_owned(),
            access_key_id_env: Some("MY_AWS_KEY".to_owned()),
            secret_access_key_env: Some("MY_AWS_SECRET".to_owned()),
            session_token_env: None,
            bearer_token_env: None,
            endpoint_url: None,
            profile_name: None,
        },
    };

    let serialized = serde_json::to_string_pretty(&profile).unwrap();
    assert!(serialized.contains(r#""kind": "bedrock""#));
    assert!(serialized.contains("MY_AWS_KEY"));

    let built = profile
        .build_with_secrets(|name| match name {
            "MY_AWS_KEY" => Some("test-key-id".to_owned()),
            "MY_AWS_SECRET" => Some("test-secret-key".to_owned()),
            _ => None,
        })
        .unwrap();

    assert_eq!(
        built.provider.capabilities().identity.model,
        "anthropic.claude-3-7-sonnet-20250219-v1:0"
    );

    // Test Bearer Token resolution
    let bearer_profile = ProviderRuntimeProfile {
        schema_version: PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
        capabilities: bedrock_capabilities(),
        policy: bedrock_policy(),
        repair: RepairPolicy {
            max_repair_attempts: 1,
        },
        transport: ProviderTransportProfile::Bedrock {
            region: "us-east-1".to_owned(),
            access_key_id_env: None,
            secret_access_key_env: None,
            session_token_env: None,
            bearer_token_env: Some("AWS_BEARER_TOKEN_BEDROCK".to_owned()),
            endpoint_url: None,
            profile_name: None,
        },
    };

    let bearer_built = bearer_profile
        .build_with_secrets(|name| match name {
            "AWS_BEARER_TOKEN_BEDROCK" => Some("ABSK_FIXTURE_TOKEN".to_owned()),
            _ => None,
        })
        .unwrap();

    assert_eq!(
        bearer_built.provider.capabilities().identity.model,
        "anthropic.claude-3-7-sonnet-20250219-v1:0"
    );
}

#[tokio::test]
async fn bedrock_discovery_returns_foundation_models() {
    let models = discover_models(DiscoveredProviderKind::Bedrock, None, None)
        .await
        .unwrap();
    assert!(
        models
            .iter()
            .any(|m| m == "anthropic.claude-3-7-sonnet-20250219-v1:0")
    );
    assert!(models.iter().any(|m| m == "amazon.nova-pro-v1:0"));

    let profile = generate_runtime_profile(
        DiscoveredProviderKind::Bedrock,
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        "anthropic.claude-3-7-sonnet-20250219-v1:0",
        None,
        None,
    )
    .unwrap();

    assert_eq!(profile.capabilities.identity.provider, "bedrock");
}

#[test]
fn bedrock_provider_config_resolves_and_builds_runtime_profile() {
    let models = vec![
        "anthropic.claude-3-7-sonnet-20250219-v1:0".to_owned(),
        "anthropic.claude-3-haiku-20240307-v1:0".to_owned(),
    ];
    let config = argus_provider::generate_provider_config(
        DiscoveredProviderKind::Bedrock,
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        &models,
        None,
        None,
    )
    .unwrap();

    let resolved_sonnet = config.resolve_runtime_profile(Some("claude-3-7-sonnet")).unwrap();
    assert_eq!(resolved_sonnet.capabilities.identity.model, "anthropic.claude-3-7-sonnet-20250219-v1:0");

    let resolved_haiku = config.resolve_runtime_profile(Some("claude-3-haiku")).unwrap();
    assert_eq!(resolved_haiku.capabilities.identity.model, "anthropic.claude-3-haiku-20240307-v1:0");

    let resolved_default = config.resolve_runtime_profile(None).unwrap();
    assert_eq!(resolved_default.capabilities.identity.model, "anthropic.claude-3-7-sonnet-20250219-v1:0");
}
