use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone)]
pub(crate) struct WorkerState {
    pub(crate) trace_id: String,
    pub(crate) trace_states: Vec<String>,
    pub(crate) pending_spans: HashMap<String, PendingSpan>,
    pub(crate) implicit_spans: Vec<PendingSpan>,
    pub(crate) terminal_error: Option<String>,
}

impl WorkerState {
    pub(crate) fn is_empty(&self) -> bool {
        self.pending_spans.is_empty()
            && self.implicit_spans.is_empty()
            && self.terminal_error.is_none()
    }
}

#[derive(Clone)]
pub(crate) struct PendingSpan {
    pub(crate) span_id: String,
    pub(crate) parent_span_id: Option<String>,
    pub(crate) start_time_ns: u128,
    pub(crate) attributes: HashMap<String, String>,
}

thread_local! {
    pub(crate) static WORKER_STATES: RefCell<HashMap<String, WorkerState>> =
        RefCell::new(HashMap::new());
}
