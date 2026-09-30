use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait Reflection {
    fn new() -> Self;
    fn has_agent_type(&self, name: String) -> bool;
}

struct ReflectionImpl;

#[agent_implementation]
impl Reflection for ReflectionImpl {
    fn new() -> Self {
        Self
    }

    #[inline(never)]
    fn has_agent_type(&self, name: String) -> bool {
        golem_rust::golem_agentic::golem::agent::host::get_agent_type(&name).is_some()
    }
}
