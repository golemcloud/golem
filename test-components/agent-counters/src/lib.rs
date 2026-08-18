pub mod repository;
mod snapshot_test;

use golem_rust::bindings::golem::agent::host::Datetime;
use golem_rust::bindings::golem::api::context::start_span;
use golem_rust::quota::QuotaToken;
use golem_rust::{agent_definition, agent_implementation, generate_idempotency_key};

/// Resource name the chaos suite's quota stream reserves against. Must match the
/// resource the chaos prep creates on the environment — see
/// `integration_tests::chaos::prep`.
const CHAOS_QUOTA_RESOURCE: &str = "chaos-quota";

/// Page size used when touching retained memory so the OS backs it with real
/// resident pages rather than leaving it as untouched (non-resident) reservation.
const PAGE_SIZE: usize = 4096;

/// Spins doing cheap arithmetic for approximately `millis` milliseconds, polling
/// the monotonic clock between batches of work rather than on every iteration so
/// the workload is CPU-bound, not clock-syscall-bound. Returns an accumulated
/// value so the work cannot be optimised away.
fn busy_loop(millis: u32) -> u32 {
    let deadline = std::time::Duration::from_millis(millis as u64);
    let start = std::time::Instant::now();
    let mut acc: u32 = 0;
    loop {
        for i in 0..10_000u32 {
            acc = acc.wrapping_add(i).wrapping_mul(31).wrapping_add(7);
        }
        if start.elapsed() >= deadline {
            break;
        }
    }
    acc
}

/// Grows `buffer` to hold `bytes` and touches one byte per page so the memory
/// becomes resident (real RSS), not just reserved address space.
fn retain_memory(buffer: &mut Vec<u8>, bytes: u32) {
    let bytes = bytes as usize;
    buffer.clear();
    buffer.shrink_to_fit();
    buffer.resize(bytes, 0);
    let mut page = 0;
    while page < bytes {
        buffer[page] = buffer[page].wrapping_add(1);
        page += PAGE_SIZE;
    }
}

#[agent_definition]
trait Counter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    async fn increment_through_rpc(&mut self) -> u32;
    async fn increment_through_rpc_to_ephemeral(&mut self) -> u32;
    async fn increment_through_rpc_to_ephemeral_phantom(&mut self) -> u32;

    /// Spins for `millis` milliseconds of cheap CPU work, then increments and
    /// returns the counter. Used to define an "active" agent without making the
    /// workload oplog-bound on a tight loop.
    fn busy_for(&mut self, millis: u32) -> u32;

    /// Retains `bytes` of resident linear memory in the agent's state and
    /// increments the counter. The memory stays resident across invocations so
    /// the agent contributes a controllable footprint to the executor's pool.
    fn allocate_memory(&mut self, bytes: u32) -> u32;

    /// Performs `entries` cheap host calls under smart persistence. Used by
    /// oplog recovery benchmarks to grow replay history without CPU burn.
    fn oplog_heavy(&mut self, entries: u32) -> u32;

    /// Reads the counter without touching it. Chaos scenarios compare this
    /// against the number of increments they submitted, so the read itself must
    /// not move the number it is measuring.
    ///
    /// Named `count` rather than `get` because the generated RPC client already
    /// exposes `CounterClient::get(name)` as its constructor — an agent method
    /// called `get` shadows it and breaks every client call site.
    fn count(&self) -> u32;

    /// Waits `millis`, then increments, returning the post-increment count.
    ///
    /// The in-flight chaos scenarios need an operation that is still running
    /// when a pod is killed. Sleeping rather than spinning is the point:
    /// `busy_for` would pin a core per concurrent operation, which turns a
    /// crash-recovery experiment into a saturation experiment.
    ///
    /// The increment lands *after* the wait, so the state change falls inside
    /// the fault window rather than before it — that is the mutation whose
    /// exactly-once behaviour is under test.
    fn sleep_and_increment(&mut self, millis: u32) -> u32;
}

struct CounterImpl {
    count: u32,
    id: String,
    retained: Vec<u8>,
}

#[agent_implementation]
impl Counter for CounterImpl {
    fn new(id: String) -> Self {
        Self {
            id,
            count: 0,
            retained: Vec::new(),
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    async fn increment_through_rpc(&mut self) -> u32 {
        let mut client = CounterClient::get(format!("{}-inner", self.id));
        client.increment().await
    }

    async fn increment_through_rpc_to_ephemeral(&mut self) -> u32 {
        let mut client = EphemeralCounterClient::new_phantom(format!("{}-ephemeral", self.id));
        client.increment().await
    }

    async fn increment_through_rpc_to_ephemeral_phantom(&mut self) -> u32 {
        let mut client = EphemeralSingletonCounterClient::new_phantom();
        client.increment().await
    }

    fn busy_for(&mut self, millis: u32) -> u32 {
        let _ = busy_loop(millis);
        self.count += 1;
        self.count
    }

    fn allocate_memory(&mut self, bytes: u32) -> u32 {
        retain_memory(&mut self.retained, bytes);
        self.count += 1;
        self.count
    }

    fn oplog_heavy(&mut self, entries: u32) -> u32 {
        for _ in 0..entries {
            let mut buf = [0u8; 4];
            wstd::rand::get_random_bytes(&mut buf);
            self.count = self.count.wrapping_add(u32::from_le_bytes(buf));
        }
        self.count
    }

    fn count(&self) -> u32 {
        self.count
    }

    fn sleep_and_increment(&mut self, millis: u32) -> u32 {
        std::thread::sleep(std::time::Duration::from_millis(millis as u64));
        self.count += 1;
        self.count
    }
}

/// Counter that holds a quota token for its whole lifetime (GOL-364).
///
/// The point is the *lease*, not the counting. Acquiring a `QuotaToken` makes
/// the executor take a quota lease from the shard-manager and keep renewing it
/// — on golem-dev, every 10s against a 60s lease. Holding the token in agent
/// state keeps that lease live, which is what puts continuous traffic on the
/// executor→shard-manager link.
///
/// That link is otherwise idle during a chaos run: `invoke_and_await` goes
/// client → worker-service → executor and never touches the shard-manager, and
/// the shard-manager's health check asks the Kubernetes API rather than the
/// executor. A partition between the two is invisible to everything else, which
/// is exactly why S1's first three runs measured nothing.
#[agent_definition]
trait QuotaCounter {
    fn new(id: String) -> Self;

    /// Reserves and commits one unit against the held token, then increments.
    ///
    /// Returns the post-increment count, like `Counter.increment`, so the same
    /// read-back and exactly-once machinery applies unchanged. A reservation
    /// the platform refuses is reported rather than panicking: under a
    /// partition, refusal is a legitimate outcome and the scenario needs to
    /// record it, not die of it.
    fn reserve_and_increment(&mut self) -> u32;

    /// How many reservations were refused. Read back after the run to see what
    /// losing the lease actually cost.
    fn refused(&self) -> u32;

    fn count(&self) -> u32;
}

struct QuotaCounterImpl {
    _id: String,
    count: u32,
    refused: u32,
    /// Held for the agent's lifetime — dropping it would release the lease and
    /// stop the renewal traffic this agent exists to generate.
    token: QuotaToken,
}

#[agent_implementation]
impl QuotaCounter for QuotaCounterImpl {
    fn new(id: String) -> Self {
        Self {
            _id: id,
            count: 0,
            refused: 0,
            token: QuotaToken::new(CHAOS_QUOTA_RESOURCE, 1),
        }
    }

    fn reserve_and_increment(&mut self) -> u32 {
        match self.token.reserve(1) {
            Ok(reservation) => {
                reservation.commit(1);
                self.count += 1;
            }
            Err(_) => {
                self.refused += 1;
            }
        }
        self.count
    }

    fn refused(&self) -> u32 {
        self.refused
    }

    fn count(&self) -> u32 {
        self.count
    }
}

#[agent_definition(ephemeral)]
trait EphemeralCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;

    /// See [`Counter::busy_for`].
    fn busy_for(&mut self, millis: u32) -> u32;

    /// See [`Counter::allocate_memory`].
    fn allocate_memory(&mut self, bytes: u32) -> u32;
}

struct EphemeralCounterImpl {
    count: u32,
    _id: String,
    retained: Vec<u8>,
}

#[agent_implementation]
impl EphemeralCounter for EphemeralCounterImpl {
    fn new(id: String) -> Self {
        Self {
            _id: id,
            count: 0,
            retained: Vec::new(),
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn busy_for(&mut self, millis: u32) -> u32 {
        let _ = busy_loop(millis);
        self.count += 1;
        self.count
    }

    fn allocate_memory(&mut self, bytes: u32) -> u32 {
        retain_memory(&mut self.retained, bytes);
        self.count += 1;
        self.count
    }
}

#[agent_definition(ephemeral)]
trait EphemeralSingletonCounter {
    fn new() -> Self;
    fn increment(&mut self) -> u32;
}

struct EphemeralSingletonCounterImpl {
    count: u32,
}

#[agent_implementation]
impl EphemeralSingletonCounter for EphemeralSingletonCounterImpl {
    fn new() -> Self {
        Self { count: 0 }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}

/// Near-no-op target for schedule-density. The scheduled action under test is
/// dispatching this method, not its guest-side work.
#[agent_definition]
trait ScheduleCounter {
    fn new(id: String) -> Self;

    /// Counts the fire rather than doing nothing at all: chaos scenarios need
    /// some durable trace that a scheduled action actually landed, and the
    /// increment is far cheaper than anything the dispatch itself costs.
    fn poll(&mut self);

    /// How many times `poll` has fired. Read after recovery to compare against
    /// the number of actions the driver scheduled.
    fn polls(&self) -> u32;
}

struct ScheduleCounterImpl {
    _id: String,
    polls: u32,
}

#[agent_implementation]
impl ScheduleCounter for ScheduleCounterImpl {
    fn new(id: String) -> Self {
        Self { _id: id, polls: 0 }
    }

    fn poll(&mut self) {
        self.polls += 1;
    }

    fn polls(&self) -> u32 {
        self.polls
    }
}

/// Schedules no-op polls on durable targets. Keeping scheduling separate from
/// the target lets the benchmark prepare target residency before registration.
#[agent_definition]
trait ScheduleEmitter {
    fn new(id: String) -> Self;
    fn warm(&self);
    fn schedule_poll_at(
        &self,
        target_name: String,
        seconds: u64,
        nanoseconds: u32,
        context_spans: u32,
    );
}

struct ScheduleEmitterImpl {
    _id: String,
}

#[agent_implementation]
impl ScheduleEmitter for ScheduleEmitterImpl {
    fn new(id: String) -> Self {
        Self { _id: id }
    }

    fn warm(&self) {}

    fn schedule_poll_at(
        &self,
        target_name: String,
        seconds: u64,
        nanoseconds: u32,
        context_spans: u32,
    ) {
        let _spans: Vec<_> = (0..context_spans)
            .map(|_| start_span("schedule-density"))
            .collect();
        let mut target = ScheduleCounterClient::get(target_name);
        target.schedule_poll(Datetime {
            seconds,
            nanoseconds,
        });
    }
}

#[agent_definition(ephemeral)]
trait HostFunctionTests {
    fn new(id: String) -> Self;
    fn generate_idempotency_keys(&mut self) -> (String, String);
}

struct HostFunctionTestsImpl {
    _id: String,
}

#[agent_implementation]
impl HostFunctionTests for HostFunctionTestsImpl {
    fn new(id: String) -> Self {
        Self { _id: id }
    }

    fn generate_idempotency_keys(&mut self) -> (String, String) {
        let key1 = generate_idempotency_key();
        let key2 = generate_idempotency_key();
        (key1.to_string(), key2.to_string())
    }
}

#[agent_definition]
trait FailingCounter {
    fn new(id: String) -> Self;
    fn add(&mut self, value: u64);
    fn get(&self) -> u64;
}

struct FailingCounterImpl {
    total: u64,
    _id: String,
}

#[agent_implementation]
impl FailingCounter for FailingCounterImpl {
    fn new(id: String) -> Self {
        Self { total: 0, _id: id }
    }

    fn add(&mut self, value: u64) {
        eprintln!("error log message");
        if value > 10 {
            panic!("value is too large");
        }
        self.total += value;
    }

    fn get(&self) -> u64 {
        self.total
    }
}
