// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{
    ClassifiedHostError, DurabilityHost, HostFailureKind, SemanticTrapRetryOverride,
    SemanticTrapRetryOverrideMarker, SemanticTrapRetryVerdict,
};
use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_in_function_retry;
use crate::services::HasWorker;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::oplog::OplogError;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{DurableFunctionType, OplogEntry, OplogErrorKind, OplogIndex};
use golem_common::model::{
    NamedRetryPolicy, PredicateValue, RetryEvaluationError, RetryPolicyState, RetryProperties,
    RetryVerdict, ThreadRng, Timestamp,
};
use golem_service_base::error::worker_executor::InterruptKind;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// Result of `try_trigger_retry_or_loop`: tells the host function caller what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalRetryResult {
    /// The operation result (success or permanent failure) should be persisted in the oplog.
    Persist,
    /// The host function should re-execute the operation (an internal retry sleep has completed).
    RetryInternally,
}

/// Result of `InFunctionRetryState::decide_async_retry`: tells the async RPC caller what to do.
#[derive(Debug, Clone)]
pub enum AsyncRetryDecision {
    /// The caller should wait for the given duration, then retry the operation.
    RetryAfterDelay(Duration),
    /// Max retry attempts exhausted — persist the failure permanently.
    Exhausted,
    /// Fall back to trap+replay. When policy evaluation already happened, carries the exact
    /// post-step decision so the trap path does not step the policy a second time.
    FallBackToTrap(Option<SemanticTrapRetryOverride>),
}

#[derive(Debug)]
pub struct DurableExecutionState {
    pub is_live: bool,
    pub snapshotting_mode: bool,
    /// Maximum delay for in-function retries. Delays exceeding this fall back to trap+replay.
    pub max_in_function_retry_delay: Duration,
}

/// Subset of `DurabilityHost` needed by `InFunctionRetryState::decide_retry`.
///
/// This trait is implemented both for `DurableWorkerCtx` (in-context host calls) and
/// for `TaskRetryContext` (spawned background tasks like async RPC/HTTP), allowing
/// the retry decision logic to be shared without duplicating delay calculation,
/// metric emission, and oplog error writing.
#[async_trait]
pub trait InFunctionRetryHost {
    /// Returns the oplog index that `OplogEntry::Error` entries should reference as `retry_from`.
    fn current_retry_point(&self) -> OplogIndex;

    /// Returns available semantic retry policies for the current worker context.
    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy>;

    /// Returns the current semantic retry policy state for the given retry point, if available.
    async fn current_retry_state_for(&self, retry_from: OplogIndex) -> Option<RetryPolicyState>;

    /// Returns the current durable execution state.
    fn durable_execution_state(&self) -> DurableExecutionState;

    /// Writes an `OplogEntry::Error` entry for an in-function retry attempt, and commits.
    ///
    /// A refusal is returned: the retry must not run again for an attempt that was never recorded.
    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
    ) -> Result<(), OplogError>;
}

pub(crate) fn collect_named_retry_policies(
    config: &HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,
) -> Vec<NamedRetryPolicy> {
    use golem_common::schema::FromSchema;

    let mut policies = Vec::new();

    for typed in config.values() {
        let value = typed.value();
        if let Ok(mut parsed) = Vec::<NamedRetryPolicy>::from_value(value) {
            policies.append(&mut parsed);
            continue;
        }

        if let Ok(parsed) = NamedRetryPolicy::from_value(value) {
            policies.push(parsed);
        }
    }

    policies
}

/// Merges the named-retry-policy tiers into the effective policy list, shared by the direct
/// worker context and the spawned-task retry context: the config-based default catch-all
/// (tier 0), agent-config policies (tier 1), and environment-level policies (tier 2) override
/// each other by name in that order, then the runtime overlay (tier 3, highest precedence) is
/// applied — a `Some` mutation inserts/replaces the policy, a `None` mutation removes it. The
/// result is sorted by descending priority, then by name.
pub(crate) fn merge_named_retry_policy_tiers(
    default_policy: NamedRetryPolicy,
    agent_config_policies: impl IntoIterator<Item = NamedRetryPolicy>,
    environment_policies: impl IntoIterator<Item = NamedRetryPolicy>,
    runtime_retry_policy_mutations: &BTreeMap<String, Option<NamedRetryPolicy>>,
) -> Vec<NamedRetryPolicy> {
    let mut deduped = BTreeMap::new();
    deduped.insert(default_policy.name.clone(), default_policy);
    for policy in agent_config_policies {
        deduped.insert(policy.name.clone(), policy);
    }
    for policy in environment_policies {
        deduped.insert(policy.name.clone(), policy);
    }
    for (name, mutation) in runtime_retry_policy_mutations {
        match mutation {
            Some(policy) => {
                deduped.insert(name.clone(), policy.clone());
            }
            None => {
                deduped.remove(name);
            }
        }
    }
    let mut policies: Vec<NamedRetryPolicy> = deduped.into_values().collect();
    policies.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.name.cmp(&b.name))
    });
    policies
}

pub(crate) fn evaluate_named_policy_step(
    named_policy: &NamedRetryPolicy,
    properties: &RetryProperties,
    current_state: Option<&RetryPolicyState>,
) -> Result<(RetryPolicyState, RetryVerdict), RetryEvaluationError> {
    evaluate_named_policy_step_at(
        named_policy,
        properties,
        current_state,
        Timestamp::now_utc().to_millis(),
    )
}

fn evaluate_named_policy_step_at(
    named_policy: &NamedRetryPolicy,
    properties: &RetryProperties,
    current_state: Option<&RetryPolicyState>,
    now_millis: u64,
) -> Result<(RetryPolicyState, RetryVerdict), RetryEvaluationError> {
    let mut rng = ThreadRng;
    let state = current_state
        .cloned()
        .unwrap_or_else(|| named_policy.policy.initial_state());
    let (state, time_box_state) = if named_policy.policy.contains_time_box() {
        match state {
            RetryPolicyState::TimeBox {
                started_at_millis,
                elapsed_millis,
                inner,
            } => {
                let elapsed_millis =
                    elapsed_millis.max(now_millis.saturating_sub(started_at_millis));
                (*inner, Some((started_at_millis, elapsed_millis)))
            }
            state => (state, Some((now_millis, 0))),
        }
    } else {
        (state, None)
    };
    let elapsed = Duration::from_millis(
        time_box_state
            .map(|(_, elapsed_millis)| elapsed_millis)
            .unwrap_or_default(),
    );

    let (new_state, verdict) = named_policy
        .policy
        .step(&state, elapsed, properties, &mut rng);

    let new_state = match time_box_state {
        Some((started_at_millis, elapsed_millis)) => RetryPolicyState::TimeBox {
            started_at_millis,
            elapsed_millis,
            inner: Box::new(new_state),
        },
        None => new_state,
    };

    Ok((new_state, verdict))
}

/// Like [`evaluate_named_policy_step`], but if the evaluation reports
/// `InvalidState` against a non-empty `current_state` (typically because
/// the persisted state was written by a *different* policy at the same
/// retry point — e.g. the in-function HTTP status retry path leaves a
/// `CountBox{Counter}` state at the request's begin index, and a
/// subsequent trap-retry path then evaluates a `Jitter/Exp` policy
/// against that slot), automatically retry the evaluation as if no
/// prior state existed.
///
/// This is a defensive guard for the case where a single retry-state
/// slot is shared between multiple distinct policies. Without it, the
/// shape mismatch falls through to the legacy retry config and silently
/// disables retrying altogether.
///
/// We only do this fallback for `InvalidState`; other evaluation errors
/// (e.g. `PropertyNotFound`, `CoercionFailed`) are real errors and must
/// surface as before.
pub(crate) fn evaluate_named_policy_step_resetting_on_invalid_state(
    named_policy: &NamedRetryPolicy,
    properties: &RetryProperties,
    current_state: Option<&RetryPolicyState>,
) -> Result<(RetryPolicyState, RetryVerdict), RetryEvaluationError> {
    let result = evaluate_named_policy_step(named_policy, properties, current_state)?;
    match (&result.1, current_state) {
        (RetryVerdict::Error(RetryEvaluationError::InvalidState { details }), Some(_)) => {
            tracing::debug!(
                policy = %named_policy.name,
                details,
                "Retry-state shape mismatch detected; retrying evaluation with empty state"
            );
            evaluate_named_policy_step(named_policy, properties, None)
        }
        _ => Ok(result),
    }
}

/// Encapsulates in-function retry state for a single durable function invocation.
///
/// Tracks the retry count accumulated within the current host function call
/// (as opposed to oplog-level retries which restart the entire worker).
#[derive(Debug)]
pub struct InFunctionRetryState {
    /// Number of in-function retries performed so far in this invocation.
    retry_count: u32,
}

impl Default for InFunctionRetryState {
    fn default() -> Self {
        Self::new()
    }
}

impl InFunctionRetryState {
    pub fn new() -> Self {
        Self { retry_count: 0 }
    }

    /// Returns the number of in-function retries performed so far.
    pub fn retry_count(&self) -> u32 {
        self.retry_count
    }

    /// Decides whether an in-function retry should happen against a specific named
    /// policy (skipping the normal resolver).
    ///
    /// Use this when the caller has already selected the matching policy and must
    /// not re-enter the resolver — for example, code paths whose entire behavior
    /// depends on whether a *user-defined* policy matches (the synthesized default
    /// must not silently take over). All other semantics —
    /// retry-state tracking, oplog error entry, retry metric — are identical to
    /// `decide_retry_with_properties`.
    pub async fn decide_retry_for_named_policy(
        &mut self,
        ctx: &mut (impl InFunctionRetryHost + Sync),
        function_label: &str,
        properties: &RetryProperties,
        named_policy: &NamedRetryPolicy,
    ) -> AsyncRetryDecision {
        let retry_point = ctx.current_retry_point();
        let current_state = ctx.current_retry_state_for(retry_point).await;
        let oplog_retry_count = current_state.as_ref().map(|s| s.retry_count()).unwrap_or(0);
        let total_attempts = self.retry_count + oplog_retry_count;

        self.apply_policy_step(
            ctx,
            function_label,
            properties,
            named_policy,
            current_state.as_ref(),
            total_attempts,
            retry_point,
        )
        .await
    }

    /// Decides whether an in-function retry should happen based on the current retry budget,
    /// and delay threshold.
    ///
    /// On `RetryAfterDelay`, increments the retry counter, writes an oplog error entry,
    /// emits a debug log, and records the metric.
    pub async fn decide_retry_with_properties(
        &mut self,
        ctx: &mut (impl InFunctionRetryHost + Sync),
        function_label: &str,
        properties: &RetryProperties,
    ) -> AsyncRetryDecision {
        let retry_point = ctx.current_retry_point();
        let current_state = ctx.current_retry_state_for(retry_point).await;
        let oplog_retry_count = current_state.as_ref().map(|s| s.retry_count()).unwrap_or(0);
        let total_attempts = self.retry_count + oplog_retry_count;

        let policies = ctx.named_retry_policies().await;
        let named_policy = match NamedRetryPolicy::resolve(&policies, properties) {
            Ok(Some(policy)) => policy,
            Ok(None) => return AsyncRetryDecision::Exhausted,
            Err(error) => {
                warn!(
                    function = function_label,
                    total_attempts,
                    ?error,
                    "Failed resolving semantic retry policy"
                );
                return AsyncRetryDecision::Exhausted;
            }
        };

        self.apply_policy_step(
            ctx,
            function_label,
            properties,
            named_policy,
            current_state.as_ref(),
            total_attempts,
            retry_point,
        )
        .await
    }

    /// Shared body of the in-function retry decision: evaluates the selected named
    /// policy against current retry state, updates retry-budget bookkeeping, and
    /// produces the resulting `AsyncRetryDecision`.
    #[allow(clippy::too_many_arguments)]
    async fn apply_policy_step(
        &mut self,
        ctx: &mut (impl InFunctionRetryHost + Sync),
        function_label: &str,
        properties: &RetryProperties,
        named_policy: &NamedRetryPolicy,
        current_state: Option<&RetryPolicyState>,
        total_attempts: u32,
        retry_point: OplogIndex,
    ) -> AsyncRetryDecision {
        let retry_policy_state: Option<RetryPolicyState>;

        let delay = match evaluate_named_policy_step_resetting_on_invalid_state(
            named_policy,
            properties,
            current_state,
        ) {
            Ok((new_state, RetryVerdict::Retry(delay))) => {
                retry_policy_state = Some(new_state);
                delay
            }
            Ok((_new_state, RetryVerdict::GiveUp)) => return AsyncRetryDecision::Exhausted,
            Ok((_new_state, RetryVerdict::Error(error))) => {
                warn!(
                    function = function_label,
                    retry_policy = %named_policy.name,
                    total_attempts,
                    ?error,
                    "Semantic retry policy evaluation returned error verdict"
                );
                return AsyncRetryDecision::Exhausted;
            }
            Err(error) => {
                warn!(
                    function = function_label,
                    retry_policy = %named_policy.name,
                    total_attempts,
                    ?error,
                    "Failed evaluating semantic retry policy"
                );
                return AsyncRetryDecision::Exhausted;
            }
        };

        let state = ctx.durable_execution_state();
        if delay > state.max_in_function_retry_delay {
            let semantic_override =
                named_policy
                    .policy
                    .contains_time_box()
                    .then(|| SemanticTrapRetryOverride {
                        retry_from: retry_point,
                        policy_name: named_policy.name.clone(),
                        verdict: SemanticTrapRetryVerdict::Retry(delay),
                        retry_policy_state: retry_policy_state
                            .expect("retry verdict must produce retry policy state"),
                    });
            return AsyncRetryDecision::FallBackToTrap(semantic_override);
        }

        // Refused, the shard has a new owner: the failure is returned instead of retried, and the
        // trap it becomes is classified as the lost shard.
        if ctx
            .append_retry_error_entry(retry_point, retry_policy_state)
            .await
            .is_err()
        {
            return AsyncRetryDecision::FallBackToTrap(None);
        }
        self.retry_count += 1;

        debug!(
            retry_count = self.retry_count,
            total_attempt = total_attempts + 1,
            function = function_label,
            ?delay,
            "In-function retry",
        );
        record_in_function_retry();

        AsyncRetryDecision::RetryAfterDelay(delay)
    }
}
pub(crate) async fn try_trigger_host_trap_retry(
    ctx: &mut (impl InFunctionRetryHost + Sync),
    failure: Error,
    properties: RetryProperties,
) -> anyhow::Result<()> {
    let current_retry_point = ctx.current_retry_point();

    // Resolve the matching named retry policy. The synthesized
    // default-from-config policy (with `Predicate::True`) is always
    // present in `named_retry_policies()`, so resolution finds at least
    // one match for every property set; there is no legacy `RetryConfig`
    // path any more.
    let policies = ctx.named_retry_policies().await;
    let named_policy =
        match NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
            &policies,
            &properties,
        ) {
            Ok(Some(named_policy)) => named_policy,
            Ok(None) => {
                warn!(
                    retry_path = "host-trap",
                    "No named retry policy matched (including the synthesized default); giving up"
                );
                return Ok(());
            }
            Err(error) => {
                warn!(
                    ?error,
                    retry_path = "host-trap",
                    "Failed resolving semantic host-trap retry policy; giving up"
                );
                return Ok(());
            }
        };

    let current_state = ctx.current_retry_state_for(current_retry_point).await;
    let total_attempts = current_state.as_ref().map(|s| s.retry_count()).unwrap_or(0);

    match evaluate_named_policy_step_resetting_on_invalid_state(
        named_policy,
        &properties,
        current_state.as_ref(),
    ) {
        Ok((new_state, RetryVerdict::Retry(delay))) => {
            debug!(
                retry_policy = %named_policy.name,
                retry_path = "host-trap",
                retry_policy_source = "worker-local",
                retry_decision = "retry",
                attempt = total_attempts + 1,
                delay_ms = delay.as_millis() as u64,
                "Semantic host-trap retry: triggering retry"
            );
            // Attach the resolved verdict to the failure so the post-trap recovery path can honour
            // it directly without re-resolving from an impoverished trap context (e.g. missing HTTP
            // `status-code`).
            let payload = SemanticTrapRetryOverride {
                retry_from: current_retry_point,
                policy_name: named_policy.name.clone(),
                verdict: SemanticTrapRetryVerdict::Retry(delay),
                retry_policy_state: new_state,
            };
            Err(anyhow::Error::new(SemanticTrapRetryOverrideMarker {
                payload,
                inner: failure,
            }))
        }
        Ok((_new_state, RetryVerdict::GiveUp)) => {
            debug!(
                retry_policy = %named_policy.name,
                retry_path = "host-trap",
                retry_policy_source = "worker-local",
                retry_decision = "give-up",
                attempt = total_attempts + 1,
                "Semantic host-trap retry: exhausted"
            );
            Ok(())
        }
        Ok((_new_state, RetryVerdict::Error(error))) => {
            warn!(
                retry_policy = %named_policy.name,
                ?error,
                retry_path = "host-trap",
                fallback_reason = "eval-error",
                "Semantic host-trap retry evaluation returned an error verdict; giving up"
            );
            Ok(())
        }
        Err(error) => {
            warn!(
                retry_policy = %named_policy.name,
                ?error,
                retry_path = "host-trap",
                fallback_reason = "eval-error",
                "Failed evaluating semantic host-trap retry policy; giving up"
            );
            Ok(())
        }
    }
}
#[async_trait]
impl<Ctx: WorkerCtx> InFunctionRetryHost for DurableWorkerCtx<Ctx> {
    fn current_retry_point(&self) -> OplogIndex {
        self.state.effective_retry_point()
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        self.state.named_retry_policies().await
    }

    async fn current_retry_state_for(&self, retry_from: OplogIndex) -> Option<RetryPolicyState> {
        let latest_status = self.public_state.worker().get_last_known_status().await;
        latest_status.current_retry_state.get(&retry_from).cloned()
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: self.state.durable_call_is_live() || self.state.durability_is_suppressed(),
            snapshotting_mode: self.state.snapshotting_mode,
            max_in_function_retry_delay: self.state.config.max_in_function_retry_delay,
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
    ) -> Result<(), OplogError> {
        if self.state.durability_is_suppressed() {
            return Ok(());
        }

        use golem_common::model::oplog::AgentError;
        let entry = OplogEntry::error(
            self.entity_parent_start_index(),
            OplogErrorKind::Invocation,
            AgentError::TransientError("in-function retry".to_string()),
            retry_from,
            retry_policy_state,
        );
        self.public_state
            .worker()
            .add_and_commit_oplog(entry)
            .await?;
        Ok(())
    }
}
/// Holds the in-function retry decision logic for a single durable host call, decoupled from how
/// the call's request/response is persisted or replayed. Both the sequential [`Durability`] and the
/// concurrent [`crate::durable_host::concurrent::DurableCallSession`] own one and route their
/// `try_trigger_retry*` methods through it, so the retry logic has a single home.
///
/// Every method takes `&mut impl DurabilityHost`, so the controller stays unit-testable against
/// `MockDurabilityHost` without a real `DurableWorkerCtx`.
pub struct InFunctionRetryController {
    function_type: DurableFunctionType,
    durable_execution_state: DurableExecutionState,
    retry_state: InFunctionRetryState,
    /// Fully-qualified host-function name used as the retry decision's function label.
    function_label: &'static str,
    parent_start_index: Option<OplogIndex>,
    /// The protocol owner authorizes repair or typed loss under the child's recorded Start.
    incomplete_repair_authorized_by_scope: bool,
}

impl InFunctionRetryController {
    pub fn new(
        function_type: DurableFunctionType,
        durable_execution_state: DurableExecutionState,
        function_label: &'static str,
    ) -> Self {
        Self {
            function_type,
            durable_execution_state,
            retry_state: InFunctionRetryState::new(),
            function_label,
            parent_start_index: None,
            incomplete_repair_authorized_by_scope: false,
        }
    }

    pub fn function_type(&self) -> &DurableFunctionType {
        &self.function_type
    }

    pub fn with_parent_start_index(mut self, parent_start_index: Option<OplogIndex>) -> Self {
        self.parent_start_index = parent_start_index;
        self
    }

    pub fn with_incomplete_repair_authorized_by_scope(mut self, authorized: bool) -> Self {
        self.incomplete_repair_authorized_by_scope = authorized;
        self
    }

    pub fn incomplete_repair_authorized_by_scope(&self) -> bool {
        self.incomplete_repair_authorized_by_scope
    }

    pub fn durable_execution_state(&self) -> &DurableExecutionState {
        &self.durable_execution_state
    }

    pub fn is_live(&self) -> bool {
        self.durable_execution_state.is_live
    }

    /// Checks if the current retry policy allows more retries, and if yes, then returns
    /// with `Err(failure)`. This error should be directly returned from host function
    /// implementations, triggering a retry.
    ///
    /// If retrying is not possible, the function returns Ok(()) and the host function
    /// can continue persisting the failed result permanently.
    ///
    /// The `classify` closure determines whether the failure is transient or permanent. Permanent
    /// failures return `Ok(())` immediately so the caller can persist them. Transient failures are
    /// wrapped in [`ClassifiedHostError`] before retry-policy evaluation so trap classification
    /// preserves the failure kind.
    pub async fn try_trigger_retry<Ok, Err: Display>(
        &self,
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<()> {
        self.try_trigger_retry_with_properties(ctx, result, classify, RetryProperties::new())
            .await
    }

    pub async fn try_trigger_retry_with_properties<Ok, Err: Display>(
        &self,
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        if let Err(err) = result {
            let kind = classify(err);
            match kind {
                HostFailureKind::Permanent => Ok(()),
                HostFailureKind::Transient => {
                    let message = err.to_string();
                    let failure = Error::new(ClassifiedHostError {
                        kind,
                        message: message.clone(),
                    });
                    let mut properties = properties;
                    properties.set("error-type", PredicateValue::Text("transient".to_string()));
                    ctx.try_trigger_retry(failure, properties).await
                }
            }
        } else {
            Ok(())
        }
    }

    /// Like `try_trigger_retry`, but supports in-function retries for eligible host functions.
    ///
    /// Returns:
    /// - `Ok(InternalRetryResult::Persist)` — persist the result (success or permanent/exhausted failure)
    /// - `Ok(InternalRetryResult::RetryInternally)` — caller should re-execute the operation
    /// - `Err(...)` — propagated trap (fallback to oplog replay, or interrupt)
    pub async fn try_trigger_retry_or_loop<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<InternalRetryResult> {
        self.try_trigger_retry_or_loop_with_properties(
            ctx,
            result,
            classify,
            RetryProperties::new(),
        )
        .await
    }

    pub async fn try_trigger_retry_or_loop_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<InternalRetryResult> {
        self.try_trigger_retry_or_loop_with_capabilities(
            ctx,
            result,
            classify,
            properties,
            |ctx| ctx.create_interrupt_signal(),
            |ctx, failure, properties| ctx.try_trigger_retry(failure, properties),
        )
        .await
    }

    pub(crate) async fn try_trigger_retry_or_loop_with_capabilities<H, Ok, Err: Display>(
        &mut self,
        ctx: &mut H,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
        interrupt: impl FnOnce(&H) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>>,
        fallback: impl for<'a> FnOnce(
            &'a mut H,
            Error,
            RetryProperties,
        )
            -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>,
    ) -> anyhow::Result<InternalRetryResult>
    where
        H: InFunctionRetryHost + Sync,
    {
        let err = match result {
            Ok(_) => return Ok(InternalRetryResult::Persist),
            Err(err) => err,
        };

        let kind = classify(err);

        let mut properties = properties;
        properties.set(
            "error-type",
            PredicateValue::Text(match kind {
                HostFailureKind::Transient => "transient".to_string(),
                HostFailureKind::Permanent => "permanent".to_string(),
            }),
        );

        if kind == HostFailureKind::Permanent {
            return Ok(InternalRetryResult::Persist);
        }

        if !self.is_eligible_for_internal_retry() {
            let message = err.to_string();
            let failure = Error::new(ClassifiedHostError { kind, message });
            fallback(ctx, failure, properties).await?;
            return Ok(InternalRetryResult::Persist);
        }

        let decision = self
            .retry_state
            .decide_retry_with_properties(ctx, self.function_label, &properties)
            .await;

        match decision {
            AsyncRetryDecision::RetryAfterDelay(delay) => {
                // Interrupt-aware sleep
                let interrupt = interrupt(ctx);
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);

                match futures::future::select(sleep, interrupt).await {
                    futures::future::Either::Left((_done, _)) => {
                        Ok(InternalRetryResult::RetryInternally)
                    }
                    futures::future::Either::Right((interrupt_kind, _)) => {
                        Err(anyhow::Error::from(interrupt_kind))
                    }
                }
            }
            AsyncRetryDecision::Exhausted => Ok(InternalRetryResult::Persist),
            AsyncRetryDecision::FallBackToTrap(semantic_override) => {
                let message = err.to_string();
                let failure = Error::new(ClassifiedHostError { kind, message });
                if let Some(payload) = semantic_override {
                    return Err(anyhow::Error::new(SemanticTrapRetryOverrideMarker {
                        payload,
                        inner: failure,
                    }));
                }
                fallback(ctx, failure, properties).await?;
                // If try_trigger_retry returned Ok, retries are exhausted — persist the failure
                Ok(InternalRetryResult::Persist)
            }
        }
    }

    /// Checks whether the current function type is eligible for in-function retry.
    fn is_eligible_for_internal_retry(&self) -> bool {
        match &self.function_type {
            DurableFunctionType::ReadRemote
            | DurableFunctionType::ReadLocal
            | DurableFunctionType::WriteLocal
            | DurableFunctionType::WriteRemote => true,
        }
    }

    /// Top-level calls repair in place. A child requires its protocol owner's decision,
    /// whether that decision re-executes an effect or records typed resource loss.
    pub fn can_reexecute_on_incomplete_replay(&self) -> bool {
        self.parent_start_index.is_none() || self.incomplete_repair_authorized_by_scope
    }
}

/// Counts the number of `OplogEntry::Error` entries in the oplog whose `retry_from`
/// matches the given `retry_point`. This is used to initialize `base_retry_count`
/// when spawning background retry tasks, so that the retry budget accounts for
/// errors already recorded before the task was spawned.
pub async fn count_oplog_errors_for(
    oplog: &Arc<dyn crate::services::oplog::Oplog>,
    retry_point: OplogIndex,
) -> Result<u32, crate::services::oplog::UnreadableOplog> {
    let len = oplog.length().await?;
    if len == 0 {
        return Ok(0);
    }
    let entries = oplog.read_exact(OplogIndex::INITIAL, len).await?;
    let mut count: u32 = 0;
    for entry in entries.values() {
        if let OplogEntry::Error { retry_from, .. } = entry
            && *retry_from == retry_point
        {
            count += 1;
        }
    }
    Ok(count)
}

/// Implementation of [`InFunctionRetryHost`] for spawned background tasks (async RPC, HTTP).
///
/// Unlike `DurableWorkerCtx`, this does NOT read retry state from worker status
/// (which is invocation-scoped and can panic if detached). Instead it captures
/// the current retry policy state at spawn time from the worker's last known status.
///
/// Error entries are written through `Worker::add_and_commit_oplog` so that the
/// worker status tracker is properly updated (unlike raw `oplog.add_and_commit`).
pub struct TaskRetryContext<Ctx: WorkerCtx> {
    /// The oplog index that error entries reference as their `retry_from` point.
    pub retry_point: OplogIndex,
    /// Entity invocation that initiated this task, if any.
    pub entity_parent_start_index: Option<OplogIndex>,
    /// Environment state service for lazy policy fetching
    pub environment_state_service: Arc<dyn EnvironmentStateService>,
    /// Environment ID for policy lookup
    pub environment_id: EnvironmentId,
    /// Default catch-all retry policy derived from GolemConfig (priority 0, Predicate::True)
    pub default_retry_policy: NamedRetryPolicy,
    /// Cached agent-config-derived retry policies (cheap, already computed)
    pub agent_config_retry_policies: Vec<NamedRetryPolicy>,
    /// Runtime overlay mutations (set/remove via guest API)
    pub runtime_retry_policy_mutations: BTreeMap<String, Option<NamedRetryPolicy>>,
    /// Maximum delay for in-function retries; delays exceeding this fall back to trap+replay.
    pub max_in_function_retry_delay: Duration,
    /// The current semantic retry policy state, if a named policy was previously selected.
    pub current_retry_policy_state: Option<RetryPolicyState>,
    /// Properties describing the error context (verb, URI, status code, etc.) for predicate evaluation.
    pub retry_properties: RetryProperties,
    /// Reference to the worker that owns this task.
    pub worker: Arc<crate::worker::Worker<Ctx>>,
}

#[async_trait]
impl<Ctx: WorkerCtx> InFunctionRetryHost for TaskRetryContext<Ctx> {
    fn current_retry_point(&self) -> OplogIndex {
        self.retry_point
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        // Fetch environment policies lazily (only called on error path)
        let environment_policies = self
            .environment_state_service
            .get_retry_policies(self.environment_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to fetch environment retry policies in task context: {e}");
                vec![]
            });

        merge_named_retry_policy_tiers(
            self.default_retry_policy.clone(),
            self.agent_config_retry_policies.iter().cloned(),
            environment_policies,
            &self.runtime_retry_policy_mutations,
        )
    }

    async fn current_retry_state_for(&self, _retry_from: OplogIndex) -> Option<RetryPolicyState> {
        self.current_retry_policy_state.clone()
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            snapshotting_mode: false,
            max_in_function_retry_delay: self.max_in_function_retry_delay,
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
    ) -> Result<(), OplogError> {
        use golem_common::model::oplog::AgentError;
        let entry = OplogEntry::error(
            self.entity_parent_start_index,
            OplogErrorKind::Invocation,
            AgentError::TransientError("in-function retry".to_string()),
            retry_from,
            retry_policy_state.clone(),
        );
        self.worker.add_and_commit_oplog(entry).await?;

        self.current_retry_policy_state = retry_policy_state;
        Ok(())
    }
}

/// Shared retry loop for spawned background tasks (async RPC, HTTP).
///
/// Runs `operation` in a loop, retrying on transient failures according to the
/// retry budget. Delegates retry decisions to `InFunctionRetryState::decide_retry`
/// via a `TaskRetryContext`, ensuring delay calculation, metric emission, and oplog
/// error writing are shared with the in-context host function retry path.
///
/// Returns:
/// - `Ok(value)` on success
/// - `Err(err)` when the error is permanent, retries are exhausted, or the
///   computed delay exceeds `max_delay`
///
/// For spawned tasks, `FallBackToTrap` from `decide_retry` means "stop inline
/// retry and return the error" — there is no WASM stack to trap.
pub async fn in_task_retry_loop<Ctx, T, E, C, Op, Fut, ISF, ISFut>(
    mut task_ctx: TaskRetryContext<Ctx>,
    classify: C,
    mut operation: Op,
    interrupt_signal_factory: ISF,
) -> Result<T, E>
where
    Ctx: WorkerCtx,
    E: Display,
    C: Fn(&E) -> HostFailureKind,
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    ISF: Fn() -> ISFut,
    ISFut: Future<Output = InterruptKind>,
{
    let mut retry_state = InFunctionRetryState::new();

    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let error_kind = classify(&err);
                if error_kind == HostFailureKind::Permanent {
                    return Err(err);
                }

                let mut retry_properties = task_ctx.retry_properties.clone();
                retry_properties.set("error-type", PredicateValue::Text("transient".to_string()));

                let decision = retry_state
                    .decide_retry_with_properties(&mut task_ctx, "in-task", &retry_properties)
                    .await;

                match decision {
                    AsyncRetryDecision::RetryAfterDelay(delay) => {
                        let sleep = tokio::time::sleep(delay);
                        let interrupt = interrupt_signal_factory();
                        tokio::pin!(sleep);
                        tokio::pin!(interrupt);

                        match futures::future::select(sleep, interrupt).await {
                            futures::future::Either::Left((_done, _)) => {
                                // Sleep completed, continue retry loop
                            }
                            futures::future::Either::Right((_interrupt_kind, _)) => {
                                // Interrupted during backoff — return the last transient error
                                return Err(err);
                            }
                        }
                    }
                    AsyncRetryDecision::Exhausted | AsyncRetryDecision::FallBackToTrap(_) => {
                        return Err(err);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
