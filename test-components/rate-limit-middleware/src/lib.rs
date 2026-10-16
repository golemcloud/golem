use golem_rust::tool::{
    InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool,
    ToolInvokeError, UnderlyingTool,
};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, agent_definition, agent_implementation,
    generate_idempotency_key, universal_tool_middleware,
};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod k3_persistent_rate_limit_contract_tests;

#[derive(Clone, Debug, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct Admission {
    pub admitted: bool,
    pub duplicate: bool,
    pub remaining: u64,
    pub retry_after_milliseconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct BackendStats {
    pub attempts: u64,
    pub committed_charges: u64,
    pub recorded_decisions: u64,
}

#[agent_definition]
pub trait RateLimitBackend {
    fn new(policy: String, limit: u64, window_milliseconds: u64) -> Self;

    async fn admit(&mut self, key: String, logical_invocation_id: String) -> Admission;

    fn stats(&self) -> BackendStats;
}

struct RateLimitBackendImpl {
    _policy: String,
    limit: u64,
    window_milliseconds: u64,
    windows: BTreeMap<(String, u64), u64>,
    decisions: BTreeMap<String, Admission>,
    charged: BTreeSet<String>,
    attempts: u64,
}

impl RateLimitBackendImpl {
    fn admit_at(
        &mut self,
        key: String,
        logical_invocation_id: String,
        now_milliseconds: u64,
    ) -> Admission {
        self.attempts += 1;

        if let Some(decision) = self.decisions.get(&logical_invocation_id) {
            let mut decision = decision.clone();
            decision.duplicate = true;
            return decision;
        }

        let limit = self.limit;
        let window_milliseconds = self.window_milliseconds;
        let starts_at_milliseconds = now_milliseconds - now_milliseconds % window_milliseconds;
        let count = self
            .windows
            .entry((key, starts_at_milliseconds))
            .or_insert(0);

        let decision = if *count < limit {
            *count += 1;
            self.charged.insert(logical_invocation_id.clone());
            Admission {
                admitted: true,
                duplicate: false,
                remaining: limit - *count,
                retry_after_milliseconds: 0,
            }
        } else {
            Admission {
                admitted: false,
                duplicate: false,
                remaining: 0,
                retry_after_milliseconds: starts_at_milliseconds
                    .saturating_add(window_milliseconds)
                    .saturating_sub(now_milliseconds),
            }
        };
        self.decisions
            .insert(logical_invocation_id, decision.clone());
        decision
    }
}

#[agent_implementation]
impl RateLimitBackend for RateLimitBackendImpl {
    fn new(policy: String, limit: u64, window_milliseconds: u64) -> Self {
        assert!(!policy.is_empty(), "rate-limit policy must not be empty");
        assert!(limit > 0, "rate limit must be greater than zero");
        assert!(
            window_milliseconds > 0,
            "rate-limit window must be greater than zero"
        );
        Self {
            _policy: policy,
            limit,
            window_milliseconds,
            windows: BTreeMap::new(),
            decisions: BTreeMap::new(),
            charged: BTreeSet::new(),
            attempts: 0,
        }
    }

    async fn admit(&mut self, key: String, logical_invocation_id: String) -> Admission {
        let now_milliseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must not precede the Unix epoch")
            .as_millis() as u64;
        self.admit_at(key, logical_invocation_id, now_milliseconds)
    }

    fn stats(&self) -> BackendStats {
        BackendStats {
            attempts: self.attempts,
            committed_charges: self.charged.len() as u64,
            recorded_decisions: self.decisions.len() as u64,
        }
    }
}

#[derive(IntoSchema, FromSchema)]
struct RateLimitParameters {
    policy: String,
    limit: u64,
    window_milliseconds: u64,
}

fn principal_key(principal: &Principal) -> String {
    match principal {
        Principal::Oidc(principal) => format!(
            "oidc:{}:{}:{}",
            principal.issuer.len(),
            principal.issuer,
            principal.sub
        ),
        Principal::Agent(principal) => {
            let uuid = &principal.agent_id.component_id.uuid;
            format!(
                "agent:{:016x}{:016x}:{}",
                uuid.high_bits, uuid.low_bits, principal.agent_id.agent_id
            )
        }
        Principal::GolemUser(principal) => {
            let uuid = &principal.account_id.uuid;
            format!("golem-user:{:016x}{:016x}", uuid.high_bits, uuid.low_bits)
        }
        Principal::Anonymous => "anonymous".to_string(),
    }
}

#[universal_tool_middleware(
    name = "persistent-rate-limit",
    version = "0.1.0",
    parameters = RateLimitParameters
)]
async fn persistent_rate_limit(
    parameters: RateLimitParameters,
    _tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: golem_rust::TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    if parameters.policy.is_empty() {
        return Err(ToolInvokeError::InvalidInput(
            "rate-limit policy must not be empty".to_string(),
        ));
    }
    if parameters.limit == 0 || parameters.window_milliseconds == 0 {
        return Err(ToolInvokeError::InvalidInput(
            "rate-limit limit and windowMilliseconds must be greater than zero".to_string(),
        ));
    }

    let mut backend = RateLimitBackendClient::get(
        parameters.policy,
        parameters.limit,
        parameters.window_milliseconds,
    );
    let admission = backend
        .admit(
            principal_key(&principal),
            generate_idempotency_key().to_string(),
        )
        .await;
    if !admission.admitted {
        return Err(ToolInvokeError::ResourceExhausted(format!(
            "rate limit exceeded; retry after {} ms",
            admission.retry_after_milliseconds
        )));
    }

    underlying
        .invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use test_r::test;

    fn backend(limit: u64) -> RateLimitBackendImpl {
        RateLimitBackendImpl::new("test".to_string(), limit, 1_000)
    }

    #[test]
    fn rate_1_limit_boundary_and_independent_keys() {
        let mut backend = backend(3);
        for ordinal in 1..=3 {
            let result = backend.admit_at("principal-a".to_string(), format!("a-{ordinal}"), 1_234);
            assert!(result.admitted);
            assert_eq!(result.remaining, 3 - ordinal);
        }

        let rejected = backend.admit_at("principal-a".to_string(), "a-4".to_string(), 1_234);
        assert!(!rejected.admitted);
        assert_eq!(rejected.retry_after_milliseconds, 766);

        let independent = backend.admit_at("principal-b".to_string(), "b-1".to_string(), 1_234);
        assert!(independent.admitted);
        assert_eq!(independent.remaining, 2);
    }

    #[test]
    fn rate_1_fixed_window_refills_at_epoch_aligned_boundary() {
        let mut backend = backend(1);
        assert!(
            backend
                .admit_at("key".into(), "first".into(), 1_999)
                .admitted
        );
        assert!(
            !backend
                .admit_at("key".into(), "blocked".into(), 1_999)
                .admitted
        );
        assert!(
            backend
                .admit_at("key".into(), "refilled".into(), 2_000)
                .admitted
        );
    }

    #[test]
    fn rate_1_fixed_window_does_not_refill_twice_after_clock_rollback() {
        let mut backend = backend(1);
        assert!(
            backend
                .admit_at("key".into(), "current-first".into(), 2_234)
                .admitted
        );

        // A wall-clock correction visits an older epoch-aligned window.
        assert!(
            backend
                .admit_at("key".into(), "older".into(), 1_234)
                .admitted
        );

        // Returning to the already charged [2_000, 3_000) window must not grant
        // that window a second allowance.
        assert!(
            !backend
                .admit_at("key".into(), "current-second".into(), 2_235)
                .admitted
        );
    }

    #[test]
    fn rate_2_distinct_owners_share_one_atomic_backend_key() {
        const LIMIT: usize = 11;
        const OWNERS: usize = 64;
        let backend = Arc::new(Mutex::new(backend(LIMIT as u64)));
        let barrier = Arc::new(Barrier::new(OWNERS));
        let mut calls = Vec::new();

        for owner in 0..OWNERS {
            let backend = backend.clone();
            let barrier = barrier.clone();
            calls.push(thread::spawn(move || {
                barrier.wait();
                backend.lock().unwrap().admit_at(
                    "shared-principal".to_string(),
                    format!("owner-{owner}"),
                    42,
                )
            }));
        }

        let admitted = calls
            .into_iter()
            .map(|call| call.join().unwrap())
            .filter(|result| result.admitted)
            .count();
        assert_eq!(admitted, LIMIT);
        assert_eq!(
            backend.lock().unwrap().stats().committed_charges,
            LIMIT as u64
        );
    }

    #[test]
    fn rate_3_replayed_attempt_returns_committed_decision_without_charge() {
        let mut backend = backend(1);
        let first = backend.admit_at("principal".into(), "stable-logical-invocation".into(), 999);
        assert!(first.admitted);
        assert!(!first.duplicate);

        // Models recovery after the backend committed but before the middleware propagated its
        // result. The replay can arrive in a later window and still receives the original decision.
        let replay = backend.admit_at(
            "principal".into(),
            "stable-logical-invocation".into(),
            5_000,
        );
        assert!(replay.admitted);
        assert!(replay.duplicate);
        assert_eq!(backend.stats().attempts, 2);
        assert_eq!(backend.stats().committed_charges, 1);

        let next = backend.admit_at("principal".into(), "new-logical-invocation".into(), 5_000);
        assert!(next.admitted);
        assert_eq!(backend.stats().committed_charges, 2);
    }
}
