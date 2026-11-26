use golem_rust::TypedSchemaValue;
use golem_rust::tool::{
    InputStream, InvocationContext, OutputStream, Tool, ToolMiddleware, ToolMiddlewareInvokeFuture,
    ToolMiddlewareScope, UnderlyingTool,
};

#[cfg(feature = "provider")]
mod provider {
    use golem_rust::{tool_definition, tool_implementation};

    #[tool_definition(version = "1.0.0")]
    pub trait ToolX {
        async fn run(&self, value: String) -> String;
    }

    struct X;

    #[tool_implementation]
    impl ToolX for X {
        async fn run(&self, value: String) -> String {
            value
        }
    }

    #[tool_definition(version = "1.0.0")]
    pub trait ToolY {
        async fn run(&self, value: String) -> String;
    }

    struct Y;

    #[tool_implementation]
    impl ToolY for Y {
        async fn run(&self, value: String) -> String {
            value
        }
    }
}

fn invoke(
    _name: String,
    _tool: Tool,
    _parameters: TypedSchemaValue,
    command: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    _context: InvocationContext,
    underlying: UnderlyingTool,
) -> ToolMiddlewareInvokeFuture {
    Box::pin(async move {
        underlying
            .invoke_forwarding_outputs(command, input, stdin, stdout, stderr)
            .await
    })
}

fn register(name: &str, aliases: &[&str]) {
    golem_rust::tool::register_tool_middleware(
        ToolMiddleware {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            doc: Default::default(),
            scope: ToolMiddlewareScope::Universal,
            parameter_schema: golem_rust::schema::try_into_schema_graph::<()>().unwrap(),
        },
        invoke,
    );
}

golem_rust::ctor::__support::ctor_parse!(
    #[ctor]
    fn register_middlewares() {
        golem_rust::tool::install_middleware_exports();
        #[cfg(feature = "x")]
        register("layer-x", &[]);
        #[cfg(feature = "y")]
        register("layer-y", &[]);
        #[cfg(feature = "alias-x")]
        register("late-layer", &["layer-x"]);
    }
);
