use golem_rust::agent_definition;

struct CollisionAgentInvocationResult;

#[agent_definition(ephemeral)]
trait CollisionAgent {
    fn new() -> Self;
    fn run(&self) -> String;
}

fn main() {}
