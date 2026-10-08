use golem_rust as _;

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod tests;

#[cfg(feature = "agent")]
mod agent {
    use golem_rust::{agent_definition, agent_implementation};

    #[agent_definition]
    trait MinimalAgent {
        fn new(name: String) -> Self;
        fn hello(&self) -> String;
    }

    struct Agent(String);

    #[agent_implementation]
    impl MinimalAgent for Agent {
        fn new(name: String) -> Self {
            Self(name)
        }

        fn hello(&self) -> String {
            format!("hello {}", self.0)
        }
    }
}

#[cfg(feature = "tool")]
mod tool {
    use golem_rust::tool_implementation;
    use middleware_definition::{PublicEcho, PublicError};

    struct Echo;

    #[tool_implementation]
    impl PublicEcho for Echo {
        fn echo(&self, value: String) -> Result<String, PublicError> {
            Ok(format!("echo:{value}"))
        }
    }
}

#[cfg(feature = "middleware")]
mod middleware {
    use golem_rust::{tool::ToolInvokeError, tool_middleware};
    use middleware_definition::{PublicEchoMiddleware, PublicEchoUnderlying, PublicError};

    struct Policy;

    impl Policy {
        fn new() -> Self {
            Self
        }
    }

    #[tool_middleware(name = "minimal-policy", constructor = Policy::new)]
    impl PublicEchoMiddleware for Policy {
        async fn echo(
            &self,
            underlying: &PublicEchoUnderlying,
            value: String,
        ) -> Result<String, ToolInvokeError<PublicError>> {
            underlying.echo(format!("policy:{value}")).await
        }
    }
}
