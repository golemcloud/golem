use golem_rust::tool_definition;

// A `#[arg(...)]` must bind to a real method parameter. `missing` does not name
// any parameter of `grep`, so this is a build-time error.
#[tool_definition]
trait Grep {
    #[arg(missing = "option")]
    fn grep(&self, pattern: String) -> Result<(), ()>;
}

fn main() {}
