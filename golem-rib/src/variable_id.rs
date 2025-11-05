// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use serde::{Deserialize, Serialize};
use std::fmt::Display;
use desert_rust::BinaryCodec;

#[derive(
    Hash, Eq, Debug, Clone, PartialEq, Ord, PartialOrd, Serialize, Deserialize, BinaryCodec,
)]
#[desert(evolution())]
pub enum VariableId {
    Global(String),
    Local(String, Option<Id>),
    MatchIdentifier(MatchIdentifier),
    ListComprehension(ListComprehensionIdentifier),
    ListReduce(ListAggregationIdentifier),
}

impl VariableId {
    pub fn as_instance_variable(&self) -> VariableId {
        let variable_string = match self {
            VariableId::Global(name) => name.clone(),
            VariableId::Local(name, identifier) => {
                if let Some(id) = identifier {
                    format!("{}-{}", name, id.0)
                } else {
                    name.clone()
                }
            }
            VariableId::MatchIdentifier(m) => format!("{}-{}", m.name, m.match_arm_index),
            VariableId::ListComprehension(l) => l.name.clone(),
            VariableId::ListReduce(r) => r.name.clone(),
        };

        VariableId::global(format!("__instance_{variable_string}"))
    }
    pub fn list_comprehension_identifier(name: impl AsRef<str>) -> VariableId {
        VariableId::ListComprehension(ListComprehensionIdentifier {
            name: name.as_ref().to_string(),
        })
    }

    pub fn list_reduce_identifier(name: impl AsRef<str>) -> VariableId {
        VariableId::ListReduce(ListAggregationIdentifier {
            name: name.as_ref().to_string(),
        })
    }

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
            VariableId::ListComprehension(l) => l.name.clone(),
            VariableId::ListReduce(r) => r.name.clone(),
        }
    }

    pub fn is_global(&self) -> bool {
        match self {
            VariableId::Global(_) => true,
            VariableId::Local(_, _) => false,
            VariableId::MatchIdentifier(_) => false,
            VariableId::ListComprehension(_) => false,
            VariableId::ListReduce(_) => false,
        }
    }

    pub fn is_local(&self) -> bool {
        match self {
            VariableId::Global(_) => false,
            VariableId::Local(_, _) => true,
            VariableId::MatchIdentifier(_) => false,
            VariableId::ListComprehension(_) => false,
            VariableId::ListReduce(_) => false,
        }
    }

    pub fn is_match_binding(&self) -> bool {
        match self {
            VariableId::Global(_) => false,
            VariableId::Local(_, _) => false,
            VariableId::MatchIdentifier(_) => true,
            VariableId::ListComprehension(_) => false,
            VariableId::ListReduce(_) => false,
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
            VariableId::ListComprehension(l) => VariableId::ListComprehension(l.clone()),
            VariableId::ListReduce(l) => VariableId::ListReduce(l.clone()),
        }
    }
}

#[derive(
    Hash, Eq, Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, Ord, PartialOrd,
)]
#[desert(evolution())]
pub struct ListComprehensionIdentifier {
    pub name: String,
}

#[derive(
    Hash, Eq, Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, Ord, PartialOrd,
)]
#[desert(evolution())]
pub struct ListAggregationIdentifier {
    pub name: String,
}

#[derive(
    Hash, Eq, Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, Ord, PartialOrd,
)]
#[desert(evolution())]
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
            VariableId::Global(name) => write!(f, "{name}"),
            VariableId::Local(name, _) => write!(f, "{name}"),
            VariableId::MatchIdentifier(m) => write!(f, "{}", m.name),
            VariableId::ListComprehension(l) => write!(f, "{}", l.name),
            VariableId::ListReduce(r) => write!(f, "{}", r.name),
        }
    }
}
#[derive(
    Hash, Eq, Debug, Clone, PartialEq, Serialize, Deserialize, BinaryCodec, Ord, PartialOrd,
)]
#[desert(transparent)]
pub struct Id(pub(crate) u32);

mod protobuf {
    use crate::proto::golem::rib::VariableId as ProtoVariableId;
    use crate::{Id, VariableId};

    impl TryFrom<ProtoVariableId> for VariableId {
        type Error = String;

        fn try_from(value: ProtoVariableId) -> Result<Self, Self::Error> {
            let variable_id = value.variable_id.ok_or("Missing variable_id".to_string())?;

            match variable_id {
                crate::proto::golem::rib::variable_id::VariableId::Global(global) => {
                    Ok(VariableId::Global(global.name))
                }
                crate::proto::golem::rib::variable_id::VariableId::Local(local) => Ok(
                    VariableId::Local(local.name, local.id.map(|x| Id(x as u32))),
                ),
                crate::proto::golem::rib::variable_id::VariableId::MatchIdentifier(
                    match_identifier,
                ) => Ok(VariableId::MatchIdentifier(crate::MatchIdentifier {
                    name: match_identifier.name,
                    match_arm_index: match_identifier.match_arm_index as usize,
                })),
                crate::proto::golem::rib::variable_id::VariableId::ListComprehensionIdentifier(
                    list_comprehension,
                ) => Ok(VariableId::ListComprehension(
                    crate::ListComprehensionIdentifier {
                        name: list_comprehension.name,
                    },
                )),
                crate::proto::golem::rib::variable_id::VariableId::ListAggregationIdentifier(
                    list_aggregation,
                ) => Ok(VariableId::ListReduce(crate::ListAggregationIdentifier {
                    name: list_aggregation.name,
                })),
            }
        }
    }

    impl From<VariableId> for ProtoVariableId {
        fn from(value: VariableId) -> Self {
            match value {
                VariableId::Global(name) => ProtoVariableId {
                    variable_id: Some(
                        crate::proto::golem::rib::variable_id::VariableId::Global(
                            crate::proto::golem::rib::Global { name },
                        ),
                    ),
                },
                VariableId::MatchIdentifier(m) => ProtoVariableId {
                    variable_id: Some(
                        crate::proto::golem::rib::variable_id::VariableId::MatchIdentifier(
                            crate::proto::golem::rib::MatchIdentifier {
                                name: m.name,
                                match_arm_index: m.match_arm_index as u32,
                            },
                        ),
                    ),
                },
                VariableId::Local(name, id) => ProtoVariableId {
                    variable_id: Some(
                        crate::proto::golem::rib::variable_id::VariableId::Local(
                            crate::proto::golem::rib::Local {
                                name,
                                id: id.map(|x| x.0 as u64),
                            },
                        ),
                    ),
                },
                VariableId::ListComprehension(l) => ProtoVariableId {
                    variable_id: Some(
                        crate::proto::golem::rib::variable_id::VariableId::ListComprehensionIdentifier(
                            crate::proto::golem::rib::ListComprehensionIdentifier {
                                name: l.name,
                            },
                        ),
                    ),
                },
                VariableId::ListReduce(r) => ProtoVariableId {
                    variable_id: Some(
                        crate::proto::golem::rib::variable_id::VariableId::ListAggregationIdentifier(
                            crate::proto::golem::rib::ListAggregationIdentifier {
                                name: r.name,
                            },
                        ),
                    ),
                },
            }
        }
    }
}
