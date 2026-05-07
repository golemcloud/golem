use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait SameCrateAgent {
    fn new(id: String) -> Self;
    fn ping(&self) -> String;
}

struct SameCrateAgentImpl {
    id: String,
}

#[agent_implementation]
impl SameCrateAgent for SameCrateAgentImpl {
    fn new(id: String) -> Self {
        Self { id }
    }

    fn ping(&self) -> String {
        self.id.clone()
    }
}

fn main() {}
