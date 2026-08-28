# Provider runtime profiles

Argus executes admitted review work against configured model providers:

```bash
# Using a named profile from the user catalog:
argus work documentation --profile lemonade-qwen --limit 10

# Using the repository default profile:
argus work --limit 10

# Using an explicit profile file path:
argus work documentation --profile ./custom-profile.json --limit 5
```

The limit defaults to 1 work item. Profile files define provider capabilities, review policies/budgets, and transport endpoints with environment variable substitution.

---

## Security Model: System vs. Project Configuration

Argus enforces a strict security boundary between shared repository policies and machine-level provider configurations:

- **Project Configuration (`<repo>/.argus/config/argus.json`)**:
  Committed to Git. Defines schema version, policy baselines, and profile **identities** (e.g. `"default_profile": "lemonade-qwen"`).
  **Important**: Project repositories never contain provider transport definitions or endpoint credentials.

- **System / User Catalog (`%APPDATA%\argus\profiles\<name>.json` or `~/.config/argus/profiles/<name>.json`)**:
  Stores named provider runtime profile definitions on the local developer machine. Not committed to project repositories.

---

## Profile Discovery & Resolution

When `--profile <name-or-path>` is specified (or when falling back to `default_profile` from project config):

1. **Explicit File Paths**:
   If the argument starts with `./`, `../`, contains `/` or `\`, is an absolute path, or ends with `.json`, Argus loads the file directly from the filesystem (relative to the workspace root or absolute).

2. **Named Profiles**:
   If the argument is a bare profile name (e.g. `lemonade-qwen`, `ollama`, `default`), Argus strictly searches user/system catalogs in the following order:
   - **Environment Override**: `$ARGUS_CONFIG_DIR/profiles/<name>.json`
   - **Windows**:
     - `%APPDATA%\argus\profiles\<name>.json`
     - `%USERPROFILE%\.config\argus\profiles\<name>.json`
   - **Linux / macOS**:
     - `$XDG_CONFIG_HOME/argus/profiles/<name>.json`
     - `~/.config/argus/profiles/<name>.json`

If the profile cannot be found, Argus fails closed and lists all candidate user catalog locations searched.

---

## Environment Variable Substitution

Profile JSON files support runtime environment variable interpolation before deserialization:

- `${VAR_NAME}` — Expands to the value of `$VAR_NAME`. Fails if the variable is unset.
- `${VAR_NAME:-default_value}` — Expands to `$VAR_NAME` if set, otherwise falls back to `default_value`.
- `$VAR_NAME` — Expands alphanumeric/underscore variable name.
- `\${VAR}` / `\$VAR` — Escaped literal string.

### Example with Environment Interpolation

```json
{
  "schema_version": 1,
  "capabilities": {
    "identity": {
      "provider": "lemonade",
      "provider_version": "lemonade@${LEMONADE_HOST:-127.0.0.1}",
      "model": "Qwen3.6-35B-A3B-GGUF",
      "model_version": "Qwen3.6-35B-A3B-GGUF"
    },
    "deployment": "same_network",
    "context_window_tokens": 131072,
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

## Transport Configurations

Use one of these objects for the `transport` field:

```json
{"kind": "lemonade", "base_url": "http://${LEMONADE_HOST:-127.0.0.1}:13305/v1", "api_key_env": null, "request_timeout_seconds": 1800}
{"kind": "ollama", "base_url": null}
{"kind": "openai", "api_key_env": "OPENAI_API_KEY"}
{"kind": "anthropic", "api_key_env": "ANTHROPIC_API_KEY"}
{"kind": "lm_studio", "base_url": null, "api_key_env": null}
{"kind": "watsonx", "service_url": "https://us-south.ml.cloud.ibm.com", "api_version": "2024-05-31", "scope": {"kind": "project", "id": "project-id"}, "credential": {"kind": "api_key", "env": "WATSONX_API_KEY"}}
```

### Timeouts & Network Policies

- **`request_timeout_seconds`**: Supported by HTTP/OpenAI-compatible transports (such as Lemonade). For local/network inference on large LLMs or quantized models, configure this to a reasonable duration (e.g. `1800` for 30 minutes) to avoid premature request cancellation.
- **`capabilities.deployment`**:
  - `local`: Allows loopback endpoints only (`127.0.0.1`, `localhost`).
  - `same_network`: Allows local network IP addresses.
  - `online`: Requires HTTPS and explicit `policy.authorize_online_transmission: true`.

---

## Automated Profile Discovery

You can automatically query a model provider endpoint, discover available models, and populate your user catalog profiles:

```bash
# Discover local or network Lemonade models:
argus profile discover --type lemonade --endpoint http://10.0.0.51:13305/v1

# Discover local Ollama models:
argus profile discover --type ollama --endpoint http://127.0.0.1:11434

# Discover OpenAI models using an environment variable for authentication:
argus profile discover --type openai --api-key-env OPENAI_API_KEY

# Discover Anthropic models:
argus profile discover --type anthropic --api-key-env ANTHROPIC_API_KEY

# Discover local LM Studio models:
argus profile discover --type lm_studio --endpoint http://127.0.0.1:1234/v1
```

### Listing Installed Profiles

To view all installed provider runtime profiles across your catalog and project:

```bash
argus profile list
```

