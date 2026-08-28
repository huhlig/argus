# `argus-provider`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-provider`** manages model provider profile resolution, transport connectors, rate limiting, evidence byte enforcement, repair attempt policies, and LLM execution for Argus.

---

## Core Responsibilities

- **Profile Resolution**: Discovers and loads provider runtime profiles (`.json`) from project catalogs (`.argus/config/profiles/`), system paths (`~/.config/argus/profiles/`), environment variables, or explicit paths.
- **Provider Transports**: Connects to LLM backends:
  - **Lemonade**: Local/Network OpenAI-compatible HTTP server (`http://<host>:<port>/v1`).
  - **Ollama**: Local Ollama server (`http://localhost:11434`).
  - **OpenAI**: OpenAI Chat Completions API (`OPENAI_API_KEY`).
  - **Anthropic**: Anthropic Messages API (`ANTHROPIC_API_KEY`).
  - **Watsonx**: IBM watsonx.ai foundation models API.
  - **LM Studio**: Local LM Studio OpenAI-compatible endpoint.
- **Executor Rate & Limits**: Enforces request caps, max input/output tokens, evidence size ceilings, and concurrency limits.
- **Telemetry Tracking**: Records call latency, token consumption, and failure diagnostics to `DurableProviderTelemetryPublisher`.

---

## Provider Profile Schema (`.json`)

Example profile: `.argus/config/profiles/lemonade-gemma.json`

```json
{
  "schema_version": 1,
  "capabilities": {
    "identity": {
      "provider": "lemonade",
      "provider_version": "lemonade@10.0.0.51:13305",
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
    "base_url": "http://10.0.0.51:13305/v1",
    "api_key_env": null,
    "request_timeout_seconds": 300
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

### Profile Resolution Example

```rust
use std::path::Path;
use argus_provider::resolve_provider_profile;

fn main() -> Result<(), argus_core::ArgusError> {
    let repo_root = Path::new(".");
    let (profile_path, profile) = resolve_provider_profile(repo_root, "lemonade-gemma")?;
    println!("Loaded profile from {:?}", profile_path);
    println!("Provider: {}", profile.capabilities.identity.provider);
    println!("Model: {}", profile.capabilities.identity.model);
    Ok(())
}
```
