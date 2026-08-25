# Provider runtime profiles

Argus executes admitted documentation work with:

```text
argus work documentation --profile .argus/config/provider.json --limit 10
```

The limit defaults to one work item. Profile files contain provider identity,
privacy and budget policy, and environment-variable names. They never contain
credential values.

## Ollama example

```json
{
  "schema_version": 1,
  "capabilities": {
    "identity": {
      "provider": "ollama",
      "provider_version": "ollama@0.11",
      "model": "qwen3:30b",
      "model_version": "qwen3:30b"
    },
    "deployment": "local",
    "context_window_tokens": 32768,
    "max_output_tokens": 4096,
    "structured_output": "best_effort",
    "tool_calling": false,
    "concurrency_capacity": 1,
    "supported_classifications": ["internal"],
    "reports_token_usage": true,
    "reports_estimated_cost": false
  },
  "policy": {
    "repository_classification": "internal",
    "authorize_online_transmission": false,
    "substitution": "pinned",
    "limits": {
      "max_requests": 100,
      "max_input_tokens": 1000000,
      "max_output_tokens": 409600,
      "max_evidence_bytes": 100000000,
      "max_evidence_expansions": 0,
      "max_concurrency": 1,
      "max_estimated_cost_microusd": null
    }
  },
  "repair": {
    "max_repair_attempts": 1
  },
  "transport": {
    "kind": "ollama",
    "base_url": null
  }
}
```

`model_version` is the exact model identity expected in responses. A changed
identity fails closed instead of silently substituting a model.

## Transport configurations

Use one of these objects as `transport`. Online profiles must also set
`capabilities.deployment` to `online` and explicitly set
`policy.authorize_online_transmission` to `true`.

```json
{"kind":"anthropic","api_key_env":"ANTHROPIC_API_KEY"}
{"kind":"openai","api_key_env":"OPENAI_API_KEY"}
{"kind":"ollama","base_url":null}
{"kind":"lemonade","base_url":null,"api_key_env":null}
{"kind":"lm_studio","base_url":null,"api_key_env":null}
{"kind":"watsonx","service_url":"https://us-south.ml.cloud.ibm.com","api_version":"2024-05-31","scope":{"kind":"project","id":"project-id"},"credential":{"kind":"api_key","env":"WATSONX_API_KEY"}}
```

The default local endpoints are:

- Ollama: `http://127.0.0.1:11434/v1`
- Lemonade: `http://127.0.0.1:13305/v1`
- LM Studio: `http://127.0.0.1:1234/v1`

Same-network endpoints are allowed only when the capability deployment is
`same_network`. Local deployment accepts loopback endpoints only. Online
endpoints require HTTPS.
