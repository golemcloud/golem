use golem_rust::agent_client;

#[agent_client(type_name = "NamedOnly")]
trait NamedOnly {
    fn call(&self);
}

#[agent_client]
trait ConstructorOnly {
    fn new(id: String) -> Self;
    fn call(&self);
}

#[agent_client(mode = "ephemeral")]
trait ModeOnly {
    fn call(&self);
}

#[agent_client(type_name = "TooMany")]
trait TooMany {
    fn first(id: String) -> Self;
    fn second(id: u64) -> Self;
    fn call(&self);
}

fn main() {}
