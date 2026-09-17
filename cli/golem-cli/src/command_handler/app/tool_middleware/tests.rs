// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");

use super::{GrantExecutionDecision, grant_execution_decision};
use test_r::test;

#[test]
fn plan_stops_before_mutation_when_access_is_missing() {
    assert_eq!(
        grant_execution_decision(false, true, true, true),
        GrantExecutionDecision::StopAfterPlan
    );
}

#[test]
fn stage_rejects_access_reconciliation() {
    assert_eq!(
        grant_execution_decision(true, false, true, true),
        GrantExecutionDecision::RejectStage
    );
}

#[test]
fn deletion_only_plan_can_continue_read_only() {
    assert_eq!(
        grant_execution_decision(false, true, false, true),
        GrantExecutionDecision::ContinueReadOnly
    );
}

#[test]
fn apply_requires_confirmation_so_cancellation_precedes_mutation() {
    assert_eq!(
        grant_execution_decision(false, false, true, true),
        GrantExecutionDecision::ConfirmApply
    );
}
