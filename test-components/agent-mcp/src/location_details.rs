use golem_rust::{FromSchema, IntoSchema, IntoWire};

#[derive(IntoSchema, FromSchema, IntoWire)]
pub struct LocationDetails {
    pub lat: f64,
    pub long: f64,
    pub country: String,
    pub population: u64,
}
