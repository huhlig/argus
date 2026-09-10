# `argus-provider`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-provider`** manages model provider profile resolution, transport connectors, rate limiting, evidence byte enforcement, repair attempt policies, and LLM execution for Argus.

---

## Core Responsibilities

- **Profile Resolution & Security**: Discovers and loads provider runtime profiles (`.json`) strictly from machine/user catalogs (`%APPDATA%\argus\profiles\`, `~/.config/argus/profiles\`, `$ARGUS_CONFIG_DIR/profiles\`) or explicit file paths, ensuring sensitive provider configurations and credentials stay out of version control.
- **Environment Substitution**: Intercalates `${VAR_NAME}` and `${VAR_NAME:-default}` before deserializing profile JSON.
- **Provider Transports**: Connects to LLM backends:
  - **Lemonade**: Local/Network OpenAI-compatible HTTP server (`http://<host>:<port>/v1`) with configurable `request_timeout_seconds`.
  - **Ollama**: Local Ollama server (`http://localhost:11434`).
  - **OpenAI**: OpenAI Chat Completions API (`OPENAI_API_KEY`).
  - **Anthropic**: Anthropic Messages API (`ANTHROPIC_API_KEY`).
  - **Watsonx**: IBM watsonx.ai foundation models API.
  - **LM Studio**: Local LM Studio OpenAI-compatible endpoint.
- **Executor Rate & Limits**: Enforces request caps, max input/output tokens, evidence size ceilings, and concurrency limits.
- **Telemetry Tracking**: Records call latency, token consumption, and failure diagnostics to `DurableProviderTelemetryPublisher`.

---

## Provider Profile Schema (`.json`)

Example profile in user catalog (`%APPDATA%\argus\profiles\lemonade-gemma.json`):

```json
{
  "schema_version": 1,
  "capabilities": {
    "identity": {
      "provider": "lemonade",
      "provider_version": "lemonade@${LEMONADE_HOST:-10.0.0.51}",
      "model": "Gemma-4-31B-it-GGUF",
      "model_version": "Gemma-4-31B-it-GGUF"
    },
    "deployment": "same_network",
    "context_window_tokens": 256000,
    "max_output_tokens": 8192,
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
      "max_requests": 20,
      "max_input_tokens": 1000000,
      "max_output_tokens": 163840,
      "max_evidence_bytes": 10000000,
      "max_evidence_expansions": 0,
      "max_concurrency": 1,
      "max_estimated_cost_microusd": null
    }
  },
  "repair": {
    "max_repair_attempts": 1
  },
  "transport": {
    "kind": "lemonade",
    "base_url": "http://${LEMONADE_HOST:-10.0.0.51}:13305/v1",
    "api_key_env": null,
    "request_timeout_seconds": 1800
  }
}
```

---

## How to Use

Add `argus-provider` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-provider = { path = "../argus-provider" }
```

### Profile Resolution

Named profiles are resolved via the CLI or orchestration layer by querying user catalog locations (`%APPDATA%\argus\profiles\`, `~/.config/argus/profiles\`) and applying environment variable substitution before runtime execution.
