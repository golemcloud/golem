use golem_rust::Schema;

#[derive(Schema)]
pub struct LocationDetails {
    pub lat: f64,
    pub long: f64,
    pub country: String,
    pub population: u64
}