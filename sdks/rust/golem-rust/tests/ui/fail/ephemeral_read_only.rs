use golem_rust::{agent_definition, agent_implementation, read_only};

#[agent_definition(ephemeral)]
trait EphemeralReadOnlyAgent {
    fn new() -> Self;

    #[read_only]
    fn get_value(&self) -> u32;
}

struct EphemeralReadOnlyAgentImpl;

#[agent_implementation]
impl EphemeralReadOnlyAgent for EphemeralReadOnlyAgentImpl {
    fn new() -> Self {
        Self
    }

    fn get_value(&self) -> u32 {
        0
    }
}

fn main() {}
