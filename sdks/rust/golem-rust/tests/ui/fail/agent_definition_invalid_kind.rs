use golem_rust::agent_definition;

#[agent_definition(kind = "router")]
trait InvalidKindAgent {
    fn new() -> Self;
}

fn main() {}
