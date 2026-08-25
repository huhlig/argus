use argus_workflow::compile_target_review;
use langchart_runtime::{
    RunStatus,
    instance::ScriptedAgentActor,
    simulation::{SimActorMap, SimulationResult, WorkflowSimulator},
};
use serde_json::json;
use std::sync::Arc;

fn actor(event: &str) -> ScriptedAgentActor {
    let payload = if event == "outcome.recorded" {
        json!({
            "result_ref": "assessment:fixture",
            "disposition": "inserted",
            "storage_key": "fixture-key"
        })
    } else {
        json!({})
    };
    ScriptedAgentActor::emit(event, payload)
}

fn base(review_event: &str) -> SimActorMap {
    SimActorMap::new()
        .add("prepare_evidence", actor("evidence.prepared"))
        .add("primary_review", actor(review_event))
}

async fn simulate(actors: SimActorMap) -> SimulationResult {
    WorkflowSimulator::new(Arc::new(compile_target_review().unwrap()))
        .with_actors(actors)
        .run()
        .await
        .unwrap()
}

fn traversed(result: &SimulationResult, from: &str, to: &str, event: &str) -> bool {
    result.events.iter().any(|record| {
        let value = serde_json::to_value(&record.payload).unwrap();
        value["kind"] == "transition_selected"
            && value["from"] == from
            && value["to"] == to
            && value["event_type"] == event
    })
}

#[tokio::test]
async fn pass_and_suggestion_record_an_outcome() {
    for event in ["review.pass", "review.suggestion"] {
        let result = simulate(base(event).add("record_outcome", actor("outcome.recorded"))).await;
        assert_eq!(result.status, RunStatus::Completed);
        assert!(traversed(
            &result,
            "primary_review",
            "record_outcome",
            event
        ));
    }
}

#[tokio::test]
async fn candidates_are_recorded_and_scheduled_before_the_outcome() {
    let result = simulate(
        base("review.candidate_found")
            .add("record_candidates", actor("candidates.recorded"))
            .add("schedule_finding_work", actor("finding_work.scheduled"))
            .add("record_outcome", actor("outcome.recorded")),
    )
    .await;

    assert_eq!(result.status, RunStatus::Completed);
    assert!(traversed(
        &result,
        "primary_review",
        "record_candidates",
        "review.candidate_found"
    ));
    assert!(traversed(
        &result,
        "record_candidates",
        "schedule_finding_work",
        "candidates.recorded"
    ));
}

#[tokio::test]
async fn denied_or_exhausted_evidence_becomes_unable_to_verify() {
    for event in ["request.denied", "budget.exhausted"] {
        let result = simulate(
            base("review.unable_to_verify")
                .add("evaluate_evidence_request", actor(event))
                .add(
                    "record_unable_to_verify",
                    actor("unable_to_verify.recorded"),
                )
                .add("record_outcome", actor("outcome.recorded")),
        )
        .await;
        assert_eq!(result.status, RunStatus::Completed);
        assert!(traversed(
            &result,
            "evaluate_evidence_request",
            "record_unable_to_verify",
            event
        ));
    }
}

#[tokio::test]
async fn allowed_evidence_expansion_reenters_primary_review() {
    let result = WorkflowSimulator::new(Arc::new(compile_target_review().unwrap()))
        .with_actors(
            base("review.unable_to_verify")
                .add("evaluate_evidence_request", actor("request.allowed"))
                .add("expand_evidence", actor("evidence.expanded")),
        )
        .with_step_limit(12)
        .run()
        .await
        .unwrap();

    assert_eq!(result.status, RunStatus::Running);
    assert!(traversed(
        &result,
        "evaluate_evidence_request",
        "expand_evidence",
        "request.allowed"
    ));
    assert!(traversed(
        &result,
        "expand_evidence",
        "primary_review",
        "evidence.expanded"
    ));
}

#[tokio::test]
async fn declared_review_failure_never_uses_the_pass_outcome_path() {
    let result =
        simulate(base("review.failed").add("record_failure", actor("failure.recorded"))).await;

    assert_eq!(result.status, RunStatus::Completed);
    assert!(traversed(
        &result,
        "primary_review",
        "record_failure",
        "review.failed"
    ));
    assert!(!traversed(
        &result,
        "primary_review",
        "record_outcome",
        "review.pass"
    ));
}

#[tokio::test]
async fn provider_actor_error_fails_the_run_instead_of_passing() {
    let result = simulate(
        SimActorMap::new()
            .add("prepare_evidence", actor("evidence.prepared"))
            .add(
                "primary_review",
                ScriptedAgentActor::fail("provider unavailable"),
            ),
    )
    .await;

    assert_eq!(result.status, RunStatus::Failed);
    assert!(!traversed(
        &result,
        "primary_review",
        "record_outcome",
        "review.pass"
    ));
}
