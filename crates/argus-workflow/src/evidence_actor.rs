use crate::{
    EvidenceExpansionRecord, EvidenceRequestDecision, EvidenceRequestDisposition,
    WorkflowDataRecord, WorkflowDataStore, WorkflowDataWrite,
};
use argus_core::{ArgusError, EvidenceKind, TargetId};
use argus_evidence::{
    AuthorizedEvidenceExpansion, EVIDENCE_SCHEMA_VERSION, EvidenceBudget, EvidenceDisposition,
    EvidenceExpansionPolicy, EvidenceRequest, EvidenceRequestAuthorizer, ExpansionDenialReason,
    ExpansionUsage, PackageArtifact,
};
use async_trait::async_trait;
use langchart_runtime::{
    AgentActor, AgentError, AgentInvocation, AgentOutputEvent, CapabilityBroker, CapabilityEnvelope,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceRequestDraft {
    requested_targets: BTreeSet<TargetId>,
    requested_kinds: BTreeSet<EvidenceKind>,
    additional_budget: EvidenceBudgetDraft,
    rationale: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceBudgetDraft {
    #[serde(rename = "max_bytes")]
    bytes: usize,
    #[serde(rename = "max_tokens")]
    tokens: usize,
    #[serde(rename = "max_items")]
    items: usize,
    #[serde(rename = "max_relation_depth")]
    relation_depth: u32,
}

impl From<EvidenceBudgetDraft> for EvidenceBudget {
    fn from(value: EvidenceBudgetDraft) -> Self {
        Self {
            max_bytes: value.bytes,
            max_tokens: value.tokens,
            max_items: value.items,
            max_relation_depth: value.relation_depth,
        }
    }
}

pub(crate) fn parse_evidence_request_draft(value: &Value) -> Result<EvidenceRequestDraft, String> {
    let draft: EvidenceRequestDraft =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    if draft.requested_targets.is_empty()
        || draft.requested_kinds.is_empty()
        || draft.rationale.trim().is_empty()
        || draft.rationale.trim() != draft.rationale
        || draft.additional_budget.bytes == 0
        || draft.additional_budget.tokens == 0
        || draft.additional_budget.items == 0
    {
        return Err(
            "evidence request requires normalized rationale, targets, kinds, and positive limits"
                .to_owned(),
        );
    }
    Ok(draft)
}

pub struct EvidenceRequestEvaluatorActor {
    workflow_data: Arc<WorkflowDataStore>,
    base_package: PackageArtifact,
    policy: EvidenceExpansionPolicy,
}

#[async_trait]
pub trait EvidenceExpander: Send + Sync {
    /// Materializes the authorized revision. Implementations must be idempotent by
    /// `authorization.authorization_hash` because a crash may repeat this call.
    async fn expand(
        &self,
        base_package: &PackageArtifact,
        authorization: &AuthorizedEvidenceExpansion,
    ) -> Result<PackageArtifact, ArgusError>;
}

pub struct EvidenceExpansionActor {
    workflow_data: Arc<WorkflowDataStore>,
    base_package: PackageArtifact,
    expander: Arc<dyn EvidenceExpander>,
}

impl EvidenceExpansionActor {
    #[must_use]
    pub const fn new(
        workflow_data: Arc<WorkflowDataStore>,
        base_package: PackageArtifact,
        expander: Arc<dyn EvidenceExpander>,
    ) -> Self {
        Self {
            workflow_data,
            base_package,
            expander,
        }
    }

    async fn expand(
        &self,
        run_id: &str,
        declared_events: &[String],
    ) -> Result<AgentOutputEvent, AgentError> {
        let record = load(self.workflow_data.clone(), run_id).await?;
        let authorization = latest_authorization(&record)?.clone();
        if let Some(expansion) = record
            .data
            .evidence_expansions
            .iter()
            .find(|item| item.authorization_hash == authorization.authorization_hash)
        {
            return expansion_event(expansion, declared_events);
        }
        validate_active_package(&record, &self.base_package)?;
        if authorization.request.base_package != self.base_package.hash
            || record.data.evidence_revision
                != record
                    .data
                    .evidence_request_decisions
                    .last()
                    .map_or(0, |decision| decision.evidence_revision)
        {
            return Err(AgentError::Internal(
                "evidence authorization does not bind the active package revision".to_owned(),
            ));
        }
        let expanded = self
            .expander
            .expand(&self.base_package, &authorization)
            .await
            .map_err(|error| AgentError::Internal(error.to_string()))?;
        validate_expanded_package(&self.base_package, &authorization, &expanded)?;
        let to_revision = record
            .data
            .evidence_revision
            .checked_add(1)
            .ok_or_else(|| AgentError::Internal("evidence revision overflow".to_owned()))?;
        let expansion = EvidenceExpansionRecord {
            authorization_hash: authorization.authorization_hash.clone(),
            from_revision: record.data.evidence_revision,
            previous_package: self.base_package.hash.clone(),
            to_revision,
            package_ref: expanded.hash,
        };
        let mut proposed = record.data;
        proposed.evidence_revision = to_revision;
        proposed.evidence_package_ref = expansion.package_ref.as_str().to_owned();
        proposed.evidence_expansion_count = proposed.evidence_expansion_count.saturating_add(1);
        proposed.evidence_expansions.push(expansion);
        let effective = compare_and_swap(
            self.workflow_data.clone(),
            run_id,
            record.revision,
            proposed,
        )
        .await?;
        let expansion = effective
            .data
            .evidence_expansions
            .iter()
            .find(|item| item.authorization_hash == authorization.authorization_hash)
            .ok_or_else(|| {
                AgentError::Internal("durable evidence expansion is missing".to_owned())
            })?;
        expansion_event(expansion, declared_events)
    }
}

#[async_trait]
impl AgentActor for EvidenceExpansionActor {
    async fn run(
        &self,
        invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        self.expand(invocation.run_id.as_ref(), &invocation.output_event_types)
            .await
    }
}

impl EvidenceRequestEvaluatorActor {
    #[must_use]
    pub const fn new(
        workflow_data: Arc<WorkflowDataStore>,
        base_package: PackageArtifact,
        policy: EvidenceExpansionPolicy,
    ) -> Self {
        Self {
            workflow_data,
            base_package,
            policy,
        }
    }

    async fn evaluate(
        &self,
        run_id: &str,
        declared_events: &[String],
    ) -> Result<AgentOutputEvent, AgentError> {
        let record = load(self.workflow_data.clone(), run_id).await?;
        if let Some(decision) = current_request_decision(&record) {
            return decision_event(decision, declared_events);
        }
        validate_active_package(&record, &self.base_package)?;
        let primary = record
            .data
            .primary_decisions
            .last()
            .filter(|decision| decision.evidence_revision == record.data.evidence_revision)
            .ok_or_else(|| {
                AgentError::Internal("current primary decision is missing".to_owned())
            })?;
        if primary.event_type != "review.unable_to_verify" {
            return Err(AgentError::Internal(
                "evidence evaluator requires an unable-to-verify decision".to_owned(),
            ));
        }
        let draft = primary
            .payload
            .get("requested_evidence")
            .ok_or_else(|| AgentError::Internal("evidence request draft is missing".to_owned()))
            .and_then(|value| parse_evidence_request_draft(value).map_err(AgentError::Internal))?;
        let usage = current_usage(&record);
        let request = EvidenceRequest {
            sequence: usage.approved_requests.saturating_add(1),
            base_package: self.base_package.hash.clone(),
            requested_targets: draft.requested_targets,
            requested_kinds: draft.requested_kinds,
            additional_budget: draft.additional_budget.into(),
            rationale: draft.rationale,
        };
        let disposition = match EvidenceRequestAuthorizer::authorize(
            &self.base_package,
            request.clone(),
            &self.policy,
            &usage,
        ) {
            Ok(authorization) => EvidenceRequestDisposition::Allowed { authorization },
            Err(denial) => EvidenceRequestDisposition::Denied { denial },
        };
        let mut proposed = record.data;
        proposed
            .evidence_request_decisions
            .push(EvidenceRequestDecision {
                evidence_revision: proposed.evidence_revision,
                request,
                disposition,
            });
        let effective = compare_and_swap(
            self.workflow_data.clone(),
            run_id,
            record.revision,
            proposed,
        )
        .await?;
        let decision = current_request_decision(&effective).ok_or_else(|| {
            AgentError::Internal("durable evidence request decision is missing".to_owned())
        })?;
        decision_event(decision, declared_events)
    }
}

#[async_trait]
impl AgentActor for EvidenceRequestEvaluatorActor {
    async fn run(
        &self,
        invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        self.evaluate(invocation.run_id.as_ref(), &invocation.output_event_types)
            .await
    }
}

fn current_request_decision(record: &WorkflowDataRecord) -> Option<&EvidenceRequestDecision> {
    record
        .data
        .evidence_request_decisions
        .last()
        .filter(|decision| decision.evidence_revision == record.data.evidence_revision)
}

fn latest_authorization(
    record: &WorkflowDataRecord,
) -> Result<&AuthorizedEvidenceExpansion, AgentError> {
    let decision = record
        .data
        .evidence_request_decisions
        .last()
        .ok_or_else(|| AgentError::Internal("evidence request decision is missing".to_owned()))?;
    match &decision.disposition {
        EvidenceRequestDisposition::Allowed { authorization } => Ok(authorization),
        EvidenceRequestDisposition::Denied { .. } => Err(AgentError::Internal(
            "denied evidence request cannot be expanded".to_owned(),
        )),
    }
}

fn validate_active_package(
    record: &WorkflowDataRecord,
    base_package: &PackageArtifact,
) -> Result<(), AgentError> {
    base_package
        .validate_identity()
        .map_err(|error| AgentError::Internal(error.to_string()))?;
    if record.data.evidence_package_ref != base_package.hash.as_str()
        || record.data.evidence_revision != base_package.package.revision
        || record.data.policy_id != base_package.package.policy
    {
        return Err(AgentError::Internal(
            "active evidence package identity does not match workflow data".to_owned(),
        ));
    }
    Ok(())
}

fn validate_expanded_package(
    base: &PackageArtifact,
    authorization: &AuthorizedEvidenceExpansion,
    expanded: &PackageArtifact,
) -> Result<(), AgentError> {
    expanded
        .validate_identity()
        .map_err(|error| AgentError::Internal(error.to_string()))?;
    let expected_revision = base
        .package
        .revision
        .checked_add(1)
        .ok_or_else(|| AgentError::Internal("evidence package revision overflow".to_owned()))?;
    let package = &expanded.package;
    if package.schema_version != EVIDENCE_SCHEMA_VERSION
        || package.revision != expected_revision
        || package.previous_package.as_ref() != Some(&base.hash)
        || package.snapshot != base.package.snapshot
        || package.configuration != base.package.configuration
        || package.target != base.package.target
        || package.policy != base.package.policy
        || package.policy_version != base.package.policy_version
        || package.budget != authorization.request.additional_budget
    {
        return Err(AgentError::Internal(
            "expanded evidence package violates its authorized identity or revision".to_owned(),
        ));
    }
    let included = package
        .items
        .iter()
        .filter(|item| {
            matches!(
                item.disposition,
                EvidenceDisposition::Included
                    | EvidenceDisposition::Summarized
                    | EvidenceDisposition::Partial
            )
        })
        .collect::<Vec<_>>();
    if package.used_bytes > package.budget.max_bytes
        || package.used_tokens > package.budget.max_tokens
        || included.len() > package.budget.max_items
        || included
            .iter()
            .any(|item| item.relation_depth > package.budget.max_relation_depth)
    {
        return Err(AgentError::Internal(
            "expanded evidence package exceeds its authorized budget".to_owned(),
        ));
    }
    Ok(())
}

fn expansion_event(
    expansion: &EvidenceExpansionRecord,
    declared_events: &[String],
) -> Result<AgentOutputEvent, AgentError> {
    const EVENT: &str = "evidence.expanded";
    if !declared_events.iter().any(|declared| declared == EVENT) {
        return Err(AgentError::Internal(format!(
            "evidence expander emitted undeclared event `{EVENT}`"
        )));
    }
    Ok(AgentOutputEvent {
        event_type: EVENT.to_owned(),
        payload: json!({
            "authorization_hash": expansion.authorization_hash,
            "evidence_revision": expansion.to_revision,
            "package_ref": expansion.package_ref,
        }),
    })
}

fn current_usage(record: &WorkflowDataRecord) -> ExpansionUsage {
    record
        .data
        .evidence_request_decisions
        .iter()
        .rev()
        .find_map(|decision| match &decision.disposition {
            EvidenceRequestDisposition::Allowed { authorization } => {
                Some(authorization.next_usage.clone())
            }
            EvidenceRequestDisposition::Denied { .. } => None,
        })
        .unwrap_or_default()
}

fn decision_event(
    decision: &EvidenceRequestDecision,
    declared_events: &[String],
) -> Result<AgentOutputEvent, AgentError> {
    let (event_type, payload) = match &decision.disposition {
        EvidenceRequestDisposition::Allowed { authorization } => (
            "request.allowed",
            json!({"authorization_hash": authorization.authorization_hash}),
        ),
        EvidenceRequestDisposition::Denied { denial } => {
            let event_type = if is_budget_exhaustion(denial.reason) {
                "budget.exhausted"
            } else {
                "request.denied"
            };
            (
                event_type,
                json!({"reason": denial.reason, "detail": denial.detail}),
            )
        }
    };
    if !declared_events
        .iter()
        .any(|declared| declared == event_type)
    {
        return Err(AgentError::Internal(format!(
            "evidence evaluator emitted undeclared event `{event_type}`"
        )));
    }
    Ok(AgentOutputEvent {
        event_type: event_type.to_owned(),
        payload,
    })
}

const fn is_budget_exhaustion(reason: ExpansionDenialReason) -> bool {
    matches!(
        reason,
        ExpansionDenialReason::RequestLimitExhausted
            | ExpansionDenialReason::ByteBudgetExhausted
            | ExpansionDenialReason::TokenBudgetExhausted
            | ExpansionDenialReason::ItemBudgetExhausted
            | ExpansionDenialReason::RelationDepthExhausted
    )
}

async fn load(
    store: Arc<WorkflowDataStore>,
    run_id: &str,
) -> Result<WorkflowDataRecord, AgentError> {
    let run_id = run_id.to_owned();
    tokio::task::spawn_blocking(move || store.load(&run_id))
        .await
        .map_err(|error| AgentError::Internal(format!("workflow data task failed: {error}")))?
        .map_err(|error| AgentError::Internal(error.to_string()))?
        .ok_or_else(|| AgentError::Internal("workflow data record is missing".to_owned()))
}

async fn compare_and_swap(
    store: Arc<WorkflowDataStore>,
    run_id: &str,
    revision: u64,
    proposed: crate::ReviewWorkflowData,
) -> Result<WorkflowDataRecord, AgentError> {
    let run_id = run_id.to_owned();
    let write =
        tokio::task::spawn_blocking(move || store.compare_and_swap(&run_id, revision, proposed))
            .await
            .map_err(|error| AgentError::Internal(format!("workflow data task failed: {error}")))?
            .map_err(|error| AgentError::Internal(error.to_string()))?;
    match write {
        WorkflowDataWrite::Updated(record) | WorkflowDataWrite::Existing(record) => Ok(record),
        WorkflowDataWrite::Inserted(_) => Err(AgentError::Internal(
            "evidence evaluator unexpectedly inserted workflow data".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PrimaryReviewDecision, ReviewWorkflowData};
    use argus_core::{ConfigurationId, ContentHash, PolicyId, SnapshotId, WorkItemId};
    use argus_evidence::{DataClassification, EVIDENCE_SCHEMA_VERSION, EvidencePackage};
    use argus_provider::ProviderIdentity;
    use std::path::Path;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct FixtureExpander {
        artifact: Mutex<PackageArtifact>,
        calls: AtomicUsize,
        fail_first_after_materialization: AtomicBool,
    }

    #[async_trait]
    impl EvidenceExpander for FixtureExpander {
        async fn expand(
            &self,
            _base_package: &PackageArtifact,
            _authorization: &AuthorizedEvidenceExpansion,
        ) -> Result<PackageArtifact, ArgusError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self
                .fail_first_after_materialization
                .swap(false, Ordering::SeqCst)
            {
                return Err(ArgusError::invariant(
                    "injected crash after evidence materialization",
                ));
            }
            Ok(self.artifact.lock().unwrap().clone())
        }
    }

    fn budget(bytes: usize, tokens: usize, items: usize, depth: u32) -> EvidenceBudget {
        EvidenceBudget {
            max_bytes: bytes,
            max_tokens: tokens,
            max_items: items,
            max_relation_depth: depth,
        }
    }

    fn artifact(target: &TargetId, policy: &PolicyId) -> PackageArtifact {
        let package = EvidencePackage {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            revision: 1,
            previous_package: None,
            snapshot: SnapshotId::derive([b"snapshot".as_slice()]),
            configuration: ConfigurationId::derive([b"configuration".as_slice()]),
            target: target.clone(),
            policy: policy.clone(),
            policy_version: "1".to_owned(),
            budget: budget(100, 100, 2, 0),
            used_bytes: 0,
            used_tokens: 0,
            items: Vec::new(),
            unsatisfied_requirements: Vec::new(),
        };
        PackageArtifact {
            hash: ContentHash::digest(&serde_json::to_vec(&package).unwrap()),
            package,
        }
    }

    fn policy(allowed_target: TargetId, max_requests: u32) -> EvidenceExpansionPolicy {
        EvidenceExpansionPolicy {
            max_requests,
            cumulative_budget: budget(1_000, 500, 10, 2),
            allowed_targets: BTreeSet::from([allowed_target]),
            allowed_kinds: BTreeSet::from([EvidenceKind::Source, EvidenceKind::Test]),
            maximum_classification: DataClassification::Sensitive,
        }
    }

    fn expanded(
        base: &PackageArtifact,
        authorization: &AuthorizedEvidenceExpansion,
    ) -> PackageArtifact {
        let package = EvidencePackage {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            revision: base.package.revision + 1,
            previous_package: Some(base.hash.clone()),
            snapshot: base.package.snapshot.clone(),
            configuration: base.package.configuration.clone(),
            target: base.package.target.clone(),
            policy: base.package.policy.clone(),
            policy_version: base.package.policy_version.clone(),
            budget: authorization.request.additional_budget.clone(),
            used_bytes: 0,
            used_tokens: 0,
            items: Vec::new(),
            unsatisfied_requirements: Vec::new(),
        };
        PackageArtifact {
            hash: ContentHash::digest(&serde_json::to_vec(&package).unwrap()),
            package,
        }
    }

    fn store(package: &PackageArtifact, requested_target: &TargetId) -> Arc<WorkflowDataStore> {
        let directory = tempfile::tempdir().unwrap().keep();
        store_at(&directory, package, requested_target)
    }

    fn store_at(
        directory: &Path,
        package: &PackageArtifact,
        requested_target: &TargetId,
    ) -> Arc<WorkflowDataStore> {
        let store = Arc::new(WorkflowDataStore::open(directory).unwrap());
        store
            .create(
                "run-1",
                ReviewWorkflowData {
                    work_id: WorkItemId::derive([b"evidence-request-work".as_slice()]),
                    review_unit_id: "crate:fixture".to_owned(),
                    policy_id: package.package.policy.clone(),
                    evidence_package_ref: package.hash.as_str().to_owned(),
                    evidence_revision: 1,
                    primary_decisions: vec![PrimaryReviewDecision {
                        evidence_revision: 1,
                        event_type: "review.unable_to_verify".to_owned(),
                        payload: json!({
                            "reason": "tests are required",
                            "requested_evidence": {
                                "requested_targets": [requested_target],
                                "requested_kinds": ["test"],
                                "additional_budget": {
                                    "max_bytes": 200,
                                    "max_tokens": 100,
                                    "max_items": 2,
                                    "max_relation_depth": 1
                                },
                                "rationale": "inspect tests exercising the target"
                            }
                        }),
                        provider: ProviderIdentity {
                            provider: "fixture-local".to_owned(),
                            provider_version: "1".to_owned(),
                            model: "reviewer".to_owned(),
                            model_version: "pinned".to_owned(),
                        },
                        request_id: "run-1:primary_review:evidence-1".to_owned(),
                        attempt: 0,
                    }],
                    candidate_findings: Vec::new(),
                    scheduled_verification_work: Vec::new(),
                    verification_results: Vec::new(),
                    evidence_request_decisions: Vec::new(),
                    evidence_expansions: Vec::new(),
                    escalation_count: 0,
                    evidence_expansion_count: 0,
                    adjudication: None,
                },
            )
            .unwrap();
        store
    }

    fn events() -> Vec<String> {
        ["request.allowed", "request.denied", "budget.exhausted"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn authorization_is_durable_replay_safe_and_uses_trusted_identity() {
        let target = TargetId::derive([b"target".as_slice()]);
        let requested = TargetId::derive([b"related".as_slice()]);
        let policy_id = PolicyId::derive([b"policy".as_slice()]);
        let package = artifact(&target, &policy_id);
        let store = store(&package, &requested);
        let actor = EvidenceRequestEvaluatorActor::new(
            store.clone(),
            package.clone(),
            policy(requested, 2),
        );

        assert_eq!(
            actor.evaluate("run-1", &events()).await.unwrap().event_type,
            "request.allowed"
        );
        assert_eq!(
            actor.evaluate("run-1", &events()).await.unwrap().event_type,
            "request.allowed"
        );
        let durable = store.load("run-1").unwrap().unwrap();
        assert_eq!(durable.data.evidence_request_decisions.len(), 1);
        let decision = &durable.data.evidence_request_decisions[0];
        assert_eq!(decision.request.sequence, 1);
        assert_eq!(decision.request.base_package, package.hash);

        let mut forged = durable.data;
        let EvidenceRequestDisposition::Allowed { authorization } =
            &mut forged.evidence_request_decisions[0].disposition
        else {
            unreachable!();
        };
        authorization.next_usage.used_bytes += 1;
        assert!(store.compare_and_swap("run-1", 1, forged).is_err());
    }

    #[tokio::test]
    async fn scope_and_budget_denials_map_to_distinct_workflow_events() {
        let target = TargetId::derive([b"target".as_slice()]);
        let outside = TargetId::derive([b"outside".as_slice()]);
        let policy_id = PolicyId::derive([b"policy".as_slice()]);
        let package = artifact(&target, &policy_id);

        let denied_store = store(&package, &outside);
        let denied = EvidenceRequestEvaluatorActor::new(
            denied_store,
            package.clone(),
            policy(target.clone(), 2),
        );
        assert_eq!(
            denied
                .evaluate("run-1", &events())
                .await
                .unwrap()
                .event_type,
            "request.denied"
        );

        let exhausted_store = store(&package, &target);
        let exhausted =
            EvidenceRequestEvaluatorActor::new(exhausted_store, package, policy(target, 0));
        assert_eq!(
            exhausted
                .evaluate("run-1", &events())
                .await
                .unwrap()
                .event_type,
            "budget.exhausted"
        );
    }

    #[tokio::test]
    async fn expansion_advances_the_package_chain_once_and_replays_without_materializing() {
        let target = TargetId::derive([b"target".as_slice()]);
        let policy_id = PolicyId::derive([b"policy".as_slice()]);
        let package = artifact(&target, &policy_id);
        let store = store(&package, &target);
        EvidenceRequestEvaluatorActor::new(store.clone(), package.clone(), policy(target, 2))
            .evaluate("run-1", &events())
            .await
            .unwrap();
        let authorization = latest_authorization(&store.load("run-1").unwrap().unwrap())
            .unwrap()
            .clone();
        let expander = Arc::new(FixtureExpander {
            artifact: Mutex::new(expanded(&package, &authorization)),
            calls: AtomicUsize::new(0),
            fail_first_after_materialization: AtomicBool::new(false),
        });
        let actor = EvidenceExpansionActor::new(store.clone(), package, expander.clone());

        assert_eq!(
            actor
                .expand("run-1", &["evidence.expanded".to_owned()])
                .await
                .unwrap()
                .event_type,
            "evidence.expanded"
        );
        actor
            .expand("run-1", &["evidence.expanded".to_owned()])
            .await
            .unwrap();

        let durable = store.load("run-1").unwrap().unwrap();
        assert_eq!(durable.data.evidence_revision, 2);
        assert_eq!(durable.data.evidence_expansion_count, 1);
        assert_eq!(durable.data.evidence_expansions.len(), 1);
        assert_eq!(expander.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn crash_after_materialization_retries_authorization_without_duplicate_commit() {
        let target = TargetId::derive([b"target".as_slice()]);
        let policy_id = PolicyId::derive([b"policy".as_slice()]);
        let package = artifact(&target, &policy_id);
        let store = store(&package, &target);
        EvidenceRequestEvaluatorActor::new(store.clone(), package.clone(), policy(target, 2))
            .evaluate("run-1", &events())
            .await
            .unwrap();
        let authorization = latest_authorization(&store.load("run-1").unwrap().unwrap())
            .unwrap()
            .clone();
        let expander = Arc::new(FixtureExpander {
            artifact: Mutex::new(expanded(&package, &authorization)),
            calls: AtomicUsize::new(0),
            fail_first_after_materialization: AtomicBool::new(true),
        });
        let actor = EvidenceExpansionActor::new(store.clone(), package, expander.clone());

        assert!(
            actor
                .expand("run-1", &["evidence.expanded".to_owned()])
                .await
                .is_err()
        );
        assert_eq!(
            store.load("run-1").unwrap().unwrap().data.evidence_revision,
            1
        );
        actor
            .expand("run-1", &["evidence.expanded".to_owned()])
            .await
            .unwrap();
        actor
            .expand("run-1", &["evidence.expanded".to_owned()])
            .await
            .unwrap();

        let durable = store.load("run-1").unwrap().unwrap();
        assert_eq!(durable.data.evidence_expansions.len(), 1);
        assert_eq!(expander.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn expanded_package_cannot_change_trusted_review_identity() {
        let target = TargetId::derive([b"target".as_slice()]);
        let policy_id = PolicyId::derive([b"policy".as_slice()]);
        let package = artifact(&target, &policy_id);
        let store = store(&package, &target);
        EvidenceRequestEvaluatorActor::new(store.clone(), package.clone(), policy(target, 2))
            .evaluate("run-1", &events())
            .await
            .unwrap();
        let authorization = latest_authorization(&store.load("run-1").unwrap().unwrap())
            .unwrap()
            .clone();
        let mut altered = expanded(&package, &authorization);
        altered.package.target = TargetId::derive([b"forged-target".as_slice()]);
        altered.hash = ContentHash::digest(&serde_json::to_vec(&altered.package).unwrap());
        let expander = Arc::new(FixtureExpander {
            artifact: Mutex::new(altered),
            calls: AtomicUsize::new(0),
            fail_first_after_materialization: AtomicBool::new(false),
        });
        let actor = EvidenceExpansionActor::new(store.clone(), package, expander);

        assert!(
            actor
                .expand("run-1", &["evidence.expanded".to_owned()])
                .await
                .is_err()
        );
        assert_eq!(
            store.load("run-1").unwrap().unwrap().data.evidence_revision,
            1
        );
    }

    #[tokio::test]
    async fn reopen_after_argus_commit_replays_without_materializer_call() {
        let directory = tempfile::tempdir().unwrap();
        let target = TargetId::derive([b"target".as_slice()]);
        let policy_id = PolicyId::derive([b"policy".as_slice()]);
        let package = artifact(&target, &policy_id);
        let authorization;
        {
            let store = store_at(directory.path(), &package, &target);
            EvidenceRequestEvaluatorActor::new(store.clone(), package.clone(), policy(target, 2))
                .evaluate("run-1", &events())
                .await
                .unwrap();
            authorization = latest_authorization(&store.load("run-1").unwrap().unwrap())
                .unwrap()
                .clone();
            let expander = Arc::new(FixtureExpander {
                artifact: Mutex::new(expanded(&package, &authorization)),
                calls: AtomicUsize::new(0),
                fail_first_after_materialization: AtomicBool::new(false),
            });
            EvidenceExpansionActor::new(store, package.clone(), expander)
                .expand("run-1", &["evidence.expanded".to_owned()])
                .await
                .unwrap();
        }

        let reopened = Arc::new(WorkflowDataStore::open(directory.path()).unwrap());
        let replay_expander = Arc::new(FixtureExpander {
            artifact: Mutex::new(expanded(&package, &authorization)),
            calls: AtomicUsize::new(0),
            fail_first_after_materialization: AtomicBool::new(false),
        });
        let event = EvidenceExpansionActor::new(reopened, package, replay_expander.clone())
            .expand("run-1", &["evidence.expanded".to_owned()])
            .await
            .unwrap();

        assert_eq!(event.event_type, "evidence.expanded");
        assert_eq!(replay_expander.calls.load(Ordering::SeqCst), 0);
    }
}
