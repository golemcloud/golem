use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait Counter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    async fn increment_through_rpc(&mut self) -> u32;
    async fn increment_through_rpc_to_ephemeral(&mut self) -> u32;
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
