use golem_rust::agentic::Principal;
use golem_rust::{FromSchema, FromWire, IntoSchema, IntoWire, WireSchema};

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
struct Audit {
    who: Principal,
}

fn main() {
    let _ = golem_rust::schema::try_into_schema_graph::<Audit>();
    let _ = golem_rust::schema::wit::direct::schema::<Audit>();
}
