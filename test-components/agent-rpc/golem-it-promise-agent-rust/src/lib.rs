use golem_rust::{PromiseId, agent_definition, agent_implementation};

#[agent_definition]
pub trait PromiseAgent {
    fn new(name: String) -> Self;
    fn get_promise(&self) -> PromiseId;
    fn await_promise(&self, promise_id: PromiseId);
    fn complete_promise(&self, promise_id: PromiseId, payload_size: u32) -> bool;
}

struct PromiseAgentImpl {
    _name: String,
}

#[agent_implementation]
impl PromiseAgent for PromiseAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn get_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn await_promise(&self, promise_id: PromiseId) {
        let _ = golem_rust::blocking_await_promise(&promise_id);
    }

    fn complete_promise(&self, promise_id: PromiseId, payload_size: u32) -> bool {
        golem_rust::complete_promise(&promise_id, &vec![0; payload_size as usize])
    }
}

/// Ceiling on the wakeup log below.
///
/// Far above what a chaos run produces — a few hundred wakeups per waiter — so
/// it only stops a misconfigured cadence from growing agent state without
/// bound. `wakes` keeps counting past it, which is what makes truncation
/// visible: a reader that gets fewer entries than wakes knows the log is short
/// rather than the wakeups missing.
const MAX_WAKEUP_LOG: usize = 10_000;

/// Wall clock in milliseconds since the epoch.
///
/// Read inside the agent rather than passed in, because the question S11 asks is
/// when the *platform* resumed the waiter. The read is durable, so an agent that
/// replays reports the original wakeup time instead of the replay's.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// A durable agent that parks on a promise and records being woken (GOL-377).
///
/// [`PromiseAgent`] above creates and completes promises in one breath, which is
/// what the mixed workload's promise stream needs and says nothing about
/// recovery. S11 needs the other half: an agent genuinely suspended on a promise
/// whose executor is then killed, and a durable record of whether the completion
/// ever reached it.
///
/// The record has to live in agent state rather than in the invocation's return
/// value, because the interesting case is exactly the one where the caller's
/// connection died with the executor. A caller that got an error learns nothing;
/// the log outlives the connection and answers anyway.
#[agent_definition]
pub trait PromiseWaiter {
    fn new(name: String) -> Self;

    /// Creates a promise for this waiter and returns it without waiting.
    ///
    /// Separate from [`PromiseWaiter::wait`] because the completer needs the
    /// promise id, and an invocation that blocks cannot return one. `token` is
    /// the driver's idempotency key for this round, and is what pairs a wakeup
    /// back to the completion that caused it.
    fn arm(&mut self, token: String) -> PromiseId;

    /// Blocks until `promise_id` is completed, then records the wakeup.
    ///
    /// Suspends the agent, so the executor holds no thread for it: this is the
    /// state S11 kills an executor in.
    fn wait(&mut self, token: String, promise_id: PromiseId);

    /// The wakeup log: one `(token, armed_millis, woken_millis)` per wakeup, in
    /// the order the waiter was resumed.
    fn wakeups(&self) -> Vec<(String, u64, u64)>;

    /// How many wakeups happened, whether or not the log kept them.
    fn wakes(&self) -> u32;
}

struct PromiseWaiterImpl {
    _name: String,
    /// When each token was armed, so a wakeup can report the interval it was
    /// parked without the driver having to join two clocks.
    armed: Vec<(String, u64)>,
    wakeups: Vec<(String, u64, u64)>,
    wakes: u32,
}

#[agent_implementation]
impl PromiseWaiter for PromiseWaiterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            armed: Vec::new(),
            wakeups: Vec::new(),
            wakes: 0,
        }
    }

    fn arm(&mut self, token: String) -> PromiseId {
        if self.armed.len() >= MAX_WAKEUP_LOG {
            self.armed.remove(0);
        }
        self.armed.push((token, now_millis()));
        golem_rust::create_promise()
    }

    fn wait(&mut self, token: String, promise_id: PromiseId) {
        let _ = golem_rust::blocking_await_promise(&promise_id);
        self.wakes += 1;
        if self.wakeups.len() < MAX_WAKEUP_LOG {
            let armed_millis = self
                .armed
                .iter()
                .rev()
                .find(|(armed_token, _)| armed_token == &token)
                .map(|(_, at)| *at)
                .unwrap_or(0);
            self.wakeups.push((token, armed_millis, now_millis()));
        }
    }

    fn wakeups(&self) -> Vec<(String, u64, u64)> {
        self.wakeups.clone()
    }

    fn wakes(&self) -> u32 {
        self.wakes
    }
}
