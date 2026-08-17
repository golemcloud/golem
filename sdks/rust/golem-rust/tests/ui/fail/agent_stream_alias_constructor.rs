use golem_rust::agent_definition;
use golem_rust::agentic::AgentStream;

type ConstructorInput<T> = AgentStream<T>;

#[agent_definition]
trait AliasedStreamConstructorAgent {
    fn new(input: ConstructorInput<String>) -> Self;
}

fn main() {}
