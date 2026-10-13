use golem_rust::Schema;

pub type WorkflowId = String;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Schema)]
pub enum Decision {
    Approved,
    Rejected,
}
