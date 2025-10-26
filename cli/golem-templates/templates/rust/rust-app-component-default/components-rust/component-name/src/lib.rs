use golem_rust::agent_definition;

#[agent_definition]
pub trait Foo {
    fn foo(&self) -> String;
}
