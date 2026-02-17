mod snapshot_test;
pub mod repository;

use golem_rust::{agent_definition, agent_implementation, generate_idempotency_key};

#[agent_definition]
trait Counter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    async fn increment_through_rpc(&mut self) -> u32;
    async fn increment_through_rpc_to_ephemeral(&mut self) -> u32;
    async fn increment_through_rpc_to_ephemeral_phantom(&mut self) -> u32;
}

struct CounterImpl {
    count: u32,
    id: String,
}

#[agent_implementation]
impl Counter for CounterImpl {
    fn new(id: String) -> Self {
        Self { id, count: 0 }
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
        let mut client = EphemeralCounterClient::get(format!("{}-ephemeral", self.id));
        client.increment().await
    }

    async fn increment_through_rpc_to_ephemeral_phantom(&mut self) -> u32 {
        let mut client = EphemeralSingletonCounterClient::new_phantom();
        client.increment().await
    }
}

#[agent_definition(ephemeral)]
trait EphemeralCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
}

struct EphemeralCounterImpl {
    count: u32,
    _id: String,
}

#[agent_implementation]
impl EphemeralCounter for EphemeralCounterImpl {
    fn new(id: String) -> Self {
        Self { _id: id, count: 0 }
    }

    fn increment(&mut self) -> u32 {
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
    count: u32
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
