# langchart-llm-watsonx

An IBM watsonx.ai implementation of Langchart's `LlmAdapter` contract.

The adapter supports project and deployment-space scopes, caller-supplied
bearer tokens, and IBM Cloud API keys. API keys are exchanged through IBM IAM;
the resulting short-lived bearer token is cached in memory and refreshed before
expiry.

```rust,no_run
use langchart_llm_watsonx::{
    WatsonxAdapter, WatsonxConfig, WatsonxCredentials, WatsonxScope,
};

let adapter = WatsonxAdapter::new(
    WatsonxConfig::new(
        "https://us-south.ml.cloud.ibm.com",
        "2024-05-31",
        WatsonxScope::Project("project-id".to_owned()),
    ),
    WatsonxCredentials::ApiKey("api-key".to_owned()),
)?;
# Ok::<(), langchart_llm_watsonx::BuildError>(())
```

Credentials are held in memory and are intentionally excluded from `Debug`
output. Model enumeration is not currently implemented, so `list_models`
returns Langchart's default empty result.
