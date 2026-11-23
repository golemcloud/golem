use golem_rust::IntoSchema;
use golem_rust::agentic::Caller;

#[derive(IntoSchema)]
struct Audit {
    who: Caller,
}

fn main() {
    let _ = golem_rust::schema::try_into_schema_graph::<Audit>();
}
