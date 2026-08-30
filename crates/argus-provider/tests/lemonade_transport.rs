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
    DiscoveredProviderKind, ProviderTransportProfile, StructuredOutputSupport,
    generate_provider_config,
};

#[test]
fn lemonade_provider_config_resolves_and_builds_runtime_profile() {
    let models = vec![
        "Qwen/Qwen2.5-Coder-32B-Instruct".to_owned(),
        "meta-llama/Llama-3.3-70B-Instruct".to_owned(),
    ];
    let config = generate_provider_config(
        DiscoveredProviderKind::Lemonade,
        "http://10.0.0.51:13305/v1",
        &models,
        None,
        Some(1800),
    )
    .unwrap();

    assert_eq!(config.provider, "lemonade");
    assert!(matches!(
        config.transport,
        ProviderTransportProfile::Lemonade {
            request_timeout_seconds: Some(1800),
            ..
        }
    ));

    let resolved_default = config.resolve_runtime_profile(None).unwrap();
    assert_eq!(
        resolved_default.capabilities.identity.model,
        "Qwen/Qwen2.5-Coder-32B-Instruct"
    );
    assert_eq!(
        resolved_default.capabilities.structured_output,
        StructuredOutputSupport::BestEffort
    );

    let resolved_llama = config
        .resolve_runtime_profile(Some("meta-llama/llama-3.3-70b-instruct"))
        .unwrap();
    assert_eq!(
        resolved_llama.capabilities.identity.model,
        "meta-llama/Llama-3.3-70B-Instruct"
    );
}
