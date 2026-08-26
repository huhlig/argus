# ADR 0005: Use Langchart adapters for LLM wire transport

- Status: Accepted
- Date: 2026-08-24

## Decision

Argus does not adopt Rig as a second model framework. Provider wire transports
implement Langchart's `LlmAdapter` contract. Anthropic, OpenAI, Ollama,
Lemonade, and LM Studio use Langchart's `langchart-llm-generic` adapter.
Ollama, Lemonade, and LM Studio use their OpenAI-compatible endpoints.

watsonx implements the same `LlmAdapter` contract in Langchart's
`langchart-llm-watsonx` crate because IBM Cloud requires an IAM API-key exchange
and a project or space scope. It caches short-lived IAM bearer tokens and
requests JSON output from the watsonx text-chat API. Credentials remain in
memory and are not part of provider identity, workflow state, or durable audit
records.

An Argus adapter wraps `LlmAdapter` behind the Argus-owned `ModelProvider`
contract. Argus continues to enforce deployment classification, online
transmission authorization, pinned provider/model identity, health, concurrency,
request and token budgets, structured-output validation, and repair limits.

`PrimaryReviewActor` binds its sealed invocation capability envelope to a
per-invocation adapter and invokes the governed transport through Langchart's
capability broker. The Argus executor accepts that adapter only when its complete
capability profile exactly matches the assigned provider, then independently
enforces identity, concurrency, cumulative budgets, and output validation.

Langchart's current `AgentInvocation` does not expose the workflow state's model
policy. Argus therefore sends the exact model from the durable provider
assignment; model-profile resolution occurs when the run's provider and engine
adapters are assembled. This preserves pinned identity while avoiding an
actor-controlled routing choice.

Provider executors publish cumulative telemetry snapshots under a caller-owned
runtime session ID. Repeated publications replace that session's snapshot;
`argus status` aggregates distinct sessions by exact provider/model identity.
Every runtime boot must therefore use a new stable session ID, while all
executors within that boot reuse it. This prevents repeated status flushes from
double-counting and preserves earlier process totals after restart.

## Consequences

Argus reuses Langchart routing and transports without coupling durable audit
records to provider SDK types. Adding Rig later remains possible behind
`LlmAdapter`, but requires a demonstrated transport capability that Langchart's
adapters cannot provide. Langchart's shared response-format contract carries
text, JSON-object, and native JSON-schema requests across routing boundaries.
Argus maps each configured capability to the corresponding request format and
still validates every response against its evidence-bound policy contract.
