use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::rib::VariableId as ProtoVariableId;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Hash, Eq, Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum VariableId {
    Global(String),
    Local(String, Option<Id>),
    MatchIdentifier(MatchIdentifier),
}

#[derive(Hash, Eq, Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct MatchIdentifier {
    pub name: String,
    pub match_arm_index: usize, // Every match arm across the program is identified by a non-sharing index value. Within a match arm the identifier names cannot be reused
}

impl MatchIdentifier {
    pub fn new(name: String, match_arm_index: usize) -> MatchIdentifier {
        MatchIdentifier {
            name,
            match_arm_index,
        }
    }
}

impl Display for VariableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VariableId::Global(name) => write!(f, "{}", name),
            VariableId::Local(name, _) => write!(f, "{}", name),
            VariableId::MatchIdentifier(m) => write!(f, "{}", m.name),
        }
    }
}
#[derive(Hash, Eq, Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Id(u32);

impl VariableId {
    pub fn match_identifier(name: String, match_arm_index: usize) -> VariableId {
        VariableId::MatchIdentifier(MatchIdentifier {
            name,
            match_arm_index,
        })
    }
    pub fn name(&self) -> String {
        match self {
            VariableId::Global(name) => name.clone(),
            VariableId::Local(name, _) => name.clone(),
            VariableId::MatchIdentifier(m) => m.name.clone(),
        }
    }

    pub fn is_global(&self) -> bool {
        match self {
            VariableId::Global(_) => true,
            VariableId::Local(_, _) => false,
            VariableId::MatchIdentifier { .. } => false,
        }
    }

    pub fn is_match_binding(&self) -> bool {
        match self {
            VariableId::Global(_) => false,
            VariableId::Local(_, _) => false,
            VariableId::MatchIdentifier { .. } => true,
        }
    }

    // Default variable_id could global, but as soon as type inference
    // identifies them to be local it gets converted to a local with an id
    pub fn global(variable_name: String) -> VariableId {
        VariableId::Global(variable_name)
    }

    pub fn local(variable_name: &str, id: u32) -> VariableId {
        VariableId::Local(variable_name.to_string(), Some(Id(id)))
    }

    // A local variable can be directly formed during parsing itself.
    // For example: all identifiers in the LHS of a pattern-match-arm
    // don't have a local definition of the variable, yet they are considered to be local
    pub fn local_with_no_id(name: &str) -> VariableId {
        VariableId::Local(name.to_string(), None)
    }

    pub fn increment_local_variable_id(&mut self) -> VariableId {
        match self {
            VariableId::Global(name) => VariableId::Local(name.clone(), Some(Id(0))),
            VariableId::Local(name, id) => {
                let new_id = id.clone().map_or(Some(Id(0)), |x| Some(Id(x.0 + 1)));
                *id = new_id.clone();
                VariableId::Local(name.to_string(), new_id)
            }
            VariableId::MatchIdentifier(m) => VariableId::MatchIdentifier(m.clone()),
        }
    }
}

impl TryFrom<ProtoVariableId> for VariableId {
    type Error = String;

    fn try_from(value: ProtoVariableId) -> Result<Self, Self::Error> {
        let variable_id = value.variable_id.ok_or("Missing variable_id".to_string())?;

        match variable_id {
            golem_api_grpc::proto::golem::rib::variable_id::VariableId::Global(global) => {
                Ok(VariableId::Global(global.name))
            }
            golem_api_grpc::proto::golem::rib::variable_id::VariableId::Local(local) => Ok(
                VariableId::Local(local.name, local.id.map(|x| Id(x as u32))),
            ),
        }
    }
}

impl From<VariableId> for ProtoVariableId {
    fn from(value: VariableId) -> Self {
        match value {
            VariableId::Global(name) => ProtoVariableId {
                variable_id: Some(
                    golem_api_grpc::proto::golem::rib::variable_id::VariableId::Global(
                        golem_api_grpc::proto::golem::rib::Global { name },
                    ),
                ),
            },
            VariableId::MatchIdentifier(m) => ProtoVariableId {
                variable_id: Some(
                    golem_api_grpc::proto::golem::rib::variable_id::VariableId::Global(
                        golem_api_grpc::proto::golem::rib::Global { name: m.name },
                    ),
                ),
            },
            VariableId::Local(name, id) => ProtoVariableId {
                variable_id: Some(
                    golem_api_grpc::proto::golem::rib::variable_id::VariableId::Local(
                        golem_api_grpc::proto::golem::rib::Local {
                            name,
                            id: id.map(|x| x.0 as u64),
                        },
                    ),
                ),
            },
        }
    }
}
