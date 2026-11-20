use golem_rust::IntoSchema;
use golem_rust::agentic::TokenProvenance;

#[derive(IntoSchema)]
struct Audit {
    tokens: Vec<TokenProvenance>,
}

fn main() {
    let _ = golem_rust::schema::try_into_schema_graph::<Audit>();
}
