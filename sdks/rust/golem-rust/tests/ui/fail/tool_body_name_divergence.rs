use golem_rust::tool_definition;

// §5.8.1: the implicit-body method (named after the tool) must not rename the
// root command away from the tool name. `grep` is the implicit body of tool
// `grep`, so `#[command(name = "search")]` is a build-time error.
#[tool_definition]
trait Grep {
    #[command(name = "search")]
    fn grep(&self, pattern: String) -> Result<(), ()>;
}

fn main() {}
