# Intelligent Source Code Review Harness — Implementation Plan

## 1. Purpose

This document defines an implementation plan for a generic, iterative, and intelligent source-code review harness capable of reviewing an entire software application at multiple levels of abstraction.

The harness is intended to systematically assess:

- Documentation quality and freshness
- Code correctness
- Maintainability
- Performance
- API quality
- Architectural quality and consistency
- Test quality and coverage
- Error-handling quality
- Safety and security-sensitive behavior
- Consistency with ADRs, PRDs, design documents, and other engineering specifications

The system is not primarily a code-generation or automated-fixing tool. Its initial purpose is **analysis, evidence gathering, review, and reporting**.

The key design principle is:

> Every relevant source artifact must be deterministically discovered, individually considered, and assigned an explicit review outcome.

This prevents the common failure mode of repository-level AI reviews where an agent examines only a small or interesting subset of the codebase while appearing to provide a comprehensive result.

---

# 2. Goals

The harness SHALL:

1. Discover and explicitly account for the semantic structure exposed by each configured language adapter relative to an audit snapshot, analysis-configuration matrix, and recorded adapter capability boundary.
2. Represent crates, packages, modules, files, types, functions, declarations, implementations, tests, and related artifacts as reviewable targets.
3. Account for each target-policy pair independently using an appropriate target-specific rubric, while permitting relational, aggregate, or safely batched model invocations.
4. Build bounded evidence packages rather than loading an entire repository into a model context.
5. Allow reviewers to progressively request additional evidence when required.
6. Separate deterministic static analysis from model-assisted semantic analysis.
7. Review documentation for presence, completeness, accuracy, usefulness, and currency.
8. Review source code for correctness, maintainability, performance, and code quality.
9. Review architecture at module, package/crate, subsystem, and workspace/application levels.
10. Compare code, tests, documentation, and design documents for consistency.
11. Preserve evidence and provenance for each conclusion.
12. Produce both human-readable and machine-readable reports.
13. Track exact review coverage.
14. Support incremental re-review based on source and dependency changes.
15. Permit additional review policies to be added without redesigning the core harness.
16. Support local, same-network, and online model providers through a capability-aware provider abstraction.
17. Allow candidate findings to be independently verified, escalated to stronger models, and queued for human adjudication.
18. Support resumable audits of very large repositories without requiring the complete repository graph or review state to remain in memory.
19. Permit build, test, benchmark, analysis, rendering, and external publishing integrations through explicit extension contracts.
20. Provide equivalent local CLI and MCP workflows for configuring, running, inspecting, verifying, reporting, and publishing audits.

Argus is implemented in Rust, but the core review model SHALL support repositories containing any language for which a capable adapter is installed.

The first review-language adapter targets Rust. Argus itself provides the initial dogfooding repository, and a larger multi-crate Rust repository such as Mnemosyne provides scale, recovery, and cross-crate validation. Additional language adapters should follow shortly after the Rust adapter; the second adapter is an explicit test that the core domain model and evidence contracts are not accidentally Rust-specific.

---

# 3. Non-Goals

The initial system SHALL NOT:

- Automatically modify production source code.
- Automatically rewrite documentation.
- Treat the implementation as unquestionably authoritative when documentation, tests, and design disagree.
- Attempt to prove formal correctness.
- Replace normal compilation, linting, testing, profiling, benchmarking, or security tooling.
- Assume that every declaration requires extensive prose documentation.
- Generate findings merely to maximize issue count.
- Require model hosting or inference distribution to be managed by the harness itself.
- Treat model-generated findings as autonomous final decisions without operator-configured verification or human review.
- Silently transmit repository content to an online model provider.

Future versions MAY provide optional remediation workflows, but those workflows should be separate from the review and evidence-gathering system.

---

# 4. Core Philosophy

The harness should behave more like an engineering audit system than a conversational code reviewer.

The system should distinguish four major operations:

```text
Discovery
    ↓
Evidence Collection
    ↓
Evaluation
    ↓
Aggregation
```

Discovery determines what exists.

Evidence collection determines what is relevant to a particular review target.

Evaluation assesses the target against one or more review policies.

Aggregation determines whether individually reasonable artifacts collectively form a coherent and correct system.

The harness MUST avoid using an LLM as the repository crawler. Repository structure and coverage should be determined through deterministic tooling.

---

# 5. Review Hierarchy

The source repository should be modeled as a hierarchy of review targets.

For a Rust implementation, a representative hierarchy is:

```text
Workspace
├── Crate
│   ├── Module
│   │   ├── Struct
│   │   │   ├── Field
│   │   │   └── Impl
│   │   │       ├── Method
│   │   │       └── Associated Function
│   │   ├── Enum
│   │   │   └── Variant
│   │   ├── Trait
│   │   │   └── Trait Method
│   │   ├── Function
│   │   ├── Type Alias
│   │   ├── Constant
│   │   ├── Static
│   │   ├── Macro
│   │   └── Re-export
│   └── Integration Tests
└── Cross-Crate Architecture
```

Files remain important as source containers, but a file SHOULD NOT be treated as the primary semantic review unit.

A single source file can contain multiple independent concepts, while one semantic concept can also span several files.

Individual target review and aggregate review are complementary. Per-symbol review provides bounded context and exact accounting; relationship, module, package, and workspace reviews recover behavior that cannot be understood from isolated declarations.

---

# 6. High-Level Architecture

```text
                           ┌──────────────────────────┐
                           │      Repository          │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │ Workspace / Project      │
                           │ Discovery                │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │ Semantic Inventory       │
                           │ + Symbol Graph           │
                           └────────────┬─────────────┘
                                        │
                        ┌───────────────┼─────────────────┐
                        │               │                 │
                        ▼               ▼                 ▼
                 Static Analysis   Design Index       Test Index
                        │               │                 │
                        └───────────────┼─────────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │ Review Scheduler         │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │ Context / Evidence       │
                           │ Builder                  │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │ Langchart Workflow      │
                           │ Orchestrator             │
                           └────────────┬─────────────┘
                                        │
                                        ▼
             ┌────────────────────────────────────────────────────┐
             │                 Review Policies                    │
             │                                                    │
             │ Documentation  Correctness   Maintainability       │
             │ Performance    Architecture  Testing               │
             │ Safety         API Quality    Design Consistency    │
             └───────────────────────┬────────────────────────────┘
                                     │
                                     ▼
                           ┌──────────────────────────┐
                           │ Findings + Review        │
                           │ Records                  │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │ Aggregate Review         │
                           └────────────┬─────────────┘
                                        │
                                        ▼
                           ┌──────────────────────────┐
                           │ Reports / CI Results     │
                           └──────────────────────────┘
```

---

# 7. Core Domain Model

## 7.1 Review Target

Every semantically meaningful artifact should have a stable representation.

```rust
pub struct ReviewTarget {
    pub id: TargetId,
    pub language: Language,
    pub classification: TargetClassification,

    pub package: Option<String>,
    pub module_path: Option<String>,
    pub qualified_name: String,

    pub source: SourceLocation,
    pub visibility: Visibility,

    pub parent: Option<TargetId>,
    pub children: Vec<TargetId>,

    pub declaration: Option<String>,
    pub documentation: Option<String>,

    pub attributes: TargetAttributes,
}
```

The core classification should remain portable while preserving language-specific precision:

```rust
pub struct TargetClassification {
    pub core_kind: CoreTargetKind,
    pub language_kind: LanguageTargetKind,
}

pub enum CoreTargetKind {
    Repository,
    Package,
    Module,
    File,

    Type,
    Field,
    Variant,
    Callable,
    Constant,
    Macro,
    Test,
    Benchmark,
    ArchitectureBoundary,
    Subsystem,
    GeneratedArtifact,
    LanguageSpecific,
}
```

`LanguageTargetKind` is a namespaced adapter value. The Rust adapter may specialize core kinds as `crate`, `struct`, `enum`, `trait`, `trait_method`, `type_alias`, `function`, `method`, `associated_function`, `static`, `impl`, or `reexport`. Policies apply to portable core kinds where possible and may use language-specific rubrics where necessary.

---

## 7.2 Source Location

```rust
pub struct SourceLocation {
    pub path: PathBuf,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}
```

Precise source locations are important because findings must be actionable.

## 7.3 Audit Snapshot

Every audit MUST be bound to an immutable, content-addressed view of the repository and its analysis configuration.

```rust
pub struct AuditSnapshot {
    pub id: AuditSnapshotId,
    pub repository_root: PathBuf,
    pub vcs_revision: Option<String>,
    pub dirty_tree_manifest: Vec<ContentHash>,
    pub submodule_revisions: Vec<RevisionRef>,

    pub lockfile_hashes: Vec<ContentHash>,
    pub generated_input_hashes: Vec<ContentHash>,
    pub design_document_hashes: Vec<ContentHash>,

    pub language_configurations: Vec<LanguageConfiguration>,
    pub toolchain: ToolchainIdentity,
    pub environment_inputs: Vec<RecordedEnvironmentInput>,

    pub created_at: Timestamp,
}
```

For Rust, a language configuration includes at minimum the workspace members, Cargo feature set, target triple, build profile, relevant `cfg` values, compiler version, and macro-expansion/tool versions.

An audit snapshot MAY refer to a clean immutable VCS revision. For a dirty working tree, Argus MUST record content hashes and preserve or copy the analyzed content so a long-running audit cannot silently combine different file versions.

Changes discovered after snapshot creation MUST either:

1. create a new audit snapshot,
2. explicitly invalidate and restart affected work against a new snapshot,
3. or be ignored until a later audit.

Inventory, evidence, work items, assessments, findings, reports, and publication receipts MUST reference the audit snapshot identity.

---

## 7.4 Relationships

The semantic inventory should create explicit relationships among targets.

Examples include:

```text
contains
implements
implemented_by
calls
called_by
uses_type
returns_type
accepts_type
reads
writes
depends_on
reexports
tests
benchmarks
inherits
associated_with
```

A graph representation is especially useful because review context frequently depends on relationships rather than source proximity.

---

# 8. Source Language Adapters

The core review engine SHOULD NOT depend directly on Rust-specific concepts.

## 8.1 Source Reading, Syntax, and Semantics

Argus owns immutable source acquisition. It reads files from the audit snapshot as bytes, records their content hashes and encoding status, and provides bounded source slices by stable location. Language adapters do not independently reopen a mutable working tree during an audit.

Turning source bytes into targets is adapter-specific and layered:

```text
Repository and build discovery
    ↓
Lossless source and comment parsing
    ↓
Language-native semantic resolution
    ↓
Build/configuration expansion
    ↓
Normalized targets, relationships, and capability records
```

An adapter may combine several providers:

```text
Project provider          packages, modules, build targets, configurations
Syntax provider           declarations, comments, spans, incomplete source
Semantic provider         resolved symbols, types, references, implementations
Build provider            features, generated inputs, conditional compilation
Tool provider             diagnostics, tests, lint, documentation checks
Relationship provider     calls, type use, inheritance, implementation, tests
```

Argus does not require one universal parser. Each language should use its native compiler, language server, parser library, or stable analysis API where that produces the best semantic fidelity. Tree-sitter MAY be used for fast syntax discovery, lossless source navigation, partial-source recovery, or bootstrapping a new adapter. It MUST NOT be treated as sufficient evidence for resolved types, calls, implementations, macro expansion, or configuration-specific semantics unless a language-specific provider can establish those facts.

Every adapter MUST declare capability and resolution status rather than implying that all languages expose Rust-equivalent semantics:

```rust
pub struct TargetCapability {
    pub kind: AnalysisCapability,
    pub status: ResolutionStatus,
    pub provider: ProviderIdentity,
    pub limitations: Vec<AnalysisLimitation>,
}

pub enum ResolutionStatus {
    Complete,
    Partial,
    Unavailable,
    Failed,
}
```

Representative capabilities include syntax, documentation association, symbol identity, type resolution, references, calls, implementations, generated-source mapping, and configuration resolution. Coverage reports MUST expose material partial, unavailable, and failed capabilities.

Instead:

```rust
pub trait LanguageAdapter {
    fn discover_project(&self, root: &Path) -> Result<ProjectInventory>;

    fn extract_source(
        &self,
        target: TargetId,
    ) -> Result<SourceArtifact>;

    fn relationships(
        &self,
        target: TargetId,
    ) -> Result<Vec<TargetRelation>>;

    fn deterministic_checks(
        &self,
        target: TargetId,
    ) -> Result<Vec<StaticFinding>>;
}
```

## 8.2 Rust Adapter

The first adapter should use some combination of:

- `cargo metadata`
- rustdoc JSON
- compiler diagnostics
- Clippy
- `cargo test`
- `cargo test --doc`
- lossless Rust source and comment parsing
- rust-analyzer information where useful
- optional MIR/HIR-based tooling for advanced analysis

The preferred approach is compiler-aware semantic information wherever possible. A Rust parser such as the rust-analyzer syntax crates or `syn` may supply source structure and spans; tree-sitter-rust is optional, not foundational. No single source is assumed to expose every target or relationship, so the adapter reconciles provider output under stable target identities and records conflicts or gaps.

---

# 9. Repository Inventory

The inventory stage is responsible for completeness relative to the declared audit snapshot, analysis-configuration matrix, included artifact classes, and capabilities of the configured adapters. A repository may have different semantic inventories under different feature sets, targets, profiles, generated inputs, or conditional-compilation environments.

“Complete” means that every artifact exposed by the declared discovery contract is either represented, excluded with a reason, unsupported with a named capability limitation, or failed with a visible diagnostic. Generated behavior, unsupported embedded languages, unresolved macros, and failed compilation units MUST remain visible in discovery and coverage accounting rather than disappearing from the denominator.

It SHALL produce an explicit manifest of every reviewable target.

Example:

```json
{
  "workspace": "example",
  "targets": 9481,
  "by_kind": {
    "crate": 17,
    "module": 241,
    "struct": 624,
    "enum": 301,
    "trait": 98,
    "function": 1670,
    "method": 4299,
    "field": 1840,
    "macro": 91,
    "other": 300
  }
}
```

Review state is represented by orthogonal dimensions rather than one overloaded disposition:

```rust
pub enum InventoryState {
    Discovered,
    Excluded,
}

pub enum ExecutionState {
    Pending,
    Running,
    Completed,
    Failed,
    Superseded,
}

pub enum ApplicabilityState {
    Applicable,
    NotApplicable,
}

pub enum AssessmentState {
    Pass,
    CandidateFinding,
    UnableToVerify,
}

pub enum VerificationState {
    NotRequired,
    Unverified,
    Corroborated,
    Disputed,
    Rejected,
    Inconclusive,
}

pub enum HumanAdjudicationState {
    NotRequired,
    AwaitingHuman,
    Confirmed,
    Rejected,
    Modified,
}
```

Suggestions are zero or more structured recommendations attached to an assessment; they are not a disposition. Every discovered target-policy pair MUST have an inventory, applicability, execution, and assessment state. Applicable candidate findings additionally have verification and human-adjudication state according to their configured pipeline.

No target-policy pair should silently disappear from the review process.

---

# 10. Review Policies

The core harness should treat review dimensions as independent policies.

```rust
pub trait ReviewPolicy {
    fn id(&self) -> PolicyId;

    fn version(&self) -> PolicyVersion;

    fn applies_to(
        &self,
        target: &ReviewTarget,
    ) -> bool;

    fn rubric(
        &self,
        classification: &TargetClassification,
    ) -> ReviewRubric;

    fn evidence_requirements(
        &self,
        target: &ReviewTarget,
    ) -> EvidenceRequirements;

    fn evidence_expansion_policy(&self) -> EvidenceExpansionPolicy;

    fn response_schema(&self) -> StructuredOutputSchema;

    fn default_pipeline(&self) -> PipelineId;

    fn severity_rules(&self) -> SeverityRules;

    fn verification_requirements(&self) -> VerificationRequirements;
}
```

Policies describe applicability, evidence, rubrics, schemas, and review requirements. They do not directly invoke models or mutate review context. Langchart workflow actors execute policies through governed provider and evidence interfaces.

Initial policies should include:

```text
DocumentationReviewPolicy
CorrectnessReviewPolicy
MaintainabilityReviewPolicy
PerformanceReviewPolicy
ArchitectureReviewPolicy
TestingReviewPolicy
ErrorHandlingReviewPolicy
SafetyReviewPolicy
ApiQualityReviewPolicy
DesignConsistencyReviewPolicy
```

This architecture makes the harness reusable beyond the initial documentation-review use case.

---

# 11. Documentation Review

Documentation review should assess more than whether comments exist.

## 11.1 Documentation Dimensions

Each target should be evaluated for applicable dimensions:

| Dimension | Purpose |
|---|---|
| Presence | Does useful documentation exist where appropriate? |
| Purpose | Does it explain why the artifact exists? |
| Behavior | Does it accurately describe observable behavior? |
| Inputs | Are significant inputs explained? |
| Outputs | Are results and semantics explained? |
| Errors | Are meaningful failure conditions described? |
| Panics | Are relevant panic conditions identified? |
| Safety | Are unsafe requirements and invariants documented? |
| Side Effects | Are mutation, I/O, locking, persistence, or external effects documented? |
| Invariants | Are important constraints documented? |
| Lifecycle | Is creation/use/destruction behavior clear where relevant? |
| Examples | Would examples materially improve understanding? |
| Relationships | Are significant relationships to other abstractions explained? |
| Accuracy | Does the documentation agree with evidence? |
| Completeness | Is behavior omitted that a consumer or maintainer needs? |
| Currency | Does the documentation describe the current implementation? |
| Value | Does the documentation add information beyond the declaration itself? |

---

# 12. Documentation Claim Verification

Documentation should be interpreted as a collection of claims.

Example:

```rust
pub struct DocumentationClaim {
    pub id: ClaimId,
    pub target: TargetId,
    pub text: String,
    pub category: ClaimCategory,
    pub evidence: Vec<EvidenceRef>,
    pub status: ClaimStatus,
}
```

Statuses:

```rust
pub enum ClaimStatus {
    Supported,
    PartiallySupported,
    Unsupported,
    Contradicted,
    UnableToVerify,
}
```

This allows the system to distinguish between:

- missing documentation,
- weak documentation,
- stale documentation,
- misleading documentation,
- documentation that is internally correct but conflicts with design,
- and unverifiable statements.

---

# 13. Correctness Review

Correctness review should ask whether the implementation actually satisfies its apparent contract.

The reviewer should consider:

- Incorrect logic
- Boundary-condition failures
- Incorrect state transitions
- Broken invariants
- Invalid assumptions
- Incorrect error handling
- Arithmetic overflow or underflow risks
- Incorrect ownership or lifecycle behavior
- Resource leaks
- Incorrect concurrency behavior
- Race conditions
- Deadlocks
- Lock-order problems
- Transactional correctness
- Persistence correctness
- Serialization/deserialization mismatches
- Incorrect caching behavior
- Stale state
- Improper retries
- Incorrect idempotency assumptions
- Data corruption risks
- Invalid unsafe assumptions
- Unreachable or contradictory branches

Correctness findings MUST identify the evidence leading to the conclusion.

The reviewer SHOULD avoid reporting purely speculative bugs without a plausible failure path.

---

# 14. Maintainability Review

Maintainability review should focus on future engineering cost rather than stylistic preference.

It should assess:

- Excessive complexity
- Overly large functions
- Mixed responsibilities
- Poor abstraction boundaries
- Hidden coupling
- Excessive coupling
- Repeated logic
- Divergent duplicate logic
- Difficult ownership relationships
- Fragile control flow
- Excessive nesting
- Boolean-parameter abuse
- Configuration scattering
- Magic values
- Inappropriate global state
- Unclear naming where meaning cannot be inferred
- Misleading names
- Unnecessary cleverness
- Inconsistent patterns
- Dead code
- TODOs
- FIXME markers
- unimplemented paths
- placeholders
- stubs
- unreachable placeholders
- poor error propagation
- unnecessary type erasure
- overly broad interfaces
- leaky abstractions

The policy should explicitly avoid turning personal formatting or stylistic preferences into findings unless they materially affect comprehension or maintenance.

---

# 15. Performance Review

Performance analysis should be evidence-driven and conservative.

The harness should distinguish:

```text
Confirmed Performance Issue
Likely Performance Risk
Potential Optimization Opportunity
Insufficient Evidence
```

Areas of review include:

- Unexpected algorithmic complexity
- Repeated linear scans
- Nested expensive operations
- Avoidable allocations
- Excessive copying or cloning
- Poor memory locality
- Repeated serialization
- Repeated parsing
- Unbounded data structures
- Excessive locking
- Lock contention
- Blocking work in asynchronous contexts
- Excessive synchronization
- N+1 operations
- Excessive database round trips
- Missing batching
- Repeated expensive computation
- Incorrect cache usage
- Cache invalidation patterns
- Large hot-path abstractions
- Excessive dynamic dispatch in performance-sensitive paths
- Unnecessary intermediate collections
- Memory retention
- Unbounded queues
- Accidental quadratic or worse behavior

The reviewer should not recommend micro-optimizations without evidence that the code is relevant to performance.

Where available, performance review SHOULD use:

- benchmarks,
- profiling output,
- call frequency,
- telemetry,
- known hot-path annotations,
- or design documentation

to prioritize findings.

---

# 16. Architecture Review

Architecture review operates at a higher level than declaration review.

It should evaluate:

- Clear subsystem boundaries
- Layering violations
- Dependency direction
- Cyclic dependencies
- Separation of concerns
- Cross-cutting concerns
- Encapsulation
- Public API boundaries
- Domain model cohesion
- Excessive knowledge across layers
- Architecture erosion
- Duplicate abstractions
- Inconsistent abstractions
- Hidden dependencies
- Inappropriate persistence awareness
- Inappropriate transport awareness
- Mixing policy and mechanism
- Incorrect ownership of responsibilities
- Misplaced shared functionality
- Inconsistent error models
- Inconsistent lifecycle models
- Inconsistent concurrency models
- Violations of ADRs or declared architecture

Architecture review should operate progressively at:

```text
Module
    ↓
Package / Crate
    ↓
Subsystem
    ↓
Workspace / Application
```

---

# 17. Test Review

Tests provide both review targets and evidence.

The test-review policy should consider:

- Missing tests for important behavior
- Missing boundary tests
- Missing error-condition tests
- Missing concurrency tests
- Missing persistence/recovery tests
- Missing security-sensitive tests
- Overly implementation-specific tests
- Tests that do not verify their stated behavior
- Weak assertions
- Duplicate tests
- Flaky patterns
- Excessive mocking
- Missing integration coverage
- Missing regression tests around discovered defects

Tests should also be indexed against the artifacts they exercise.

Example:

```text
Transaction::commit
    tested_by:
        transaction_commit_visibility
        commit_rejects_invalid_state
        snapshot_does_not_see_future_commit
```

---

# 18. Evidence Model

Every semantic conclusion should be traceable to evidence.

```rust
pub enum EvidenceKind {
    Source,
    Documentation,
    Test,
    Benchmark,
    StaticAnalysis,
    CompilerDiagnostic,
    DesignDocument,
    Adr,
    Prd,
    CallSite,
    TypeDefinition,
    TraitDefinition,
    RuntimeMetric,
}
```

```rust
pub struct EvidenceRef {
    pub id: EvidenceId,
    pub kind: EvidenceKind,
    pub location: EvidenceLocation,
    pub summary: String,
}
```

The reviewer SHOULD distinguish direct evidence from inference.

---

# 19. Evidence Priority and Conflicts

The harness MUST NOT assume that source code is always authoritative.

For example:

```text
Design Document
      / \
     /   \
Tests --- Documentation
     \   /
      \ /
Implementation
```

Any pair may disagree.

The system should explicitly classify divergence.

```rust
pub enum Divergence {
    DocumentationVsImplementation,
    DocumentationVsDesign,
    TestsVsImplementation,
    TestsVsDesign,
    ImplementationVsDesign,
    MultipleSourcesDisagree,
}
```

When documentation and design agree but implementation differs, the system SHOULD NOT simply recommend changing the documentation to match the implementation.

Instead, it should report the inconsistency.

---

# 20. Context Builder

The context builder is one of the most important components in the system.

An individual review should receive a bounded package of immediately relevant information.

Example:

```text
TARGET
GraphStore::insert_edge

DECLARATION
pub fn insert_edge(...)

DOCUMENTATION
...

IMPLEMENTATION
...

PARENT TYPE
GraphStore

RELATED TRAIT
GraphStorage

DIRECT CALLEES
validate_edge
write_edge
update_index

IMPORTANT CALLERS
Graph::add_edge
Importer::import_edge

RELATED TESTS
insert_edge_creates_adjacency
duplicate_edge_rejected

RELATED DESIGN
ADR-017 Edge Transaction Semantics
```

The system should NOT eagerly include large unrelated portions of the repository.

---

# 21. Progressive Evidence Retrieval

Reviewers should be able to request more context when uncertainty exists.

Example state flow:

```text
Load Target
    ↓
Initial Evidence
    ↓
Evaluate
    ↓
Evidence Sufficient? ─────── yes ──────► Complete
    │
    no
    │
    ▼
Request Additional Evidence
    │
    ├── caller
    ├── callee
    ├── type definition
    ├── implementation
    ├── trait
    ├── test
    ├── sibling implementation
    ├── ADR
    ├── design section
    └── benchmark
    │
    ▼
Evaluate Again
```

Evidence retrieval should be bounded by policy-defined limits to prevent uncontrolled context expansion.

---

# 22. Review Roles

Rather than one universal prompt, the system should use specialized review roles.

## 22.1 Target Reviewer

Reviews a single declaration or semantic artifact.

Questions include:

- Is this artifact understandable?
- Is its documentation sufficient?
- Is the implementation correct?
- Are there maintainability problems?
- Are there likely performance concerns?
- Are important tests missing?

## 22.2 Consistency Reviewer

Compares related evidence.

Questions include:

- Do implementation and documentation agree?
- Do tests express the same contract?
- Do ADRs or design documents conflict with code?
- Do sibling implementations behave consistently?

## 22.3 Aggregate Reviewer

Operates on collections of reviewed artifacts.

Questions include:

- Is this module coherent?
- Are responsibilities well separated?
- Is the crate architecture understandable?
- Are there repeated issues indicating a systemic problem?
- Does the workspace architecture follow its declared design?

---

# 23. Target-Specific Rubrics

Review expectations should depend on target kind.

## 23.1 Crate / Package

Review:

- Responsibility
- Scope
- Public interface
- Architectural role
- Dependency relationships
- Major abstractions
- Cross-cutting invariants
- Entry points
- Documentation quality
- Design consistency

## 23.2 Module

Review:

- Cohesion
- Responsibility
- Boundary clarity
- Parent/child relationships
- Public surface
- Internal abstraction
- Documentation
- Dependency direction

## 23.3 Type

Review:

- Domain meaning
- Invariants
- Lifecycle
- Ownership
- Mutability
- Threading/concurrency semantics
- Serialization semantics
- Public API coherence

## 23.4 Function / Method

Review:

- Purpose
- Preconditions
- Inputs
- Outputs
- Side effects
- Error behavior
- Panic behavior
- Correctness
- Complexity
- Maintainability
- Performance
- Tests

## 23.5 Field / Variant

Review should be proportional.

Simple and obvious fields may need little or no additional explanation.

Complex fields should explain:

- semantics,
- units,
- lifecycle,
- ownership,
- invariants,
- optionality,
- and non-obvious behavior.

---

# 24. Significance Model

Not all targets deserve equal review depth.

The harness should calculate an approximate review significance score.

Potential factors include:

```text
Public API
Unsafe Code
Persistence Boundary
Transaction Boundary
Concurrency
Security Boundary
Network Boundary
Serialization Format
Cross-Crate Usage
High Fan-In
High Fan-Out
Complex Control Flow
State Machine Logic
Data Mutation
Resource Management
Architecture Boundary
Performance Hot Path
Error-Prone Domain
```

Example:

```rust
pub enum ReviewImportance {
    Critical,
    High,
    Normal,
    Low,
}
```

Importance can influence:

- review depth,
- evidence budget,
- required documentation quality,
- required testing,
- and model effort.

---

# 25. Deterministic Analysis First

The LLM should not spend effort detecting conditions that deterministic tooling can discover reliably.

Before semantic review, run applicable tooling.

For Rust:

```text
cargo check
cargo test
cargo test --doc
cargo clippy
cargo doc
rustdoc diagnostics
custom AST / semantic checks
```

Potential deterministic documentation checks:

- Missing public documentation
- Missing crate documentation
- Missing module documentation
- Unsafe functions missing Safety documentation
- Broken rustdoc links
- Failing examples
- References to nonexistent parameters
- References to removed symbols

Potential deterministic code checks:

- Compiler warnings
- Clippy findings
- Dead code
- Unreachable code
- obvious unused results
- suspicious casts
- known problematic patterns

These become evidence supplied to the semantic reviewers.

---

# 26. Finding Model

Findings should be structured before they are rendered.

```rust
pub struct Finding {
    pub id: FindingId,
    pub target: TargetId,

    pub policy: PolicyId,
    pub category: FindingCategory,
    pub severity: Severity,

    pub title: String,
    pub explanation: String,

    pub evidence: Vec<EvidenceRef>,
    pub recommendation: Option<String>,

    pub confidence: Confidence,
}
```

Example categories:

```text
DOC_MISSING
DOC_INCOMPLETE
DOC_INACCURATE
DOC_STALE

CORRECTNESS_BUG
CORRECTNESS_RISK
INVARIANT_VIOLATION

MAINTAINABILITY_COMPLEXITY
MAINTAINABILITY_DUPLICATION
MAINTAINABILITY_COUPLING
MAINTAINABILITY_ABSTRACTION

PERFORMANCE_COMPLEXITY
PERFORMANCE_ALLOCATION
PERFORMANCE_CONTENTION
PERFORMANCE_IO

ARCH_BOUNDARY
ARCH_DEPENDENCY
ARCH_DUPLICATION
ARCH_INCONSISTENCY

TEST_MISSING
TEST_INSUFFICIENT

DESIGN_DIVERGENCE
```

---

# 27. Severity Model

Suggested severity levels:

## Critical

Likely to cause:

- data loss,
- corruption,
- serious security failure,
- unsafe behavior,
- catastrophic outage,
- or fundamentally incorrect behavior.

## Major

Material defect likely to result in:

- incorrect behavior,
- incorrect API use,
- significant maintainability cost,
- significant architecture violation,
- or serious performance degradation.

## Moderate

Important issue that should be addressed but is not immediately catastrophic.

Examples:

- missing important documentation,
- weak abstraction boundaries,
- substantial complexity,
- meaningful missing test coverage.

## Minor

Localized quality issue with limited impact.

## Suggestion

Optional improvement where current behavior is acceptable.

---

# 28. Confidence Model

Every semantic finding should include confidence.

```rust
pub enum Confidence {
    High,
    Medium,
    Low,
}
```

Low-confidence findings should remain visible but distinguishable from stronger conclusions.

The system should prefer an explicit `UnableToVerify` result over inventing certainty.

Confidence is not a substitute for verification. A high-confidence model response remains a model-generated assessment until it passes the configured verification and adjudication workflow.

## 28.1 Verification and Adjudication

Candidate findings SHOULD be processed through a configurable verification pipeline.

Example:

```text
Local model discovers candidate finding
    ↓
Independent verification review
    ↓
Optional escalation to a frontier model
    ↓
Human review queue
```

Verification stages MAY use the same isolated model, different local models, or different providers. A run configuration determines the required number and type of reviews. For example, a local model may discover a candidate while a frontier model performs deeper inspection.

The verifier SHOULD independently assess the evidence before receiving the original reviewer's reasoning. This reduces correlated confirmation of an incorrect explanation.

Suggested adjudication states:

```rust
pub enum AdjudicationState {
    Unverified,
    Corroborated,
    Disputed,
    RejectedByVerifier,
    Inconclusive,
    AwaitingHumanReview,
    HumanConfirmed,
    HumanRejected,
    HumanModified,
}
```

Every verification attempt MUST record its model and provider identity, model capability profile, policy and prompt versions, evidence set, result, and reason for escalation.

Preconfigured online escalation constitutes authorization only for the provider, repository scope, and source classifications named by the run configuration. Targeted follow-up MAY instead be requested from the CLI, MCP, or a generated report. Repository content MUST NOT be silently transmitted to an online provider.

---

# 29. Iterative Review Strategy

The review process should occur in layers.

## Pass 1 — Inventory

Discover every target.

## Pass 2 — Deterministic Analysis

Run compiler, tests, linting, documentation checks, and language-specific analyzers.

## Pass 3 — Declaration Review

Review each relevant semantic target individually.

## Pass 4 — Relationship Review

Examine related groups:

- trait and implementations,
- callers and callees,
- interface and implementation,
- serializer and deserializer,
- transaction state transitions,
- test and target,
- abstraction and consumers.

## Pass 5 — Module Review

Assess the module as a coherent unit.

## Pass 6 — Crate / Package Review

Review architectural purpose, public surface, dependencies, and internal cohesion.

## Pass 7 — Application Architecture Review

Evaluate cross-package relationships and declared architecture.

## Pass 8 — Cross-Cutting Synthesis

Identify recurring themes and systemic weaknesses.

Example:

```text
17 separate functions manually implement retry logic.

Individually:
    maintainability concern

Collectively:
    missing retry-policy abstraction
```

This is a major reason aggregate review is required.

---

# 30. Review Scheduler

The scheduler should prioritize dependency-aware execution.

Suggested order:

```text
Leaf declarations
    ↓
Types / implementations
    ↓
Modules
    ↓
Crates / packages
    ↓
Subsystems
    ↓
Workspace
```

However, architectural metadata may be indexed before lower-level reviews so it can serve as evidence.

Independent target reviews may execute concurrently.

The scheduler is a resumable local coordinator. Model hosting, batching across accelerators, and distributed inference are responsibilities of the configured model providers rather than the harness.

The scheduler should provide:

- provider-specific concurrency limits,
- backpressure,
- retry with bounded backoff,
- provider health and rate-limit handling,
- crash-safe checkpoints,
- deterministic work identity,
- resumable work leases,
- bounded evidence construction,
- and fair scheduling between lightweight and deep reviews.

Audits may run for one or two weeks. Progress observability is therefore part of execution correctness. State and logs MUST be flushed regularly, and status MUST expose monotonic progress, last successful completion, queue depth, provider health, retry and failure rates, evidence-expansion and unable-to-verify rates, resource and estimated cost usage where available, disk usage, and an estimated completion range. Continued log output alone MUST NOT be treated as proof of forward progress; the scheduler SHOULD detect and report stalled partitions and work items.

Within a specific audit run and analysis configuration, the scheduler MUST expose exactly one effective outcome per applicable target-policy pair while retaining an append-only history of attempts, retries, verification, and superseded outcomes.

The repository-scale scheduler and the model-review workflow engine are deliberately separate. The scheduler admits bounded work items and owns global coverage. Langchart executes the stateful workflow for one admitted review unit. The scheduler MUST NOT encode an entire large repository as one statechart or create a live workflow task for every pending target.

---

# 31. Coverage Accounting

Coverage is a first-class output and is calculated per target-policy pair within one audit snapshot and analysis configuration.

Example:

```text
Documentation Audit

Inventory coverage
  Targets discovered:                9,268
  Explicitly excluded:                 319

Applicability coverage
  Applicable target-policy pairs:    8,825
  Not applicable:                      124

Execution coverage
  Completed:                         8,710
  Failed:                               15
  Pending:                              100

Evidence adequacy
  Adequately assessed:               8,552
  Unable to verify:                    158

Verification coverage
  Candidate findings requiring it:     201
  Independently verified:              196
  Pending or inconclusive:                5

Human adjudication
  Required:                              47
  Completed:                             41
  Pending:                                6
```

Coverage MUST distinguish:

```text
Inventory coverage
Policy applicability coverage
Execution completion
Evidence adequacy
Independent verification coverage
Human adjudication coverage
Adapter capability coverage
```

A completed execution that results in `UnableToVerify` MUST NOT count as an adequately reviewed target-policy pair. A report claiming comprehensive review MUST NOT hide failed, pending, excluded, unable-to-verify, unverified, or skipped work.

Reports MUST distinguish process coverage from validated review capability. Completing every scheduled target-policy pair establishes that the configured process ran; it does not establish that the inventory, evidence builder, or model found every real defect. Precision, recall, and calibration claims come only from applicable adjudicated evaluation data.

---

# 32. Design Document Integration

The harness should index engineering documents such as:

```text
ADRs
PRDs
Architecture Documents
Design Specifications
Protocol Specifications
README Files
Operational Runbooks
```

Documents should be broken into meaningful sections.

Each section should support:

- full-text search,
- optional vector search,
- metadata filtering,
- references to known source symbols.

Example:

```text
ADR-024 Transaction Visibility

references:
  Transaction
  Snapshot
  CommitTimestamp
  VersionChain
```

Reviewers should retrieve design evidence only when relevant.

---

# 33. Symbol-to-Design Linking

Where possible, design documents should explicitly reference source concepts.

The harness may maintain:

```rust
pub struct DesignLink {
    pub document: DocumentId,
    pub section: SectionId,
    pub target: TargetId,
    pub relation: DesignRelation,
}
```

Example relations:

```text
defines
constrains
motivates
describes
supersedes
deprecates
```

These links may be manually authored, automatically inferred, or both.

---

# 34. Incremental Review

Full repository reviews may be expensive.

The harness should maintain review fingerprints.

```rust
pub struct ReviewFingerprint {
    pub audit_snapshot: AuditSnapshotId,
    pub target_hash: Hash,
    pub implementation_hash: Hash,
    pub documentation_hash: Hash,

    pub dependency_hash: Hash,
    pub test_hash: Hash,
    pub design_hash: Hash,

    pub policy_version: String,
    pub prompt_version: String,
    pub workflow_hash: Hash,
    pub actor_versions: Vec<String>,
    pub model_reuse_class: String,
    pub evidence_builder_version: String,
    pub extension_versions: Vec<String>,
    pub toolchain_hash: Hash,
}
```

A review result may be reused when relevant inputs have not changed.

Changes should invalidate dependent reviews.

Example:

```text
Function changed
    → re-review function
    → re-review directly dependent consistency checks
    → potentially invalidate parent module aggregate

ADR changed
    → re-review linked targets
    → re-review linked module/crate architecture
```

---

# 35. Review Dependency Graph

Incremental review benefits from an explicit dependency graph.

Example:

```text
Function
├── depends on type declarations
├── depends on called functions
├── tested by tests
├── constrained by ADR
└── aggregated into module review
```

Review invalidation SHOULD propagate through review dependencies rather than simply through source-file timestamps.

Invalidation MUST include changes to source, dependency resolution, build configuration, generated inputs, design evidence, policies, prompts, workflow definitions, actor versions, model/provider identity where required by reuse policy, toolchain versions, and evidence-producing extensions.

When dependency information is incomplete, Argus MUST invalidate conservatively. If it cannot prove that a changed input is irrelevant, it invalidates the affected partition or a configured safe ancestor rather than reusing potentially stale review results.

---

# 36. Persistence Model

The harness uses a single `.argus/` repository namespace with three lifecycle-specific subdirectories:

```text
.argus/
├── config/                 Durable, human-authored configuration
├── state/                  Resumable but ephemeral working state
├── reviews/                Portable finalized review bundles
└── .gitignore
```

## 36.1 Durable Configuration

```text
.argus/config/
├── argus.toml
├── exclusions.toml
├── policies/
├── pipelines/
├── extensions/
├── publishers/
└── baselines/
```

Configuration, policies, exclusions, and accepted baselines are text-based and are normally committed to version control. Credentials MUST NOT be stored in this directory. Provider and publisher configuration should reference environment variables, credential helpers, or external secret stores.

## 36.2 Ephemeral Working State

```text
.argus/state/
├── review.redb
├── blobs/
├── logs/
├── locks/
└── tmp/
```

The `redb` database is the local working store for a review. It supports scheduling, indexes, evidence lookup, content deduplication, intermediate model attempts, aggregation, crash recovery, and resumption of long-running audits.

This state is ephemeral in the sense that it is not a portable audit artifact and may be discarded after successful finalization. It MUST nevertheless survive ordinary process interruption because an audit of a massive repository may run for a long period.

## 36.3 Portable Review Bundles

```text
.argus/reviews/<run-id>/
├── manifest.json
├── configuration.json
├── coverage.json
├── targets.jsonl
├── assessments.jsonl
├── findings.jsonl
├── verification.jsonl
├── adjudications.jsonl
├── exclusions.jsonl
├── evidence.jsonl
├── publications.jsonl
├── reports/
│   ├── summary.md
│   ├── report.html
│   └── results.sarif
└── artifacts/
```

Finalized bundles use versioned, text-based formats and MUST NOT depend on `redb`. JSONL is preferred for large record collections because it supports streaming and produces inspectable diffs. TOML is preferred for human-authored configuration. Markdown and HTML are rendered views rather than sources of truth.

The manifest should include hashes for bundle files, source revision and dirty-tree state, analysis configuration, toolchain identity, model and provider identities, policy versions, extension versions, and the guarantees offered by the bundle.

Bundles MUST distinguish four properties:

- **Auditable:** conclusions and their recorded support can be inspected.
- **Reproducible:** the inputs and tooling needed to rerun the audit are available.
- **Portable:** the bundle can be read without the working database.
- **Self-contained:** all supporting evidence is embedded in the bundle.

Not every bundle must be self-contained, but the manifest MUST state which guarantees apply. Large evidence artifacts may be stored externally when their hashes and resolvable references are retained.

Human adjudications survive finalization only when exported into a portable bundle or promoted into a text-based baseline. A historical adjudication MUST remain distinct from a reusable suppression or baseline decision.

Bundle finalization MUST be atomic from the reader's perspective:

```text
write <run-id>.partial/
    → close and flush records
    → validate every schema and reference
    → sort records by documented stable keys
    → encode canonical JSON/JSONL
    → compute file hashes and manifest
    → write finalized commit marker
    → atomically rename to <run-id>/
```

External destinations use an equivalent final commit marker when atomic rename is unavailable. Readers MUST ignore partial bundles. Re-finalizing the same logical run either reproduces the same manifest hash or creates an explicitly superseding bundle.

Every bundle file has a schema identifier and version. Importers MUST reject unsupported major versions, preserve unknown fields where required by the compatibility policy, and report migrations. Optional signing MAY provide tamper evidence in addition to content hashes.

## 36.4 Version-Control Behavior

The `.argus/.gitignore` file SHOULD ignore working state without modifying the repository's root namespace:

```gitignore
/state/
```

Review storage is configurable:

```text
local       Ignore .argus/reviews and keep bundles locally
artifact    Ignore .argus/reviews and publish bundles as CI artifacts
versioned   Do not ignore .argus/reviews
external    Finalize bundles outside the repository
```

For `local` and `artifact` modes, `.argus/.gitignore` also contains:

```gitignore
/reviews/
```

The `.argus/config/` directory MUST NOT be ignored by Argus. If repository or global Git rules already ignore `.argus/`, initialization SHOULD warn the operator.

---

# 37. Human-Readable Report

The primary report should be Markdown.

Suggested structure:

```text
1. Executive Summary
2. Scope and Coverage
3. Critical Findings
4. Major Findings
5. Cross-Cutting Themes
6. Documentation Quality
7. Correctness
8. Maintainability
9. Performance
10. Architecture
11. Testing
12. Design / ADR Divergence
13. Crate / Package Reviews
14. Module Reviews
15. Detailed Declaration Findings
16. Exclusions
17. Review Failures
18. Methodology
```

The report should emphasize actionable findings rather than dumping every successful review.

---

# 38. Per-Target Review Records

Even successful reviews should remain available in the machine-readable audit trail.

Example:

```json
{
  "target": "storage::Transaction::commit",
  "policy": "documentation",
  "inventory_state": "discovered",
  "execution_state": "completed",
  "assessment_state": "pass",
  "verification_state": "not_required",
  "human_adjudication_state": "not_required",
  "checks": {
    "purpose": "pass",
    "behavior": "pass",
    "errors": "pass",
    "panics": "not_applicable",
    "accuracy": "pass",
    "examples": "suggestion"
  },
  "findings": [],
  "suggestions": [
    "Consider adding an end-to-end transaction example."
  ]
}
```

This proves that the target was actually considered.

---

# 39. CI Integration

The harness should support both exhaustive and incremental modes.

## Pull Request Mode

Review:

- changed targets,
- impacted targets,
- relevant architecture relationships,
- affected documentation,
- affected tests.

Possible failure policy:

```text
Fail CI on new Critical findings
Fail CI on new Major correctness findings
Warn on maintainability findings
Warn on documentation regressions
```

CI policy MUST state which finding maturity levels are eligible to gate:

```text
Machine Candidate
Machine Corroborated
Human Confirmed
Human Rejected or Modified
Pending Required Adjudication
```

Representative gate modes include:

```text
advisory
    Publish candidates but never fail CI

conservative
    Fail only on findings confirmed by a human

automated
    Fail on independently corroborated findings matching configured severity/policy rules

strict
    Fail while required verification or adjudication remains pending
```

CI MUST NOT wait indefinitely for interactive human input. A configured timeout produces an explicit pending-adjudication gate result according to the selected mode. Candidate, corroborated, disputed, rejected, and human-confirmed findings remain separately visible in machine-readable output.

## Scheduled Full Audit

Periodically review the entire repository.

Example:

```text
Nightly
Weekly
Before Major Release
```

A full audit is valuable because dependency heuristics may occasionally miss indirect effects.

---

# 40. Baseline Support

Existing repositories may contain many historical findings.

The harness should support a baseline.

```text
Existing Finding
    → recorded but does not fail CI

New Finding
    → evaluated against CI policy

Worsened Finding
    → treated as new regression

Resolved Finding
    → removed from active baseline
```

This allows adoption without requiring immediate remediation of an entire legacy codebase.

Exclusions, baselines, suppressions, and disputed-finding decisions are operator-owned governance records. Every change MUST record actor identity, timestamp, reason, affected scope, previous value, source snapshot, and optional expiration. Local CLI runs use a configured operator identity with the operating-system identity recorded as supporting provenance. Broad exclusions and expired suppressions remain visible in coverage and executive reports.

---

# 41. Stable Finding Identity

Findings should attempt to survive source movement and line-number changes.

A finding identity should incorporate:

```text
Policy
Target semantic identity
Finding category
Normalized issue signature
```

rather than only:

```text
filename + line number
```

This makes baseline management more reliable.

---

# 42. Model Interaction Contract

Model outputs should be structured.

Example response schema:

```json
{
  "assessment_state": "candidate_finding",
  "summary": "...",
  "checks": [],
  "candidate_findings": [],
  "suggestions": [],
  "requested_evidence": [],
  "unable_to_verify_reason": null,
  "confidence": "high"
}
```

The harness should validate model output before accepting it.

Invalid outputs should be retried or marked as review failures.

The model should never control inventory or coverage state directly.

## 42.1 Model Provider Capabilities

Local, same-network, and online providers are first-class deployment modes. A provider adapter SHOULD advertise capabilities rather than being treated as an interchangeable text endpoint.

Representative capabilities include:

```text
Model and provider identity
Context-window size
Maximum output size
Structured-output reliability
Tool-calling support
Concurrency capacity
Health and readiness
Timeout behavior
Supported input classifications
```

The harness SHOULD tolerate local-provider limitations through health checks, bounded retries, schema-repair attempts, concurrency throttling, circuit breaking, checkpointing, and failure isolation by target-policy pair. A provider failure MUST NOT corrupt the audit or silently convert an incomplete review into a pass.

Model distribution and hosting are external to the harness. Argus coordinates provider requests but does not manage GPU clusters or distributed inference.

Quality and audit defensibility MUST NOT be traded away merely to reduce latency or token cost. The harness SHOULD nevertheless avoid redundant work and support local providers so operators can control operational cost and data exposure.

---

# 43. Reviewer Prompt Principles

All reviewer prompts should enforce:

1. Evaluate only the requested target and evidence.
2. Do not invent unavailable context.
3. Distinguish observation from inference.
4. Cite evidence for findings.
5. Do not manufacture issues merely to produce output.
6. Avoid stylistic complaints without engineering impact.
7. Prefer `UnableToVerify` when evidence is insufficient.
8. Request additional evidence only when materially useful.
9. Consider whether apparent code/documentation divergence may represent an implementation defect.
10. Explain consequences, not merely rules.
11. Review the artifact in the context of its abstraction level.
12. Avoid suggesting code changes unless needed to explain remediation.

---

# 44. Architecture of the Harness Itself

A practical implementation could be divided into the following components.

```text
review-core
    Domain model
    Review scheduler
    Policy API
    Evidence model
    Findings
    Coverage
    Fingerprints

review-language
    Language adapter interfaces
    immutable source access
    portable target classification
    adapter capability and limitation model

review-rust
    Cargo discovery
    rustdoc ingestion
    AST / semantic extraction
    compiler integration
    Clippy integration
    Rust-specific deterministic checks

review-documents
    ADR / PRD / design ingestion
    indexing
    symbol linking
    retrieval

review-context
    Evidence construction
    progressive disclosure
    context budgeting

review-agent
    Model provider abstraction
    prompts
    structured outputs
    retry / validation

argus-langchart
    versioned workflow templates
    review and verification actors
    workflow-data and event schemas
    evidence-reference resolution
    Langchart checkpoint integration
    runtime-event translation

review-policies
    Documentation
    Correctness
    Maintainability
    Performance
    Architecture
    Testing
    Safety
    Design consistency

review-storage
    redb working state
    portable bundle export and import
    fingerprint storage
    baseline state

review-report
    Markdown
    HTML
    JSON
    SARIF
    CI summaries

review-extensions
    extension registry and capability declarations
    compiled-in integrations
    subprocess protocol
    evidence ingestion
    rendering and publishing

review-mcp
    MCP resources and tools
    asynchronous job lifecycle

review-cli
    Command-line interface
```

## 44.1 Extension Capabilities

Extensions cover the full audit lifecycle and SHOULD declare one or more constrained capabilities:

```text
Discoverer
    Finds projects, targets, relationships, and documents

Evidence Producer
    Runs or ingests builds, tests, linters, benchmarks, and profiles

Review Policy
    Produces assessments and candidate findings

Verifier
    Corroborates, disputes, or escalates candidate findings

Renderer
    Produces Markdown, HTML, JSON, JSONL, SARIF, or other views

Publisher
    Creates or updates external records such as GitHub or Beads issues

Importer
    Reads prior reviews, baselines, suppressions, and external findings
```

Build, test, benchmark, and similar integrations SHOULD support both active execution and ingestion of externally produced results. This permits use in locked-down CI environments where the build runs outside Argus.

Extensions SHOULD declare:

- identity, version, and protocol version,
- capabilities and applicable project types,
- inputs, outputs, and configuration schema,
- produced evidence kinds and affected targets,
- execution, network, filesystem, and secret requirements,
- timeout, resource, and output-size limits,
- whether repository-controlled code is executed,
- and determinism and cacheability characteristics.

First-party native extensions MAY be compiled into Argus through Cargo features. External extensions SHOULD initially use a versioned subprocess protocol with JSON or JSONL messages or manifest/result files. Dynamically loaded native libraries are not required. Compiled-in and subprocess extensions SHOULD share the same logical domain contract even when their Rust interfaces differ.

Extension installation and active execution are operator-controlled. Extension output is untrusted evidence and MUST be validated. Extension failure MUST be isolated from the audit store.

### 44.1.1 Subprocess Protocol

The initial subprocess protocol is a versioned JSONL message stream over standard input and standard output. Standard error is reserved for human-readable diagnostics and MUST NOT contain protocol records.

```text
Host → handshake
Extension → handshake.accepted | handshake.rejected
Host → configure
Extension → configure.accepted | configure.rejected
Host → start
Extension → record* | progress* | heartbeat*
Host → cancel, when required
Extension → completed | failed | cancelled
```

The handshake negotiates protocol version, extension identity, capabilities, supported schemas, maximum record size, streaming support, cancellation support, and declared execution requirements.

The protocol MUST define:

- canonical message envelopes and correlation IDs,
- schema-version compatibility,
- maximum message and total-output sizes,
- backpressure and bounded buffering,
- heartbeat and timeout behavior,
- cancellation acknowledgement and forced termination,
- deterministic exit-code meanings,
- path normalization relative to the audit snapshot,
- secret references rather than serialized secret values,
- idempotency keys for side-effecting operations,
- and behavior after malformed or unexpected messages.

Publisher subprocesses MUST NOT perform an external mutation before receiving an explicit publish command containing the destination and idempotency key. Evidence-producer subprocesses SHOULD be restartable from declared inputs; partial output is retained only when its record-level validity and provenance can be established.

## 44.2 Rendering and External Publishing

Renderers are side-effect-free transformations of structured review records. Publishers perform externally visible mutations and therefore use a separate contract.

Publishers SHOULD support:

- explicit destination and authentication requirements,
- dry-run behavior,
- stable idempotency keys and duplicate detection,
- create, update, and optionally close behavior,
- rate-limit and retry handling,
- selection rules,
- provenance links,
- and durable publication receipts.

An ordinary audit MUST NOT unexpectedly create external issues. Publishing requires a separate command or an explicitly configured final pipeline stage. Automatic closure of external issues MUST be opt-in.

Rendered reports and external issue trackers are not sources of truth. Structured audit records feed renderers and publishers.

## 44.3 Langchart Workflow Orchestration

Argus SHALL use Langchart as the statechart-based orchestration engine for bounded model-assisted review workflows.

Langchart is responsible for the state transitions within one review unit. Argus remains responsible for repository discovery, global scheduling, coverage, evidence and finding identity, durable audit records, final review bundles, and external publication guarantees.

```text
Argus Audit Scheduler
    ├── target A × documentation policy ─► Langchart run
    ├── target A × correctness policy   ─► Langchart run
    ├── target B × documentation policy ─► Langchart run
    ├── relationship group              ─► Langchart run
    └── module/crate aggregate          ─► Langchart run
```

This separation permits repository-scale work to remain disk-backed and mostly dormant while a bounded set of admitted Langchart runs execute concurrently.

### 44.3.1 Review Work Item

The scheduler creates a stable work item before launching a workflow.

```rust
pub struct ReviewWorkItem {
    pub work_id: WorkId,
    pub audit_run_id: AuditRunId,

    pub kind: ReviewWorkKind,
    pub unit: ReviewUnit,
    pub policy_id: PolicyId,
    pub policy_version: String,
    pub pipeline_id: PipelineId,
    pub workflow_version: String,

    pub evidence_revision: EvidenceRevision,
    pub analysis_configuration: AnalysisConfigurationId,
    pub significance: ReviewImportance,

    pub attempt: u32,
}

pub enum ReviewWorkKind {
    TargetAssessment,
    FindingVerification,
    FindingAdjudication,
    RelationshipAssessment,
    AggregateAssessment,
}
```

Representative review units include a target, one canonical finding, a relationship group, a module aggregate, a package/crate aggregate, or a workspace aggregate. One work item has one effective result within an audit run, but may retain multiple execution attempts.

### 44.3.2 Workflow Data

Workflow state SHOULD contain identities and small decision data rather than large source text, model transcripts, or evidence blobs.

```rust
pub struct ReviewWorkflowData {
    pub work_id: WorkId,
    pub target_or_group: ReviewUnitId,
    pub policy_id: PolicyId,
    pub evidence_package: EvidencePackageRef,
    pub evidence_revision: EvidenceRevision,

    pub primary_assessment: Option<AssessmentRef>,
    pub candidate_findings: Vec<FindingRef>,
    pub verification_results: Vec<VerificationRef>,
    pub requested_evidence: Vec<EvidenceRequest>,

    pub escalation_count: u32,
    pub evidence_expansion_count: u32,
    pub adjudication: Option<AdjudicationRef>,
}
```

Large values remain in Argus's evidence or assessment stores. Langchart events and checkpoints carry stable references and content hashes.

### 44.3.3 Target Review Workflow

The initial target-review workflow SHOULD follow this state structure:

```text
PrepareEvidence
    │
    ▼
PrimaryReview
    ├── review.pass ───────────────────────────► RecordOutcome
    ├── review.suggestion ─────────────────────► RecordOutcome
    ├── review.unable_to_verify ───────────────► EvaluateEvidenceRequest
    └── review.candidate_found ────────────────► RecordCandidates

EvaluateEvidenceRequest
    ├── request.allowed ─► ExpandEvidence ─────► PrimaryReview
    ├── request.denied ────────────────────────► RecordUnableToVerify
    └── budget.exhausted ──────────────────────► RecordUnableToVerify

RecordCandidates ──────────────────────────────► ScheduleFindingWork
ScheduleFindingWork ───────────────────────────► RecordOutcome
RecordUnableToVerify ──────────────────────────► RecordOutcome
RecordOutcome ─────────────────────────────────► Complete
```

`PrimaryReview` is a Langchart agentic state. Candidate findings are canonicalized and recorded before separate finding-verification work items are scheduled. The target assessment therefore does not force multiple findings through one shared verification outcome.

Argus deterministic activities are implemented as Langchart agentic states backed by non-LLM `AgentActor` implementations. These actors receive capability envelopes, emit validated events, participate in retry and checkpoint behavior, and MUST NOT invoke an LLM unless their declared capability permits it. Host-injected events are reserved for external control, provider availability, and human input; they are not an alternative general activity system.

The workflow MUST bound evidence expansion by request count, evidence size, relationship depth, and policy-specific limits. Exhaustion produces `UnableToVerify`; it MUST NOT be converted into a pass.

### 44.3.4 Finding Verification and Adjudication Workflow

Each canonical candidate finding receives its own verification work item and workflow:

```text
PrepareFindingEvidence
    ↓
VerificationGate
    ├── verification.not_required ─────────────► HumanReviewGate
    └── verification.required ─────────────────► IndependentVerification

IndependentVerification
    ├── verification.corroborated ─────────────► HumanReviewGate
    ├── verification.rejected ─────────────────► HumanReviewGate
    ├── verification.inconclusive ─────────────► EscalationGate
    └── verification.disputed ─────────────────► EscalationGate

EscalationGate
    ├── escalation.preconfigured ──────────────► FrontierVerification
    ├── escalation.requested ──────────────────► FrontierVerification
    └── escalation.not_authorized ─────────────► HumanReviewGate

FrontierVerification
    ├── verification.corroborated ─────────────► HumanReviewGate
    ├── verification.rejected ─────────────────► HumanReviewGate
    ├── verification.disputed ─────────────────► HumanReviewGate
    └── verification.inconclusive ─────────────► HumanReviewGate

HumanReviewGate
    ├── human.required ────────────────────────► AwaitHumanAdjudication
    └── human.not_required ────────────────────► RecordFindingOutcome

AwaitHumanAdjudication
    ├── human.confirmed ───────────────────────► RecordFindingOutcome
    ├── human.rejected ────────────────────────► RecordFindingOutcome
    └── human.modified ────────────────────────► RecordFindingOutcome

RecordFindingOutcome ──────────────────────────► Complete
```

Independent verification is an agent invocation with an isolated context. It receives the candidate claim, applicable evidence, and review rubric, but SHOULD NOT initially receive the primary reviewer's narrative reasoning.

The configured pipeline determines:

- number of verification reviews,
- whether the same isolated model is acceptable,
- required provider or model profiles,
- disagreement behavior,
- severity-based escalation,
- and human-review requirements.

The effective finding record MUST distinguish the original candidate, each verification result, the current verification state, the adjudication state, whether the finding is active, and the reason for rejection or modification. Rejected findings remain in the audit trail but are excluded from active severity counts and new-finding CI gates unless configuration explicitly requests otherwise.

To improve recall as well as precision, a pipeline MAY require multiple independent primary assessments, specialized reviewers, an adversarial pass, sampling of passes, or mandatory dual review for high-significance targets. Candidate-only verification improves precision but does not measure or improve missed-defect recall.

Model routing uses Langchart model profiles such as:

```text
local_primary
local_verifier
frontier_verifier
architecture_synthesis
```

An Argus provider-policy wrapper MUST additionally enforce provider capability, repository data classification, online-transmission authorization, health, and concurrency rules.

### 44.3.5 Aggregate Workflows

Module, crate/package, subsystem, and workspace review use separate workflow templates. They consume references to completed lower-level assessments and selected supporting evidence rather than replaying every underlying prompt and response.

```text
SelectConstituentResults
    ↓
DetectCrossCuttingPatterns
    ↓
RetrieveArchitectureEvidence
    ↓
SynthesizeAggregateAssessment
    ↓
VerifyAggregateFindings
    ↓
RecordAggregateOutcome
```

Aggregate workflows are scheduled only after required constituent work reaches an acceptable terminal state. Failed, excluded, pending, and unable-to-verify constituents remain visible to the aggregate reviewer and coverage report.

### 44.3.6 Workflow Actors

The `argus-langchart` integration SHOULD provide actors with narrow responsibilities:

```text
PrepareEvidenceActor
    Requests an evidence package from Argus

ReviewActor
    Invokes the configured model and validates structured output

EvidenceRequestEvaluator
    Applies deterministic policy and budget limits

ExpandEvidenceActor
    Resolves approved evidence requests through Argus

VerificationActor
    Performs isolated corroboration or dispute analysis

EscalationPolicyActor
    Applies configured provider and privacy rules

OutcomeRecorderActor
    Commits structured assessments and effective outcomes to Argus

AggregateReviewActor
    Reviews collections of lower-level results
```

Actors MUST access LLM, MCP, memory, artifact, and secret capabilities through Langchart's capability broker. Argus-specific actors MUST NOT make ungoverned provider calls.

### 44.3.7 Event Contract

Workflow transitions SHOULD use versioned, namespaced events with validated payload schemas.

Representative events:

```text
evidence.prepared
evidence.requested
evidence.expanded
evidence.denied

review.pass
review.suggestion
review.candidate_found
review.unable_to_verify
review.failed

verification.corroborated
verification.rejected
verification.disputed
verification.inconclusive

escalation.preconfigured
escalation.requested
escalation.not_authorized

human.required
human.not_required
human.confirmed
human.rejected
human.modified

outcome.recorded
```

Payloads SHOULD contain stable references, hashes, classifications, and bounded summaries. Every event schema and workflow document MUST carry a version. Undeclared or invalid model output becomes a failed attempt or retryable validation error rather than an accepted transition.

### 44.3.8 Persistence and Recovery

Langchart checkpoints are stored beneath `.argus/state/` and answer: "Where should this workflow resume?" Argus audit records answer: "What happened, what evidence supported it, and which result is effective?"

```text
.argus/state/review.redb
    Argus scheduler, work items, indexes, effective state

.argus/state/langchart-checkpoints.redb
    Resumable Langchart execution snapshots

.argus/reviews/<run-id>/
    Portable finalized assessments, events, adjudications, and provenance
```

The two logical stores MAY share one physical `redb` database if tables and ownership are cleanly separated. They MUST remain separate logical schemas.

Recovery requires reconstructing the exact workflow document, actor set, provider policy, and evidence revision. The working state therefore records their stable identities and versions. A checkpoint whose workflow or actor version cannot be reconstructed MUST fail visibly rather than resume under newer behavior.

The exact workflow document is stored by content hash in `.argus/state/` for the duration of the run and exported with the finalized review bundle. Actor implementations record a stable actor type and semantic version. Upgrades that cannot reconstruct a compatible actor MUST require an explicit migration or restart of affected work; they MUST NOT silently resume using changed actor behavior.

Langchart's checkpoint store may retain only the latest execution snapshot for each workflow run because checkpoints are ephemeral recovery state. Historical attempts and adjudications are retained by Argus and exported to the portable review bundle.

### 44.3.9 Concurrency and Admission Control

Argus keeps pending work as disk-backed records. It admits only a bounded number of Langchart runs according to provider capacity, memory limits, review significance, and human-review pressure.

Suspended workflows MAY be checkpointed and evicted from memory. A workflow awaiting human action or unavailable provider capacity SHOULD NOT retain a live Tokio task indefinitely when it can be recovered later.

This requires a Langchart `hibernate` operation that atomically checkpoints a non-terminal run, stops its task, and removes it from the live-run registry without changing its semantic status to cancelled. Recovery recreates the run from the stored workflow, actors, and checkpoint. Until Langchart provides this operation, Argus MUST bound dormant live runs or use a runtime-lifecycle workaround that is explicitly tested for lost events and duplicate execution.

Langchart parallel states are appropriate for a bounded number of independent verification branches within one work item. Repository-wide parallelism belongs to the Argus scheduler.

### 44.3.10 Failure and Idempotency

Workflow retries MUST distinguish transient provider failure, invalid structured output, evidence unavailability, policy denial, and permanent review failure.

Outcome recording uses a logical result key consisting of the audit snapshot, audit run, work ID, policy version, evidence revision, and workflow hash. Attempt number identifies execution history but MUST NOT be part of the logical result identity.

```text
logical_result_key =
    audit_snapshot
    + audit_run
    + work_id
    + policy_version
    + evidence_revision
    + workflow_hash

attempt_key = logical_result_key + attempt_number
```

`OutcomeRecorderActor` uses a durable Argus inbox to conditionally commit or retrieve the result for the logical key. After a crash, replay first queries that inbox; an existing commit returns the same result reference without creating another assessment or finding. The workflow then records the returned reference and checkpoints its transition.

Argus and Langchart stores are not assumed to share an atomic transaction. The inbox protocol MUST tolerate a crash before or after either write. Replaying a checkpoint MUST NOT create duplicate findings or assessments, and an Argus result that was committed before a Langchart checkpoint MUST remain discoverable and attachable after recovery.

Langchart's current in-memory external-call outbox is not sufficient for durable GitHub, Beads, or other publication guarantees. Argus publishers own durable idempotency keys and publication receipts. A Langchart workflow may request publication, but the publisher subsystem performs and records the external mutation.

### 44.3.11 Workflow Versioning and Testing

Built-in workflow documents are versioned artifacts. A finalized assessment records:

```text
Workflow ID and version
Workflow document hash
Actor identities and versions
Policy and prompt versions
Model and provider identities
Evidence revision
Langchart runtime version
```

Every built-in workflow SHOULD have deterministic simulation tests covering success, evidence expansion, invalid output, retry exhaustion, provider failure, verification disagreement, unauthorized escalation, human adjudication, suspension, recovery, and idempotent replay.

Trace replay tests SHOULD be used to detect unintended behavior changes when workflow definitions, actors, or Langchart versions change.

---

# 45. Suggested CLI

Example:

The CLI should expose repository setup, audit lifecycle, verification, reporting, and publication as distinct operations.

```text
argus init
argus config validate
argus prime
argus status
argus audit
argus resume
argus cancel
argus coverage
argus findings list
argus findings show <id>
argus verify <id|selector>
argus adjudicate <id>
argus report
argus publish
argus baseline
argus extensions
argus providers
argus finalize
argus clean
```

Examples:

```bash
argus audit --pipeline full
argus audit --pipeline ci --base-ref main
argus audit --policy documentation
argus audit --target storage::Transaction::commit
argus resume RUN-1234

argus verify FINDING-1234 --pipeline verify-frontier
argus report RUN-1234 --format html
argus publish RUN-1234 --publisher github --dry-run
argus publish RUN-1234 --publisher beads --selector "severity >= major"

argus finalize RUN-1234
argus clean --state
```

`clean` MUST remove only explicitly selected working state. It MUST NOT remove finalized reviews unless separately and explicitly requested.

Commands SHOULD support stable machine-readable operation through `--format text|json|jsonl`, `--quiet`, and `--no-color`.

Audit execution, report finalization, human adjudication, and publication are separate lifecycle milestones. An audit may be finalized with unresolved or unadjudicated findings as long as their states remain explicit. CI MUST NOT wait for interactive adjudication; configured gates may use deterministic failures, coverage completeness, candidate maturity, verification state, severity, and policy.

## 45.1 Repository Initialization and Priming

`argus init` configures a repository. It SHOULD:

1. Detect repository languages and build systems.
2. Propose applicable built-in extensions.
3. Detect likely generated, vendored, and external paths.
4. Create `.argus/config/` with default full and CI pipelines.
5. Generate proposed exclusions without silently accepting them.
6. Select review storage behavior.
7. Create `.argus/.gitignore` idempotently.
8. Validate provider references without storing credentials.
9. Report every file it creates or modifies.

Initialization MUST preserve unfamiliar files and MUST NOT overwrite existing configuration without an explicit migration or force operation. It does not run a full audit.

`argus prime` prepares resumable working review state. It creates `.argus/state/`, initializes `redb`, inventories the repository, validates extensions and providers, estimates target counts, and reports configuration gaps.

```text
init     Configure the repository
prime    Prepare working review state
audit    Perform evaluation
finalize Export a portable review bundle
```

## 45.2 MCP Surface

MCP exposes the same domain operations as the CLI. Long-running operations use a job-oriented lifecycle rather than holding one request open.

Representative read-only resources or tools:

```text
get_repository_configuration
get_run
get_run_status
list_runs
get_coverage
list_targets
get_target
list_findings
get_finding
get_evidence
get_report
list_extensions
list_providers
```

Representative mutating tools:

```text
initialize_repository
prime_repository
start_audit
resume_audit
cancel_audit
start_verification
record_adjudication
generate_report
publish_findings
update_baseline
finalize_review
```

Long-running tools return a run or job identifier that clients can poll or subscribe to. Publishing, online-model transmission, and adjudication MUST remain explicit mutating operations and MUST NOT be hidden inside read operations.

### 45.2.1 MCP Authorization and Repository Scope

Every MCP server instance MUST declare whether it manages one repository or multiple repositories. Each request resolves to an explicit canonical repository root and audit snapshot; path traversal or implicit current-directory switching is prohibited.

MCP callers have authenticated identities and operation-scoped authorization. At minimum, permissions distinguish:

```text
Read configuration and reports
Read source-derived evidence
Start or cancel audits
Transmit content to online providers
Record human adjudication
Modify exclusions or baselines
Publish or close external issues
Resolve secrets
```

Mutating requests include an expected configuration or run revision to prevent stale clients from overwriting newer decisions. Concurrent adjudication or baseline updates use compare-and-swap semantics and return conflicts explicitly.

Every mutation records caller identity, timestamp, repository and snapshot, request parameters after redaction, authorization basis, result, and affected record identities. Secrets are resolved only inside the capability that needs them and are never returned through MCP resources or persisted in audit events.

Deployments MAY add interactive approval for sensitive operations. Preconfigured authorization for online providers or publishers must remain limited to the named repository scope, data classifications, destinations, and pipeline.

---

# 46. Configuration

Example conceptual configuration:

```toml
[review]
language = "rust"

[review.snapshot]
dirty_tree = "capture"

[[review.rust.configurations]]
name = "default-linux"
target = "x86_64-unknown-linux-gnu"
profile = "dev"
features = ["default"]

[review.storage]
mode = "artifact"

[review.coverage]
include_private = true
exclude_generated = true
exclude_vendor = true

[review.policies]
documentation = true
correctness = true
maintainability = true
performance = true
architecture = true
testing = true
design_consistency = true

[review.performance]
require_evidence_for_major = true

[review.design]
paths = [
    "docs/adr",
    "docs/design",
    "docs/prd"
]

[review.ci]
mode = "automated"
fail_on = [
    "critical",
    "major:correctness"
]

[review.verification]
candidate_reviews = 2
high_significance_primary_reviews = 2
sample_pass_rate = 0.02

[review.verification.escalation]
pipeline = "verify-frontier"
minimum_severity = "major"
on_disagreement = true

[review.providers.local]
adapter = "openai-compatible"
endpoint_env = "ARGUS_LOCAL_MODEL_ENDPOINT"

[review.providers.frontier]
adapter = "configured-online-provider"
credential_env = "ARGUS_FRONTIER_API_KEY"

[review.publishers.github]
repository = "owner/repository"
credential_env = "ARGUS_GITHUB_TOKEN"
```

Configuration SHOULD support focused audits, complete audits, and CI-lite audits as named pipelines. CI-lite reduces scope to changed and impacted targets; it MUST NOT silently lower evidence or verification standards.

---

# 47. SARIF Integration

The report layer SHOULD support SARIF.

This allows findings to appear naturally in:

- GitHub code scanning,
- IDE integrations,
- CI systems,
- and other static-analysis consumers.

The richer JSON review database remains the source of truth, while SARIF serves as an interoperability format.

---

# 48. Review Cost Management

Model-assisted review of every declaration can become expensive.

The system should use several controls.

## Deterministic Filtering

Do not invoke semantic reviewers for checks already conclusively resolved by deterministic analysis.

## Importance-Aware Review Depth

Critical targets receive more evidence and deeper review.

Low-importance trivial targets receive lightweight review.

## Result Caching

Reuse unchanged reviews.

## Evidence Budgeting

Limit context expansion.

## Model Tiering

Simple classification may use a smaller model.

Complex correctness or architecture review may use a stronger model.

## Batch Scheduling

Independent low-complexity targets may be safely grouped where coverage remains explicit.

---

## 48.1 Review Quality Evaluation and Calibration

Recall, precision, stability, and audit defensibility are primary product outcomes and MUST be measured independently of ordinary repository audits.

The project should maintain versioned evaluation corpora containing:

```text
Known-clean artifacts
Seeded documentation defects
Seeded correctness and boundary defects
Relational and architectural defects
Concurrency and persistence defects
Ambiguous cases requiring UnableToVerify
Adversarial comments and prompt-injection content
Large-repository and long-context cases
```

Ground truth is represented as adjudicated expected issues, acceptable alternative formulations, expected non-findings, applicable severity ranges, and evidence requirements. Evaluation data MUST distinguish a model's failure to find an issue from the inventory or evidence system's failure to expose the necessary material.

Required measurements include:

```text
Finding precision
Defect recall
Severity calibration
Confidence calibration
Duplicate-finding rate
Unable-to-verify rate
Human rejection and modification rate
Repeated-run stability
Model/provider comparison
Evidence-request efficiency
Coverage-accounting correctness
```

Metrics SHOULD be segmented by language, target kind, policy, significance, defect category, model/provider, and evidence budget. Aggregate numbers alone can hide severe regressions in rare but critical categories.

Prompt, policy, actor, workflow, model, provider, evidence-builder, or toolchain changes MUST run applicable regression evaluations before becoming the default. Thresholds and permitted regressions are configuration and release-policy decisions.

Verification disagreement is an operational signal, not ground truth. Human-adjudicated evaluation data is required to measure whether independent or frontier verification actually improves precision and recall.

Large-scale evaluation SHOULD include scheduler recovery, checkpoint replay, conservative invalidation, stable finding identity, and deterministic bundle finalization in addition to semantic review quality.

---

# 49. Avoiding Review Noise

A successful harness should optimize for engineering value, not finding count.

The policies should suppress:

- purely aesthetic preferences,
- comments that simply restate names,
- speculative performance micro-optimizations,
- generic “add more tests” findings,
- vague “this is complex” findings,
- recommendations without evidence,
- artificial documentation requirements on obvious private helpers.

A finding should answer:

```text
What is wrong?
Where is it?
Why does it matter?
What evidence supports the conclusion?
How confident are we?
What should an engineer consider doing?
```

---

# 50. Implementation Phases

## Phase 1 — Core Inventory and Review Framework

Implement:

- project abstraction,
- immutable source acquisition and content-addressed source slices,
- portable target classification and adapter capability records,
- immutable audit snapshot and analysis-configuration matrix,
- target model,
- source locations,
- relationships,
- review policies,
- findings,
- orthogonal inventory, execution, assessment, verification, and adjudication states,
- coverage,
- `.argus/config`, `.argus/state`, and `.argus/reviews` lifecycle,
- resumable `redb` working state,
- portable bundle schema,
- audit work-item and attempt model,
- Langchart workflow identity and checkpoint metadata,
- `argus init`, `argus prime`, and CLI skeleton.

Deliverable:

A harness capable of initializing a repository, capturing an immutable snapshot, exercising the inventory and coverage framework with synthetic or adapter-supplied targets, resuming interrupted inventory work, and finalizing a minimal portable text-based review bundle. Semantic target coverage is not claimed until a language adapter has successfully applied its declared discovery contract.

---

## Phase 2 — Rust Semantic Adapter

Implement:

- Cargo workspace discovery,
- crate discovery,
- module discovery,
- declaration inventory,
- rustdoc JSON ingestion,
- source spans,
- visibility,
- parent-child relationships,
- trait/impl relationships,
- basic call/type relationships where available,
- capability-qualified discovery diagnostics,
- minimal module, crate, and workspace dependency graph construction.

Deliverable:

A capability-qualified semantic inventory of an entire Rust workspace for every declared configuration in the initial audit matrix, with conditional and configuration-specific targets, unsupported artifact classes, partial relationships, and discovery failures identified explicitly.

---

## Phase 3 — Deterministic Analysis

Integrate:

- the compiled-in and subprocess extension contracts,
- active evidence production and external evidence ingestion,
- cargo check,
- Clippy,
- cargo test,
- doctests,
- documentation linting,
- TODO/FIXME/stub detection,
- unsafe documentation checks,
- basic complexity metrics.

Deliverable:

Machine-readable deterministic evidence associated with semantic targets through the common extension and provenance model.

---

## Phase 4 — Evidence and Context Engine

Implement:

- local and online model provider adapters,
- provider capability negotiation and fault handling,
- evidence repository,
- target evidence packages,
- source retrieval,
- caller/callee retrieval,
- type retrieval,
- test retrieval,
- context budgeting,
- progressive evidence requests,
- `argus-langchart` integration crate,
- versioned target-review workflow,
- evidence-reference workflow data,
- deterministic workflow simulation,
- checkpoint recovery and transactional idempotent outcome recording,
- required Langchart hibernation or a validated bounded-runtime alternative,
- regular durable progress flushing, stall detection, and detailed status reporting.

Deliverable:

A reviewer can inspect one target without receiving the complete repository, request bounded additional evidence, suspend, recover, and commit one effective outcome through a versioned Langchart workflow.

---

## Phase 5 — Documentation Review Policy

Implement:

- configurable verification and escalation pipelines,
- per-finding verification and adjudication work items,
- documentation rubric,
- claim extraction,
- claim verification,
- completeness analysis,
- stale documentation detection,
- usefulness assessment,
- per-target documentation review,
- seeded documentation evaluation corpus,
- precision, recall, calibration, and repeated-run evaluation.

Deliverable:

A complete documentation-quality audit with provable coverage, independently measured precision and recall, per-finding verification, human-review states, and local-model support.

---

## Phase 6 — Correctness Review Policy

Implement analysis for:

- logical defects,
- edge cases,
- state transitions,
- error handling,
- resource lifecycle,
- concurrency,
- persistence,
- unsafe assumptions,
- consistency with tests.

Deliverable:

Evidence-backed correctness findings.

---

## Phase 7 — Initial Architecture Review

Implement:

- module, crate/package, and workspace aggregate reviews,
- dependency and responsibility synthesis,
- cross-crate relationship review,
- public API and boundary review,
- cross-cutting pattern detection,
- explicit visibility of failed, partial, and unable-to-verify constituents.

Deliverable:

Code-derived architectural understanding grounded in complete workspace inventory and lower-level assessments.

---

## Phase 8 — Maintainability Review Policy

Implement:

- complexity analysis,
- coupling analysis,
- responsibility analysis,
- duplication identification,
- abstraction review,
- code-smell detection,
- model-assisted maintainability evaluation.

Deliverable:

Maintainability findings with architectural context.

---

## Phase 9 — Performance Review Policy

Implement:

- algorithmic complexity heuristics,
- allocation/copying review,
- hot-path identification,
- locking review,
- I/O pattern review,
- async blocking detection,
- benchmark/profiling evidence ingestion.

Deliverable:

Conservative, evidence-driven performance findings.

---

## Phase 10 — Design Document Index

Implement:

- document discovery,
- Markdown section parsing,
- FTS index,
- optional embeddings,
- metadata extraction,
- ADR/PRD/design classification,
- symbol-to-document linking,
- semantic retrieval.

Deliverable:

Relevant engineering design can be progressively retrieved during code review.

---

## Phase 11 — Design-Conformance Architecture Review

Implement aggregate review at:

- module,
- crate/package,
- subsystem,
- workspace.

Add:

- dependency analysis,
- cycle detection,
- layering rules,
- responsibility analysis,
- cross-cutting pattern detection,
- design conformance.

Deliverable:

An application-level architectural review rather than only declaration-level findings.

---

## Phase 12 — Incremental Review

Implement:

- fingerprints,
- dependency invalidation,
- cached reviews,
- changed-target detection,
- impacted-target analysis,
- baseline handling.

Deliverable:

Efficient CI and developer workflows.

---

## Phase 13 — Extended Reporting, Publishing, and CI

Implement:

- Markdown report,
- JSON results,
- JSONL review archive,
- HTML,
- SARIF,
- CI summary,
- severity gates,
- baseline comparison,
- trend metrics,
- renderer and publisher extensions,
- GitHub and Beads issue publishing where configured,
- publication receipts and idempotency,
- MCP resources and asynchronous tools,
- MCP repository scoping, authorization, concurrency control, and mutation audit logging.

Deliverable:

Production-ready integration into local CLI, CI/CD, scheduled audit, MCP, and external issue-management workflows.

---

# 51. Initial End-to-End Milestone

A useful first milestone must be repository-wide because single-package coverage does not validate actual end-to-end use. It is constrained by language and policy depth rather than by reviewing only part of a workspace.

## Scope

Argus is implemented in Rust. The first review adapter targets entire Rust repositories and Cargo workspaces, including all discovered crates under the declared configuration. Argus itself is the dogfooding repository; Mnemosyne is the larger scale, recovery, and cross-crate validation repository.

Policies:

```text
Documentation
Correctness
Architecture
```

Targets:

```text
Crate
Module
Type
Function / Method
Trait
Impl
Test
```

Evidence:

```text
Source
Documentation
Compiler
Clippy
Tests
Direct dependencies
Direct callers
```

Output:

```text
Markdown
HTML
JSON
Coverage Report
Portable Review Bundle
```

Execution:

```text
Local model provider required
Optional configured frontier escalation
Resumable redb working state
Human review queue
CLI lifecycle from init through finalize
Versioned Langchart target-review workflow
Simulation and recovery tests
Immutable audit snapshot
Per-finding verification workflow
Seeded quality evaluation and calibration
Detailed progress, health, cost, and completion estimates
Explicit execution, finalization, adjudication, and publication states
```

This milestone validates exhaustive target decomposition, bounded evidence review, code-derived architectural synthesis, long-running recovery, and developer-report usefulness across both a dogfooding repository and a larger multi-crate repository. Correctness, documentation, and architecture remain independently selectable. Design-document conformance, maintainability depth, performance, advanced incremental analysis, additional language adapters, and external publishing may follow without weakening the initial repository-wide coverage contract.

---

# 52. Recommended Next Milestone

After the initial milestone, design-document consistency should be added to the existing code-derived architecture review.

Architecture review without design context tends to infer intent.

Design review without semantic architecture lacks grounding.

The combination allows questions such as:

```text
Does the code implement the declared architecture?

Has an architectural boundary eroded?

Does a module now own behavior assigned elsewhere?

Does an ADR remain true?

Has implementation behavior drifted without documentation being updated?
```

These provide significantly more value than isolated style review.

---

# 53. Future Extensions

The architecture naturally permits additional policies.

Potential future auditors include:

```text
SecurityAuditor
ConcurrencyAuditor
UnsafeCodeAuditor
DatabaseCorrectnessAuditor
APICompatibilityAuditor
DependencyAuditor
LicensingAuditor
OperationalReadinessAuditor
ObservabilityAuditor
AccessibilityAuditor
ProtocolCompatibilityAuditor
SerializationCompatibilityAuditor
```

Language adapters could later support:

```text
TypeScript
Python
Java
Go
C++
C#
```

---

# 54. Long-Term Model

The long-term system should be viewed as a **Source Intelligence and Audit Platform**.

Its reusable foundation is:

```text
Semantic Repository Model
        +
Symbol / Dependency Graph
        +
Design Knowledge Index
        +
Evidence Retrieval
        +
Progressive Context
        +
Review State Machine
        +
Independent Audit Policies
        +
Coverage Accounting
        +
Incremental Invalidation
        +
Structured Findings
```

Documentation review, code review, architecture review, security review, and other analyses become different interpretations of the same source model.

This is preferable to creating separate agents that independently rediscover the repository for every concern.

The harness should understand the application once and permit many specialized reviewers to reason over that shared model.

---

# 55. Key Architectural Decisions

The implementation should preserve the following principles:

1. **Inventory is deterministic.**  
   AI does not decide what gets reviewed.

2. **Coverage is explicit.**  
   Every target-policy pair exposes separate inventory, execution, evidence-adequacy, verification, and adjudication state.

3. **Evidence is bounded and progressive.**  
   Reviewers see what they need and can request more.

4. **Reviews are specialized.**  
   Documentation, correctness, maintainability, performance, and architecture use different rubrics.

5. **Evidence has provenance.**  
   Findings must be traceable to source, tests, design, or analysis.

6. **Disagreement is reported, not silently resolved.**  
   Code, tests, documentation, and design may conflict.

7. **Review depth is proportional to significance.**

8. **Static tooling precedes model reasoning.**

9. **Aggregate review is required.**  
   Architecture and systemic problems cannot be understood solely from individual functions.

10. **The harness is incremental.**  
    Repository-wide intelligence should be reused rather than reconstructed on every run.

11. **The core is language-agnostic.**  
    Rust is the first adapter, not a permanent architectural limitation.

12. **The initial system reviews rather than fixes.**  
    Analysis remains distinguishable from remediation.

13. **Model review is advisory and configurable.**  
    Candidate findings may be independently verified, escalated across provider tiers, and presented for human adjudication.

14. **Local and online models are first-class.**  
    Provider capability and failure behavior are recorded, and online transmission requires run configuration or explicit follow-up.

15. **Working state and audit artifacts are separate.**  
    `redb` supports resumable work under `.argus/state`, while finalized reviews use portable text formats under `.argus/reviews` or an external destination.

16. **Extensions have constrained capabilities.**  
    Discovery, evidence production, review, verification, rendering, publishing, and importing use explicit contracts.

17. **Rendering is separate from publishing.**  
    Reports are derived views; external mutations are explicit, idempotent, and receipt-producing.

18. **One repository namespace is used.**  
    Durable configuration, ephemeral state, and finalized reviews live beneath `.argus/config`, `.argus/state`, and `.argus/reviews` respectively.

19. **Langchart orchestrates bounded review workflows.**  
    Argus owns repository-scale scheduling and the durable audit truth; Langchart owns state transitions for individual target, relationship, aggregate, verification, and adjudication workflows.

20. **Workflow state carries references rather than evidence blobs.**  
    Large source, evidence, assessment, and transcript data remains in Argus stores and is addressed by stable identity and content hash.

21. **Repository-wide parallelism is not a statechart.**  
    Argus admits bounded Langchart runs from a disk-backed queue; Langchart parallel states are reserved for bounded branches within one work item.

22. **Every audit is bound to an immutable snapshot.**  
    Inventory and evidence are complete only relative to a recorded source, dependency, toolchain, and analysis-configuration matrix.

23. **Assessment and verification are separate work.**  
    Target assessment may produce multiple canonical findings, each of which receives independent verification and adjudication state.

24. **Outcome recording is recovery-safe.**  
    A durable Argus inbox and logical result key prevent retries or checkpoint replay from creating duplicate effective findings.

25. **Review quality is measured.**  
    Seeded, human-adjudicated evaluation corpora track precision, recall, calibration, stability, and regressions across policies and providers.

---

# 56. Definition of Success

The project should be considered successful when it can take a non-trivial application and produce a defensible statement such as:

> For audit snapshot `SNAPSHOT-1234` and its declared configuration matrix, the workspace contains 12,418 reviewable semantic artifacts and 48,092 applicable target-policy pairs. Of these, 47,950 completed execution, 47,801 had adequate evidence, 149 were unable to verify, 18 failed, and 124 remain pending. All 302 candidate findings retain their original assessments; 287 completed independent verification, 15 remain disputed or pending, and 41 of 47 required human adjudications are complete. The active audit contains 3 Critical, 21 Major, 84 Moderate, and 193 Minor findings. Every result identifies its source snapshot, policy, workflow, evidence, model/provider attempts, verification, adjudication, and provenance.

That is substantially different from asking an AI system to “review the repository.”

It creates an auditable engineering process with measurable coverage, reproducible evidence, iterative analysis, and a foundation that can grow into a general-purpose software quality and architecture intelligence system.
