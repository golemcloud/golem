use golem_rust::agentic::Principal;
use golem_rust::{FromSchema, FromWire, IntoSchema, IntoWire, WireSchema};

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
struct Audit {
    who: Principal,
}

fn main() {
    let _ = |audit: &Audit| audit.who.kind();
}
