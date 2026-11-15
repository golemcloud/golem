use golem_rust::agentic::InvocationContext;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait Greeter {
    fn new(ctx: InvocationContext, name: String) -> Self;
    fn greet(&self, ctx: InvocationContext, greeting: String) -> String;
    fn plain(&self, greeting: String) -> String;
}

struct GreeterImpl {
    name: String,
    created_by: String,
}

#[agent_implementation]
impl Greeter for GreeterImpl {
    fn new(ctx: InvocationContext, name: String) -> Self {
        Self {
            name,
            created_by: ctx.caller().original().to_string(),
        }
    }

    fn greet(&self, ctx: InvocationContext, greeting: String) -> String {
        format!(
            "{greeting}, {} (made by {}, called by {})",
            self.name,
            self.created_by,
            ctx.caller().immediate()
        )
    }

    fn plain(&self, greeting: String) -> String {
        greeting
    }
}

fn main() {}
