# `argus-test-support`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-test-support`** provides test utilities, mock LLM providers, synthetic workspace builders, and in-memory simulation helpers for testing Argus crates without external dependencies.

---

## Core Responsibilities

- **Mock Provider Fixtures**: Pre-scripted LLM provider responses (`Pass`, `CandidateFinding`, `UnableToVerify`, `Failure`) with deterministic latency and token metrics.
- **Synthetic Workspace Builder**: Builds temporary fixture repository structures with clean, dirty, or malformed source code states.
- **Workflow Simulators**: Simulates `langchart` workflow steps deterministically for state machine unit tests.
- **Capturing Sinks**: Records events, queue writes, and telemetry calls for assertion in integration tests.

---

## Important Engineering Rule

> **Note**: Production crates must NEVER depend on `argus-test-support`. It should only be included under `[dev-dependencies]` in test targets.

---

## How to Use

Add `argus-test-support` to `[dev-dependencies]` in your crate's `Cargo.toml`:

```toml
[dev-dependencies]
argus-test-support = { path = "../argus-test-support" }
```

### Example Usage in Integration Test

```rust
#[cfg(test)]
mod tests {
    use argus_test-support::ScriptedProviderBuilder;

    #[test]
    fn test_mock_provider() {
        let provider = ScriptedProviderBuilder::new()
            .always_pass()
            .build();
        assert_eq!(provider.name(), "scripted");
    }
}
```
