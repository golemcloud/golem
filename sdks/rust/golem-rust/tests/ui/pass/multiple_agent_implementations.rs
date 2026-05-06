use golem_rust::agent_definition;

#[agent_definition]
trait MultiAgent {
    fn new(id: String) -> Self;
    fn ping(&self) -> String;
}

mod first {
    use super::MultiAgent;
    use golem_rust::agent_implementation;

    pub struct FirstImpl;

    #[agent_implementation]
    impl MultiAgent for FirstImpl {
        fn new(_id: String) -> Self {
            Self
        }

        fn ping(&self) -> String {
            "first".to_string()
        }
    }
}

mod second {
    use super::MultiAgent;
    use golem_rust::agent_implementation;

    pub struct SecondImpl;

    #[agent_implementation]
    impl MultiAgent for SecondImpl {
        fn new(_id: String) -> Self {
            Self
        }

        fn ping(&self) -> String {
            "second".to_string()
        }
    }
}

fn main() {}
