use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait BenchmarkAgent {
    fn new(name: String) -> Self;
    fn echo(&mut self, message: String) -> String;
    fn large_input(&mut self, input: Vec<u8>) -> u32;
    fn cpu_intensive(&mut self, length: u32) -> u32;
    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32;
}

struct BenchmarkAgentImpl {
    name: String,
}

#[agent_implementation]
impl BenchmarkAgent for BenchmarkAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn echo(&mut self, message: String) -> String {
        common_lib::echo(message)
    }

    fn large_input(&mut self, input: Vec<u8>) -> u32 {
        common_lib::large_input(input)
    }

    fn cpu_intensive(&mut self, length: u32) -> u32 {
        common_lib::cpu_intensive(length)
    }

    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32 {
        common_lib::oplog_heavy(length, persistence_on, commit)
    }
}

#[agent_definition(ephemeral)]
trait EphemeralBenchmarkAgent {
    fn new(name: String) -> Self;
    fn echo(&mut self, message: String) -> String;
    fn large_input(&mut self, input: Vec<u8>) -> u32;
    fn cpu_intensive(&mut self, length: u32) -> u32;
    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32;
}

struct EphemeralBenchmarkAgentImpl {
    name: String,
}

#[agent_implementation]
impl EphemeralBenchmarkAgent for EphemeralBenchmarkAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn echo(&mut self, message: String) -> String {
        common_lib::echo(message)
    }

    fn large_input(&mut self, input: Vec<u8>) -> u32 {
        common_lib::large_input(input)
    }

    fn cpu_intensive(&mut self, length: u32) -> u32 {
        common_lib::cpu_intensive(length)
    }

    fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32 {
        common_lib::oplog_heavy(length, persistence_on, commit)
    }
}

#[agent_definition]
trait RpcBenchmarkAgent {
    fn new(name: String) -> Self;
    async fn echo(&mut self, message: String) -> String;
    async fn large_input(&mut self, input: Vec<u8>) -> u32;
    async fn cpu_intensive(&mut self, length: u32) -> u32;
    async fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32;
}

struct RpcBenchmarkAgentImpl {
    name: String,
}

#[agent_implementation]
impl RpcBenchmarkAgent for RpcBenchmarkAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    async fn echo(&mut self, message: String) -> String {
        let mut client = BenchmarkAgentClient::get(self.name.clone());
        client.echo(message).await
    }

    async fn large_input(&mut self, input: Vec<u8>) -> u32 {
        let mut client = BenchmarkAgentClient::get(self.name.clone());
        client.large_input(input).await
    }

    async fn cpu_intensive(&mut self, length: u32) -> u32 {
        let mut client = BenchmarkAgentClient::get(self.name.clone());
        client.cpu_intensive(length).await
    }

    async fn oplog_heavy(&mut self, length: u32, persistence_on: bool, commit: bool) -> u32 {
        let mut client = BenchmarkAgentClient::get(self.name.clone());
        client.oplog_heavy(length, persistence_on, commit).await
    }
}
