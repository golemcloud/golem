use golem_rust::agent_definition;

#[agent_definition]
trait MultiAgent {
    fn new(id: String) -> Self;
    fn ping(&self) -> String;
}

struct FirstImpl;
struct SecondImpl;

impl MultiAgent for FirstImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn ping(&self) -> String {
        "first".to_string()
    }
}

impl MultiAgent for SecondImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn ping(&self) -> String {
        "second".to_string()
    }
}

fn main() {}
