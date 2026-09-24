use golem_rust::{agent_definition, endpoint};

#[agent_definition(mount = "/regular-any")]
trait RegularAgentWithAnyEndpoint {
    fn new() -> Self;

    #[endpoint(any = "/")]
    fn route(&self, request: String) -> String;
}

fn main() {}
