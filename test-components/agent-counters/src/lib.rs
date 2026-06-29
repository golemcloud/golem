pub mod repository;
mod snapshot_test;

use golem_rust::{agent_definition, agent_implementation, generate_idempotency_key};

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
