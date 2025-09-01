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

use crate::inferred_type::TypeOrigin;
use crate::{ComponentDependencyKey, InferredType, InstanceType};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Ord, PartialOrd)]
pub enum TypeInternal {
    Bool,
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
    Chr,
    Str,
    List(InferredType),
    Tuple(Vec<InferredType>),
    Record(Vec<(String, InferredType)>),
    Flags(Vec<String>),
    Enum(Vec<String>),
    Option(InferredType),
    Result {
        ok: Option<InferredType>,
        error: Option<InferredType>,
    },
    Variant(Vec<(String, Option<InferredType>)>),
    Resource {
        name: Option<String>,
        owner: Option<String>,
        resource_id: u64,
        resource_mode: u8,
    },
    Range {
        from: InferredType,
        to: Option<InferredType>,
    },
    Instance {
        instance_type: Box<InstanceType>,
    },
    AllOf(Vec<InferredType>),
    Unknown,
    // Because function result can be a vector of types
    Sequence(Vec<InferredType>),
}

impl TypeInternal {
    pub fn is_instance(&self) -> bool {
        matches!(self, TypeInternal::Instance { .. })
    }

    pub fn to_inferred_type(&self) -> InferredType {
        InferredType::new(self.clone(), TypeOrigin::NoOrigin)
    }

    pub fn narrow_to_single_component(
        &mut self,
        component_dependency_key: &ComponentDependencyKey,
    ) {
        if let TypeInternal::Instance { instance_type } = self {
            instance_type.narrow_to_single_component(component_dependency_key)
        }
    }
}

impl Eq for TypeInternal {}

impl Hash for TypeInternal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            TypeInternal::Bool => 0.hash(state),
            TypeInternal::S8 => 1.hash(state),
            TypeInternal::U8 => 2.hash(state),
            TypeInternal::S16 => 3.hash(state),
            TypeInternal::U16 => 4.hash(state),
            TypeInternal::S32 => 5.hash(state),
            TypeInternal::U32 => 6.hash(state),
            TypeInternal::S64 => 7.hash(state),
            TypeInternal::U64 => 8.hash(state),
            TypeInternal::F32 => 9.hash(state),
            TypeInternal::F64 => 10.hash(state),
            TypeInternal::Chr => 11.hash(state),
            TypeInternal::Str => 12.hash(state),
            TypeInternal::List(inner) => {
                13.hash(state);
                inner.hash(state);
            }
            TypeInternal::Tuple(inner) => {
                14.hash(state);
                inner.hash(state);
            }
            TypeInternal::Record(fields) => {
                15.hash(state);
                let mut sorted_fields = fields.clone();
                sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
                sorted_fields.hash(state);
            }
            TypeInternal::Flags(flags) => {
                16.hash(state);
                let mut sorted_flags = flags.clone();
                sorted_flags.sort();
                sorted_flags.hash(state);
            }
            TypeInternal::Enum(variants) => {
                17.hash(state);
                let mut sorted_variants = variants.clone();
                sorted_variants.sort();
                sorted_variants.hash(state);
            }
            TypeInternal::Option(inner) => {
                18.hash(state);
                inner.hash(state);
            }
            TypeInternal::Result { ok, error } => {
                19.hash(state);
                ok.hash(state);
                error.hash(state);
            }
            TypeInternal::Variant(fields) => {
                20.hash(state);
                let mut sorted_fields = fields.clone();
                sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
                sorted_fields.hash(state);
            }
            TypeInternal::Resource {
                resource_id,
                resource_mode,
                name,
                owner,
            } => {
                21.hash(state);
                resource_id.hash(state);
                resource_mode.hash(state);
                if let Some(name) = name {
                    name.hash(state);
                } else {
                    "name-unknown".hash(state);
                }

                if let Some(owner) = owner {
                    owner.hash(state);
                } else {
                    "owner-unknown".hash(state);
                }
            }
            TypeInternal::Range { from, to } => {
                22.hash(state);
                from.hash(state);
                to.hash(state);
            }
            TypeInternal::Instance { instance_type } => {
                23.hash(state);
                instance_type.hash(state);
            }
            TypeInternal::AllOf(types) | TypeInternal::Sequence(types) => {
                24.hash(state);
                let mut sorted_types = types.clone();
                sorted_types.sort();
                sorted_types.hash(state);
            }
            TypeInternal::Unknown => 25.hash(state),
        }
    }
}

impl PartialEq for TypeInternal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TypeInternal::Bool, TypeInternal::Bool) => true,
            (TypeInternal::S8, TypeInternal::S8) => true,
            (TypeInternal::U8, TypeInternal::U8) => true,
            (TypeInternal::S16, TypeInternal::S16) => true,
            (TypeInternal::U16, TypeInternal::U16) => true,
            (TypeInternal::S32, TypeInternal::S32) => true,
            (TypeInternal::U32, TypeInternal::U32) => true,
            (TypeInternal::S64, TypeInternal::S64) => true,
            (TypeInternal::U64, TypeInternal::U64) => true,
            (TypeInternal::F32, TypeInternal::F32) => true,
            (TypeInternal::F64, TypeInternal::F64) => true,
            (TypeInternal::Chr, TypeInternal::Chr) => true,
            (TypeInternal::Str, TypeInternal::Str) => true,
            (TypeInternal::List(t1), TypeInternal::List(t2)) => t1 == t2,
            (TypeInternal::Tuple(ts1), TypeInternal::Tuple(ts2)) => ts1 == ts2,
            (TypeInternal::Record(fs1), TypeInternal::Record(fs2)) => fs1 == fs2,
            (TypeInternal::Flags(vs1), TypeInternal::Flags(vs2)) => vs1 == vs2,
            (TypeInternal::Enum(vs1), TypeInternal::Enum(vs2)) => vs1 == vs2,
            (TypeInternal::Option(t1), TypeInternal::Option(t2)) => t1 == t2,
            (
                TypeInternal::Result {
                    ok: ok1,
                    error: error1,
                },
                TypeInternal::Result {
                    ok: ok2,
                    error: error2,
                },
            ) => ok1 == ok2 && error1 == error2,
            (TypeInternal::Variant(vs1), TypeInternal::Variant(vs2)) => vs1 == vs2,
            (
                TypeInternal::Resource {
                    resource_id: id1,
                    resource_mode: mode1,
                    name: name1,
                    owner: owner1,
                },
                TypeInternal::Resource {
                    resource_id: id2,
                    resource_mode: mode2,
                    name: name2,
                    owner: owner2,
                },
            ) => id1 == id2 && mode1 == mode2 && name1 == name2 && owner1 == owner2,
            (
                TypeInternal::Range {
                    from: from1,
                    to: to1,
                },
                TypeInternal::Range {
                    from: from2,
                    to: to2,
                },
            ) => from1 == from2 && to1 == to2,
            (
                TypeInternal::Instance { instance_type: t1 },
                TypeInternal::Instance { instance_type: t2 },
            ) => t1 == t2,
            (TypeInternal::Unknown, TypeInternal::Unknown) => true,

            (TypeInternal::AllOf(ts1), TypeInternal::AllOf(ts2)) => {
                let mut ts1_sorted = ts1.clone();
                let mut ts2_sorted = ts2.clone();
                ts1_sorted.sort();
                ts2_sorted.sort();
                ts1_sorted == ts2_sorted
            }
            (TypeInternal::Sequence(ts1), TypeInternal::Sequence(ts2)) => {
                let mut ts1_sorted = ts1.clone();
                let mut ts2_sorted = ts2.clone();
                ts1_sorted.sort();
                ts2_sorted.sort();
                ts1_sorted == ts2_sorted
            }

            _ => false,
        }
    }
}
