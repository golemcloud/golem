use golem_rust::{FromSchema, FromWire, IntoSchema, IntoWire, WireSchema};

#[derive(IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct LocationDetails {
    pub lat: f64,
    pub long: f64,
    pub country: String,
    pub population: u64,
}
