# `argus-policies`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-policies`** defines audit policy rules, target applicability evaluations, severity classifications, and candidate finding schemas for Argus audit pipelines.

---

## Core Responsibilities

- **Policy Pipelines**: Implements policies for documentation (`documentation-public-api@1`), correctness (`correctness-conservative@1`), and architecture (`architecture-code-derived@1`).
- **Applicability Evaluation**: Evaluates whether a discovered target in the inventory should be reviewed under a specific policy rule.
- **Finding Definitions**: Defines candidate finding structures, rule identifiers, severity levels (`Critical`, `High`, `Medium`, `Low`, `Info`), and recommendation schemas.
- **Evidence Bounding Rules**: Specifies evidence frame expansion parameters per policy type.

---

## Policy Pipelines

| Policy ID | Category | Target Scope | Focus |
| :--- | :--- | :--- | :--- |
| `documentation-public-api@1` | Documentation | Public API items (fns, structs, enums, traits) | Missing or incomplete doc comments, broken example links, missing parameter/error descriptions. |
| `correctness-conservative@1` | Correctness | All callables & types | Resource leaks, improper error handling, logic bugs, lock contention, unsafe assumptions. |
| `architecture-code-derived@1` | Architecture | Modules, packages, and workspaces | Graph-grounded boundary violations, circular dependencies, modularity anti-patterns, and layer leaks. |

---

## How to Use

Add `argus-policies` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-policies = { path = "../argus-policies" }
```

### Example Usage

```rust
use argus_policies::DocumentationApplicabilityPolicy;

fn main() -> Result<(), argus_core::ArgusError> {
    let policy = DocumentationApplicabilityPolicy::public_api()?;
    println!("Policy ID: {}", policy.id());
    Ok(())
}
```
