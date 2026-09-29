use golem_rust::{FromWire, IntoWire, Schema, WireSchema};

pub type WorkflowId = String;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    Schema,
    FromWire,
    IntoWire,
    WireSchema,
)]
pub enum Decision {
    Approved,
    Rejected,
}
