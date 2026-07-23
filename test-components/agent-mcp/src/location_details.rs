use golem_rust::{FromSchema, IntoSchema};

#[derive(IntoSchema, FromSchema)]
pub struct LocationDetails {
    pub lat: f64,
    pub long: f64,
    pub country: String,
    pub population: u64,
}
