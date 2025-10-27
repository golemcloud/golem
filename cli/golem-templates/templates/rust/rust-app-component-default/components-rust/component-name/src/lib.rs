use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait Foo {
    fn new(input: String) -> Self;
    fn foo(&self) -> String;
}

struct FooImpl {
    input: String,
}

#[agent_implementation]
impl Foo for FooImpl {
    fn new(input: String) -> Self {
        FooImpl { input }
    }
    fn foo(&self) -> String {
        self.input.clone()
    }
}
