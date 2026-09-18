use golem_rust::{agent_definition, endpoint};

#[agent_definition(kind = "http-router", mount = "/raw")]
trait InvalidAnyEndpointAgent {
    fn new() -> Self;

    #[endpoint(any = 42)]
    fn route(&self, request: String) -> String;
}

fn main() {}
