use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait RustBenchmarkAgent {
    fn new(name: String) -> Self;
    fn echo(&mut self, message: String) -> String;
    fn large_input(&mut self, input: Vec<u8>) -> u32;
    fn cpu_intensive(&mut self, length: f64) -> u32; // length is f64 to have the same interface as the TS agent
    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32;
    fn sleep(&mut self, millis: u64) -> bool;
}

struct RustBenchmarkAgentImpl {
    name: String,
}

#[agent_implementation]
impl RustBenchmarkAgent for RustBenchmarkAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn echo(&mut self, message: String) -> String {
        common_lib::echo(message)
    }

    fn large_input(&mut self, input: Vec<u8>) -> u32 {
        common_lib::large_input(input)
    }

    fn cpu_intensive(&mut self, length: f64) -> u32 {
        common_lib::cpu_intensive(length as u32)
    }

    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32 {
        common_lib::oplog_heavy(length, persistence_on, commit)
    }

    fn sleep(&mut self, millis: u64) -> bool {
        let duration = std::time::Duration::from_millis(millis);
        std::thread::sleep(duration);
        true
    }
}

#[agent_definition(ephemeral)]
trait RustEphemeralBenchmarkAgent {
    fn new(name: String) -> Self;
    fn echo(&mut self, message: String) -> String;
    fn large_input(&mut self, input: Vec<u8>) -> u32;
    fn cpu_intensive(&mut self, length: f64) -> u32;
    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32;
}

struct RustEphemeralBenchmarkAgentImpl {
    name: String,
}

#[agent_implementation]
impl RustEphemeralBenchmarkAgent for RustEphemeralBenchmarkAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn echo(&mut self, message: String) -> String {
        common_lib::echo(message)
    }

    fn large_input(&mut self, input: Vec<u8>) -> u32 {
        common_lib::large_input(input)
    }

    fn cpu_intensive(&mut self, length: f64) -> u32 {
        common_lib::cpu_intensive(length as u32)
    }

    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32 {
        common_lib::oplog_heavy(length, persistence_on, commit)
    }
}

#[agent_definition]
trait RustRpcBenchmarkAgent {
    fn new(name: String) -> Self;
    async fn echo(&mut self, message: String) -> String;
    async fn large_input(&mut self, input: Vec<u8>) -> u32;
    async fn cpu_intensive(&mut self, length: f64) -> u32; // length is f64 to have the same interface as the TS agent
    async fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32;
}

struct RustRpcBenchmarkAgentImpl {
    name: String,
}

#[agent_implementation]
impl RustRpcBenchmarkAgent for RustRpcBenchmarkAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    async fn echo(&mut self, message: String) -> String {
        let mut client = RustBenchmarkAgentClient::get(self.name.clone());
        client.echo(message).await
    }

    async fn large_input(&mut self, input: Vec<u8>) -> u32 {
        let mut client = RustBenchmarkAgentClient::get(self.name.clone());
        client.large_input(input).await
    }

    async fn cpu_intensive(&mut self, length: f64) -> u32 {
        let mut client = RustBenchmarkAgentClient::get(self.name.clone());
        client.cpu_intensive(length).await
    }

    async fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32 {
        let mut client = RustBenchmarkAgentClient::get(self.name.clone());
        client.oplog_heavy(length, persistence_on, commit).await
    }
}
