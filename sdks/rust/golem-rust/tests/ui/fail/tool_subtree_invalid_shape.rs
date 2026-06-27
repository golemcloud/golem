use golem_rust::tool_definition;

// A `#[command(subtree = ...)]` method is a pure dispatcher: the model places a
// result/constraints on a command body, not on a subtree graft. Attaching
// `#[result(...)]` to a subtree method is an invalid subtree descriptor shape
// and a build-time error.
#[tool_definition]
trait Git {
    #[command(subtree = Remote)]
    #[result(formatters = ["json"], default = "json")]
    fn remote(&self) -> RemoteSubtree;
}

fn main() {}
