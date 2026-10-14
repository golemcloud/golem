use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone)]
pub(crate) struct WorkerState {
    pub(crate) trace_id: String,
    pub(crate) trace_states: Vec<String>,
    pub(crate) pending_spans: HashMap<String, PendingSpan>,
    pub(crate) implicit_spans: Vec<PendingSpan>,
    pub(crate) terminal_error: Option<(String, u128)>,
    /// Timestamp (nanos) when the current invocation started, for duration metrics.
    pub(crate) invocation_start_ns: Option<u128>,
    /// Running total of linear memory in bytes (initial + sum of grow deltas).
    pub(crate) total_memory_bytes: u64,
    /// Running count of active resources (created - dropped).
    pub(crate) active_resources: i64,
    pub(crate) resource_identity: Option<ResourceIdentity>,
}

impl WorkerState {
    pub(crate) fn is_empty(&self) -> bool {
        self.pending_spans.is_empty()
            && self.implicit_spans.is_empty()
            && self.terminal_error.is_none()
            && self.invocation_start_ns.is_none()
            && self.total_memory_bytes == 0
            && self.active_resources == 0
            && self.resource_identity.is_none()
    }
}

#[derive(Clone)]
pub(crate) struct PendingSpan {
    pub(crate) span_id: String,
    pub(crate) trace_id: String,
    pub(crate) trace_states: Vec<String>,
    pub(crate) parent_span_id: Option<String>,
    pub(crate) links: Vec<PendingSpanLink>,
    pub(crate) start_time_ns: u128,
    pub(crate) attributes: HashMap<String, String>,
    pub(crate) kind: Option<u32>,
    pub(crate) opening_index: Option<u64>,
}

#[derive(Clone)]
pub(crate) struct ResourceIdentity {
    pub(crate) instance_id: (u64, u64),
    pub(crate) environment_id: (u64, u64),
    pub(crate) agent_mode: &'static str,
    pub(crate) owner_kind: &'static str,
}

#[derive(Clone)]
pub(crate) struct PendingSpanLink {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    pub(crate) trace_states: Vec<String>,
}

thread_local! {
    pub(crate) static WORKER_STATES: RefCell<HashMap<String, WorkerState>> =
        RefCell::new(HashMap::new());
}
